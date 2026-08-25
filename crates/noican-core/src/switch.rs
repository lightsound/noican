//! Lock-free publication and click-free activation of prepared stages.
//!
//! The control plane (UI / model-loader thread) fully constructs the next
//! [`Stage`] — including weight loading and inference-session setup — and
//! hands it to the inference thread through a bounded lock-free queue
//! ([`StagePublisher::publish`]). The inference thread ([`SwitchingEngine`])
//! fades the current stage to silence, swaps ownership, and fades the new
//! stage in, so a model switch never clicks and never blocks the audio path
//! (docs/tech-research.md §9).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_queue::ArrayQueue;

use crate::error::StageError;
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

/// Longest supported fade half. Bounds the fade so [`ratio`] is exact
/// arithmetic (`u16` → `f32` is lossless); ~1.37 s at 48 kHz, orders of
/// magnitude above any sensible switch fade.
pub const MAX_FADE_SAMPLES: usize = u16::MAX as usize;

impl SwitchingEngine {
    /// Creates a switching engine and its producer-side publisher.
    ///
    /// `fade_samples` is the length of each fade half (out and in) in
    /// samples at [`crate::stage::ENGINE_SAMPLE_RATE`]; `max_block_len`
    /// pre-sizes the warmup buffer (larger blocks still work at the cost of
    /// a reallocation on the control-of-flow path, not the audio callback).
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Unsupported`] when `fade_samples` is zero or
    /// exceeds [`MAX_FADE_SAMPLES`].
    pub fn new(
        initial: Box<dyn Stage>,
        fade_samples: usize,
        max_block_len: usize,
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

    /// Emits a constant value regardless of input.
    #[derive(Debug)]
    struct Constant {
        id: &'static str,
        value: f32,
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
            0
        }

        fn reset(&mut self) {}
    }

    fn boxed(id: &'static str, value: f32) -> Box<dyn Stage> {
        Box::new(Constant { id, value })
    }

    #[test]
    fn zero_fade_is_rejected() {
        assert!(matches!(
            SwitchingEngine::new(boxed("a", 1.0), 0, 480),
            Err(StageError::Unsupported(_))
        ));
    }

    #[test]
    fn oversized_fade_is_rejected() {
        assert!(matches!(
            SwitchingEngine::new(boxed("a", 1.0), MAX_FADE_SAMPLES + 1, 480),
            Err(StageError::Unsupported(_))
        ));
        let bounded = SwitchingEngine::new(boxed("a", 1.0), MAX_FADE_SAMPLES, 480);
        assert!(bounded.is_ok());
    }

    #[test]
    fn switch_fades_out_then_in_without_discontinuity() {
        let fade = 240;
        let (publisher, mut engine) =
            SwitchingEngine::new(boxed("a", 1.0), fade, 480).expect("engine builds");
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
            SwitchingEngine::new(boxed("a", 1.0), 4, 16).expect("engine builds");
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
            SwitchingEngine::new(boxed("a", 1.0), fade, 8).expect("engine builds");
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
}
