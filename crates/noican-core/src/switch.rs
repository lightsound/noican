//! Lock-free runtime model switching with click-free crossfades.
//!
//! The control plane (UI/CLI) builds a new [`Stage`] — an operation that
//! allocates and may take hundreds of milliseconds — and hands the box to
//! the processing thread through a wait-free SPSC ring. The processing
//! thread receives it between blocks, runs old and new stages in parallel
//! for a short equal-power crossfade, then drops the old stage (the drop
//! also happens off the audio I/O callback; see docs/tech-research.md §9 —
//! the audio callback itself only ever touches sample ring buffers).

use crate::error::StageError;
use crate::stage::{ENGINE_SAMPLE_RATE, Stage};

/// Default crossfade length: 20 ms at the engine rate.
pub const DEFAULT_CROSSFADE_SAMPLES: usize = (ENGINE_SAMPLE_RATE as usize) / 50;

/// Control-plane handle: sends freshly built stages to the switcher.
pub struct SwitchHandle {
    tx: rtrb::Producer<Box<dyn Stage>>,
}

impl std::fmt::Debug for SwitchHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwitchHandle").finish_non_exhaustive()
    }
}

impl SwitchHandle {
    /// Queues `stage` to become the active stage. Returns the stage back
    /// when the queue is full (the processing thread is not draining).
    ///
    /// # Errors
    ///
    /// Returns `Err(stage)` when the handoff ring is full.
    pub fn switch_to(&mut self, stage: Box<dyn Stage>) -> Result<(), Box<dyn Stage>> {
        self.tx.push(stage).map_err(|e| match e {
            rtrb::PushError::Full(stage) => stage,
        })
    }
}

/// Processing-thread side: owns the active stage and applies crossfades.
pub struct StageSwitcher {
    rx: rtrb::Consumer<Box<dyn Stage>>,
    current: Box<dyn Stage>,
    incoming: Option<Box<dyn Stage>>,
    /// Crossfade progress in samples (valid while `incoming` is some).
    fade_pos: usize,
    fade_len: usize,
    buf_old: Vec<f32>,
    buf_new: Vec<f32>,
}

impl std::fmt::Debug for StageSwitcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StageSwitcher")
            .field("current", &self.current.id())
            .field("switching", &self.incoming.is_some())
            .finish_non_exhaustive()
    }
}

impl StageSwitcher {
    /// Creates a switcher starting with `initial`, pre-sized for blocks of
    /// up to `max_block_len` samples, crossfading over `fade_len` samples.
    #[must_use]
    pub fn new(
        initial: Box<dyn Stage>,
        max_block_len: usize,
        fade_len: usize,
    ) -> (SwitchHandle, Self) {
        let (tx, rx) = rtrb::RingBuffer::new(2);
        (
            SwitchHandle { tx },
            Self {
                rx,
                current: initial,
                incoming: None,
                fade_pos: 0,
                fade_len: fade_len.max(1),
                buf_old: vec![0.0; max_block_len],
                buf_new: vec![0.0; max_block_len],
            },
        )
    }

    /// Identifier of the stage the output currently converges to.
    #[must_use]
    pub fn active_id(&self) -> &str {
        self.incoming
            .as_deref()
            .map_or_else(|| self.current.id(), Stage::id)
    }

    /// True while a crossfade is in progress.
    #[must_use]
    pub const fn is_switching(&self) -> bool {
        self.incoming.is_some()
    }

