//! Lock-free publication and click-free activation of prepared stages.
//!
//! The control plane (UI / model-loader thread) fully constructs the next
//! [`Stage`] — including weight loading and inference-session setup — and
//! hands it to the inference thread through a bounded lock-free queue
//! ([`StagePublisher::publish`]). The inference thread ([`SwitchingEngine`])
//! fades the current stage to silence, swaps ownership, and fades the new
//! stage in, so a model switch never clicks and never blocks the audio path
//! (docs/tech-research.md §9).
//!
//! The engine also owns the dry/wet intensity blend ([`crate::mix`]):
//! it runs *before* the switch fade gain, so during a model switch the
//! whole mixed signal — wet and delay-compensated dry alike — fades out
//! and back in. That ordering is what lets the dry-compensation delay
//! jump to the new stage's latency exactly at the silent fade boundary,
//! so switching stages with different latencies stays click-free at any
//! intensity.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_queue::ArrayQueue;

use crate::error::StageError;
use crate::mix::{DryWetMixer, IntensityControl};
use crate::stage::Stage;

struct PreparedStage {
    generation: u64,
    stage: Box<dyn Stage>,
}

/// Producer-side handle used by the UI or model-loader thread.
///
/// The queue has one preallocated slot: publishing a newer stage replaces an
/// older unconsumed stage without locking the inference thread.
#[derive(Clone)]
pub struct StagePublisher {
    queue: Arc<ArrayQueue<PreparedStage>>,
    generation: Arc<AtomicU64>,
}