    /// Processes one block, absorbing any pending stage switch.
    ///
    /// # Errors
    ///
    /// Propagates stage failures.
    pub fn process_block(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        // Absorb pending switches. If a switch arrives mid-crossfade, the
        // in-flight new stage becomes current immediately (a single hard
        // step is preferable to stacking fades).
        while let Ok(next) = self.rx.pop() {
            if let Some(prev) = self.incoming.take() {
                self.current = prev;
            }
            self.incoming = Some(next);
            self.fade_pos = 0;
        }

        let Some(mut incoming) = self.incoming.take() else {
            return self.current.process_block(input, output);
        };

        if self.buf_old.len() < input.len() {
            self.buf_old.resize(input.len(), 0.0);
            self.buf_new.resize(input.len(), 0.0);
        }
        let old_out = &mut self.buf_old[..input.len()];
        let new_out = &mut self.buf_new[..input.len()];
        self.current.process_block(input, old_out)?;
        incoming.process_block(input, new_out)?;

        #[allow(
            clippy::cast_precision_loss,
            reason = "crossfade lengths are tiny; exact f32 representation"
        )]
        let inv_len = 1.0 / self.fade_len as f32;
        for (i, out) in output.iter_mut().enumerate() {
            #[allow(
                clippy::cast_precision_loss,
                reason = "crossfade positions are tiny; exact f32 representation"
            )]
            let t = ((self.fade_pos + i).min(self.fade_len) as f32) * inv_len;
            // Equal-power crossfade.
            let phase = t * std::f32::consts::FRAC_PI_2;
            let (g_new, g_old) = phase.sin_cos();
            *out = old_out[i].mul_add(g_old, new_out[i] * g_new);
        }
        self.fade_pos += input.len();

        if self.fade_pos >= self.fade_len {
            // Old stage is dropped here, on the processing thread.
            self.current = incoming;
        } else {
            self.incoming = Some(incoming);
        }
        Ok(())
    }

    /// Resets the active stage (and abandons any in-flight switch target).
    pub fn reset(&mut self) {
        self.current.reset();
        if let Some(mut incoming) = self.incoming.take() {
            incoming.reset();
            self.current = incoming;
        }
        self.fade_pos = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stage that multiplies the input by a constant gain.
    #[derive(Debug)]
    struct Gain {
        id: &'static str,
        gain: f32,
    }

    impl Stage for Gain {
        fn id(&self) -> &str {
            self.id
        }
        fn process_block(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
            for (o, i) in output.iter_mut().zip(input) {
                *o = i * self.gain;
            }
            Ok(())
        }
        fn latency_samples(&self) -> usize {
            0
        }
        fn reset(&mut self) {}
    }

    #[test]
    fn switch_crossfades_without_discontinuity() {
        let (mut handle, mut switcher) =
            StageSwitcher::new(Box::new(Gain { id: "a", gain: 1.0 }), 128, 480);
        let input = vec![1.0_f32; 128];
        let mut output = vec![0.0_f32; 128];

        switcher.process_block(&input, &mut output).expect("ok");
        assert!(output.iter().all(|s| (*s - 1.0).abs() < 1e-6));
        assert_eq!(switcher.active_id(), "a");

        handle
            .switch_to(Box::new(Gain { id: "b", gain: 0.5 }))
            .map_err(|_| ())
            .expect("queue accepts");

        // Collect the whole transition and verify sample-to-sample steps
        // stay small (no clicks) and the endpoint is the new gain.
        let mut all = Vec::new();
        for _ in 0..8 {
            switcher.process_block(&input, &mut output).expect("ok");
            all.extend_from_slice(&output);
        }
        assert_eq!(switcher.active_id(), "b");
        assert!(!switcher.is_switching());
        for pair in all.windows(2) {
            assert!((pair[1] - pair[0]).abs() < 0.02, "click detected: {pair:?}");
        }
        let tail = all.last().copied().expect("nonempty");
        assert!((tail - 0.5).abs() < 1e-4, "did not converge: {tail}");
    }

    #[test]
    fn rapid_double_switch_lands_on_last_stage() {
        let (mut handle, mut switcher) =
            StageSwitcher::new(Box::new(Gain { id: "a", gain: 1.0 }), 64, 480);
        handle
            .switch_to(Box::new(Gain { id: "b", gain: 0.5 }))
            .map_err(|_| ())
            .expect("queue accepts");
        handle
            .switch_to(Box::new(Gain {
                id: "c",
                gain: 0.25,
            }))
            .map_err(|_| ())
            .expect("queue accepts");
        let input = vec![1.0_f32; 64];
        let mut output = vec![0.0_f32; 64];
        for _ in 0..20 {
            switcher.process_block(&input, &mut output).expect("ok");
        }
        assert_eq!(switcher.active_id(), "c");
        let tail = output.last().copied().expect("nonempty");
        assert!((tail - 0.25).abs() < 1e-4);
    }
}