impl std::fmt::Debug for StagePublisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagePublisher")
            .field("generation", &self.generation.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl StagePublisher {
    /// Publishes a fully initialized stage for activation by the inference
    /// thread. Returns the id of a previously published stage that was
    /// superseded before the inference thread consumed it, if any.
    #[must_use = "a superseded stage id usually indicates rapid re-publication worth logging"]
    pub fn publish(&self, stage: Box<dyn Stage>) -> Option<String> {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let replaced = self.queue.force_push(PreparedStage { generation, stage });
        replaced.map(|prepared| prepared.stage.id().to_owned())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Transition {
    Stable,
    FadeOut { remaining: usize },
    FadeIn { completed: usize },
}

/// Inference-thread owner of the active stage.
///
/// Retrieving a published replacement requires no mutex and no heap
/// allocation; the pending stage is warmed (fed input) while the current one
/// fades out, so its internal pipeline is primed at activation time.
pub struct SwitchingEngine {
    queue: Arc<ArrayQueue<PreparedStage>>,
    current: Box<dyn Stage>,
    current_generation: u64,
    pending: Option<PreparedStage>,
    transition: Transition,
    fade_samples: usize,
    warmup_output: Vec<f32>,
    mixer: DryWetMixer,
}

impl std::fmt::Debug for SwitchingEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwitchingEngine")
            .field("active", &self.current.id())
            .field("generation", &self.current_generation)
            .field("transition", &self.transition)
            .finish_non_exhaustive()
    }
}

/// Longest supported fade half. Bounds the fade so the internal gain
/// arithmetic is exact (`u16` → `f32` is lossless); ~1.37 s at 48 kHz,
/// orders of magnitude above any sensible switch fade.
pub const MAX_FADE_SAMPLES: usize = u16::MAX as usize;

impl SwitchingEngine {
    /// Creates a switching engine and its producer-side publisher.
    ///
    /// `fade_samples` is the length of each fade half (out and in) in
    /// samples at [`crate::stage::ENGINE_SAMPLE_RATE`]; `max_block_len`
    /// pre-sizes the warmup buffer (larger blocks still work at the cost of
    /// a reallocation on the control-of-flow path, not the audio callback).
    /// `intensity` is the shared dry/wet control the engine's mixer reads
    /// each block; the caller keeps a clone to move the slider without
    /// touching the engine.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Unsupported`] when `fade_samples` is zero or
    /// exceeds [`MAX_FADE_SAMPLES`].
    pub fn new(
        initial: Box<dyn Stage>,
        fade_samples: usize,
        max_block_len: usize,
        intensity: IntensityControl,
    ) -> Result<(StagePublisher, Self), StageError> {
        if fade_samples == 0 {
            return Err(StageError::Unsupported(
                "switch fade must contain at least one sample".to_owned(),
            ));
        }
        if fade_samples > MAX_FADE_SAMPLES {
            return Err(StageError::Unsupported(format!(
                "switch fade of {fade_samples} samples exceeds the supported \
                 maximum of {MAX_FADE_SAMPLES}"
            )));
        }
        let queue = Arc::new(ArrayQueue::new(1));
        let publisher = StagePublisher {
            queue: Arc::clone(&queue),
            generation: Arc::new(AtomicU64::new(0)),
        };
        Ok((
            publisher,
            Self {
                queue,
                current: initial,
                current_generation: 0,
                pending: None,
                transition: Transition::Stable,
                fade_samples,
                warmup_output: vec![0.0; max_block_len],
                mixer: DryWetMixer::new(intensity),
            },
        ))
    }

    /// Identifier of the currently audible stage.
    #[must_use]
    pub fn active_id(&self) -> &str {
        self.current.id()
    }

    /// Monotonic generation assigned when the current stage was published.
    #[must_use]
    pub const fn active_generation(&self) -> u64 {
        self.current_generation
    }

    /// Processes one block at the engine rate and advances any switch
    /// transition. `output.len()` must equal `input.len()`.
    ///
    /// The block passes through three steps in order: the active stage
    /// (wet), the dry/wet intensity blend (dry delayed by the *active*
    /// stage's reported latency), then the switch fade gain — so a
    /// switch fades the complete mix, and the dry-compensation delay
    /// only ever changes at the silent fade boundary.
    ///
    /// # Errors
    ///
    /// Propagates [`StageError`] from the active or warming stage.
    pub fn process_block(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        if self.transition == Transition::Stable
            && let Some(prepared) = self.queue.pop()
        {
            self.pending = Some(prepared);
            self.transition = Transition::FadeOut {
                remaining: self.fade_samples,
            };
        }

        self.current.process_block(input, output)?;
        let dry_delay = self.current.latency_samples();
        self.mixer.blend(input, output, dry_delay);
        match self.transition {
            Transition::Stable => {}
            Transition::FadeOut { mut remaining } => {
                if let Some(prepared) = &mut self.pending {
                    self.warmup_output.resize(input.len(), 0.0);
                    prepared
                        .stage
                        .process_block(input, &mut self.warmup_output[..input.len()])?;
                }
                for sample in output.iter_mut() {
                    *sample *= ratio(remaining, self.fade_samples);
                    remaining = remaining.saturating_sub(1);
                }
                if remaining == 0 {
                    self.activate_pending()?;
                    self.transition = Transition::FadeIn { completed: 0 };
                } else {
                    self.transition = Transition::FadeOut { remaining };
                }
            }
            Transition::FadeIn { mut completed } => {
                for sample in output.iter_mut() {
                    *sample *= ratio(completed, self.fade_samples);
                    completed = completed.saturating_add(1).min(self.fade_samples);
                }
                self.transition = if completed == self.fade_samples {
                    Transition::Stable
                } else {
                    Transition::FadeIn { completed }
                };
            }
        }
        Ok(())
    }

    fn activate_pending(&mut self) -> Result<(), StageError> {
        let mut prepared = self.pending.take().ok_or_else(|| {
            StageError::Unsupported("fade-out completed without a pending stage".to_owned())
        })?;
        std::mem::swap(&mut self.current, &mut prepared.stage);
        self.current_generation = prepared.generation;
        // `prepared` (now holding the superseded stage) is dropped here, on
        // the inference thread — allowed, since only the audio I/O callback
        // is allocation-free (docs/tech-research.md §9).
        Ok(())
    }
}

/// Exact fade gain: both operands fit `u16` (enforced by
/// [`MAX_FADE_SAMPLES`] in [`SwitchingEngine::new`]) and `u16` → `f32` is
/// lossless, so no precision-loss opt-out is needed.
fn ratio(numerator: usize, denominator: usize) -> f32 {
    debug_assert!(denominator <= MAX_FADE_SAMPLES);
    let numerator = u16::try_from(numerator.min(denominator)).unwrap_or(u16::MAX);
    let denominator = u16::try_from(denominator).unwrap_or(u16::MAX);
    f32::from(numerator) / f32::from(denominator)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Emits a constant value regardless of input, with a reported latency.
    #[derive(Debug)]
    struct Constant {
        id: &'static str,
        value: f32,
        latency: usize,
    }

    impl Stage for Constant {
        fn id(&self) -> &str {
            self.id
        }

        fn process_block(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
            if input.len() != output.len() {
                return Err(StageError::BufferLen {
                    expected: input.len(),
                    got: output.len(),
                });
            }
            output.fill(self.value);
            Ok(())
        }

        fn latency_samples(&self) -> usize {
            self.latency
        }

        fn reset(&mut self) {}
    }

    fn boxed(id: &'static str, value: f32) -> Box<dyn Stage> {
        Box::new(Constant {
            id,
            value,
            latency: 0,
        })
    }

    #[test]
    fn zero_fade_is_rejected() {
        assert!(matches!(
            SwitchingEngine::new(boxed("a", 1.0), 0, 480, IntensityControl::default()),
            Err(StageError::Unsupported(_))
        ));
    }

    #[test]
    fn oversized_fade_is_rejected() {
        assert!(matches!(
            SwitchingEngine::new(
                boxed("a", 1.0),
                MAX_FADE_SAMPLES + 1,
                480,
                IntensityControl::default()
            ),
            Err(StageError::Unsupported(_))
        ));
        let bounded = SwitchingEngine::new(
            boxed("a", 1.0),
            MAX_FADE_SAMPLES,
            480,
            IntensityControl::default(),
        );
        assert!(bounded.is_ok());
    }

    #[test]
    fn switch_fades_out_then_in_without_discontinuity() {
        let fade = 240;
        let (publisher, mut engine) =
            SwitchingEngine::new(boxed("a", 1.0), fade, 480, IntensityControl::default())
                .expect("engine builds");
        let input = [0.0_f32; 480];
        let mut output = [0.0_f32; 480];

        engine.process_block(&input, &mut output).expect("stable");
        assert!(output.iter().all(|s| (*s - 1.0).abs() < 1e-6));
        assert_eq!(engine.active_id(), "a");

        assert!(publisher.publish(boxed("b", -1.0)).is_none());

        // Fade-out of "a": first block after publication.
        engine.process_block(&input, &mut output).expect("fade-out");
        assert!((output[0] - 1.0).abs() < 1e-2);
        assert!(output[fade..].iter().all(|s| s.abs() < 1e-6));
        assert_eq!(engine.active_id(), "b");

        // Fade-in of "b".
        engine.process_block(&input, &mut output).expect("fade-in");
        assert!(output[0].abs() < 1e-2);
        assert!((output[fade] - -1.0).abs() < 1e-6);
        assert_eq!(engine.active_generation(), 1);

        // Stable on the replacement.
        engine.process_block(&input, &mut output).expect("stable");
        assert!(output.iter().all(|s| (*s - -1.0).abs() < 1e-6));
    }

    #[test]
    fn publishing_twice_supersedes_the_unconsumed_stage() {
        let (publisher, mut engine) =
            SwitchingEngine::new(boxed("a", 1.0), 4, 16, IntensityControl::default())
                .expect("engine builds");
        assert!(publisher.publish(boxed("b", 2.0)).is_none());
        assert_eq!(publisher.publish(boxed("c", 3.0)).as_deref(), Some("b"));

        let input = [0.0_f32; 16];
        let mut output = [0.0_f32; 16];
        engine.process_block(&input, &mut output).expect("switch");
        assert_eq!(engine.active_id(), "c");
        assert_eq!(engine.active_generation(), 2);
    }

    #[test]
    fn fade_spans_multiple_short_blocks() {
        let fade = 32;
        let (publisher, mut engine) =
            SwitchingEngine::new(boxed("a", 1.0), fade, 8, IntensityControl::default())
                .expect("engine builds");
        let superseded = publisher.publish(boxed("b", 1.0));
        assert!(superseded.is_none());
        let input = [0.0_f32; 8];
        let mut all = Vec::new();
        for _ in 0..12 {
            let mut output = [0.0_f32; 8];
            engine.process_block(&input, &mut output).expect("block");
            all.extend_from_slice(&output);
        }
        // Bounded step size between consecutive samples: no click.
        for pair in all.windows(2) {
            assert!(
                (pair[1] - pair[0]).abs() <= 1.0 / 16.0 + 1e-6,
                "discontinuity {} -> {}",
                pair[0],
                pair[1]
            );
        }
        assert_eq!(engine.active_id(), "b");
        assert!(all.last().is_some_and(|s| (*s - 1.0).abs() < 1e-6));
    }

    #[test]
    fn partial_intensity_blends_the_delay_compensated_dry_signal() {
        // A "silent" stage with latency: at intensity 0.5 the output is
        // exactly half the dry signal, delayed by the stage's reported
        // latency — proving the blend runs and the dry path is aligned.
        let latency = 100;
        let stage = Box::new(Constant {
            id: "silent",
            value: 0.0,
            latency,
        });
        let (_publisher, mut engine) =
            SwitchingEngine::new(stage, 240, 480, IntensityControl::new(0.5))
                .expect("engine builds");
        #[expect(clippy::cast_precision_loss, reason = "test signal indices are small")]
        let input: Vec<f32> = (0..960).map(|n| n as f32 * 1e-3).collect();
        let mut output = vec![0.0_f32; 960];
        for (chunk_in, chunk_out) in input.chunks(480).zip(output.chunks_mut(480)) {
            engine.process_block(chunk_in, chunk_out).expect("block");
        }
        for n in latency..960 {
            let expected = input[n - latency] * 0.5;
            assert!(
                (output[n] - expected).abs() < 1e-5,
                "sample {n}: {} != {expected}",
                output[n]
            );
        }
    }

    #[test]
    fn switching_between_latencies_at_partial_intensity_stays_click_free() {
        // The dry-compensation delay jumps from the old stage's latency
        // to the new one's exactly at the silent fade boundary; on a
        // steadily rising input any unmasked jump would appear as a
        // step. Bounded sample-to-sample deltas prove the fade masks it.
        let fade = 240;
        let old = Box::new(Constant {
            id: "old",
            value: 0.2,
            latency: 48,
        });
        let (publisher, mut engine) =
            SwitchingEngine::new(old, fade, 480, IntensityControl::new(0.5))
                .expect("engine builds");
        let superseded = publisher.publish(Box::new(Constant {
            id: "new",
            value: 0.2,
            latency: 2000,
        }));
        assert!(superseded.is_none());
        #[expect(clippy::cast_precision_loss, reason = "test signal indices are small")]
        let input: Vec<f32> = (0..4800).map(|n| n as f32 * 1e-4).collect();
        let mut all = Vec::new();
        for chunk in input.chunks(480) {
            let mut output = vec![0.0_f32; 480];
            engine.process_block(chunk, &mut output).expect("block");
            all.extend_from_slice(&output);
        }
        assert_eq!(engine.active_id(), "new");
        // Loosest legitimate slope: the fade ramp over a signal that
        // peaks below 0.5 → well under 0.5/240 per sample.
        for (n, pair) in all.windows(2).enumerate() {
            assert!(
                (pair[1] - pair[0]).abs() <= 0.5 / 240.0 + 1e-6,
                "discontinuity at {n}: {} -> {}",
                pair[0],
                pair[1]
            );
        }
    }
}
