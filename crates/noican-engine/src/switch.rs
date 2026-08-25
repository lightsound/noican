//! Lock-free publication and click-free activation of prepared model stages.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use crossbeam_queue::ArrayQueue;

use crate::{
    validate_frame_lengths, AudioStage, StageDescriptor, StageError, PIPELINE_FRAME_SAMPLES,
    PIPELINE_SAMPLE_RATE,
};

struct PreparedStage {
    generation: u64,
    stage: Box<dyn AudioStage>,
}

/// Producer-side handle used by the UI or model-loader thread.
///
/// The queue has one preallocated slot. Publishing a newer stage replaces an
/// older unconsumed stage without locking the inference thread.
#[derive(Clone)]
pub struct StagePublisher {
    queue: Arc<ArrayQueue<PreparedStage>>,
    generation: Arc<AtomicU64>,
}

impl StagePublisher {
    /// Publish a fully initialized stage for activation by the inference thread.
    ///
    /// Returns metadata for a stage that was superseded before activation.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::InvalidConfiguration`] unless the stage already
    /// exposes noican's shared 48 kHz, 480-sample contract.
    pub fn publish(
        &self,
        stage: Box<dyn AudioStage>,
    ) -> Result<Option<StageDescriptor>, StageError> {
        validate_pipeline_contract(stage.descriptor())?;
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let replaced = self.queue.force_push(PreparedStage { generation, stage });
        Ok(replaced.map(|prepared| prepared.stage.descriptor()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Transition {
    Stable,
    FadeOut { remaining: usize },
    FadeIn { completed: usize },
}

/// Inference-thread owner of the active model stage.
///
/// A pending model is warmed while the current model fades to silence. The
/// engine then swaps ownership and fades the new model in. No mutex or heap
/// allocation is needed to retrieve a published replacement.
pub struct SwitchingEngine {
    queue: Arc<ArrayQueue<PreparedStage>>,
    current: Box<dyn AudioStage>,
    current_generation: u64,
    pending: Option<PreparedStage>,
    transition: Transition,
    fade_samples: usize,
    warmup_output: Vec<f32>,
}

impl SwitchingEngine {
    /// Create a switching engine and its producer-side publisher.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::InvalidConfiguration`] if `initial` does not use
    /// the shared pipeline contract or if `fade_samples` is zero.
    pub fn new(
        initial: Box<dyn AudioStage>,
        fade_samples: usize,
    ) -> Result<(StagePublisher, Self), StageError> {
        validate_pipeline_contract(initial.descriptor())?;
        if fade_samples == 0 {
            return Err(StageError::InvalidConfiguration {
                stage: initial.descriptor().id,
                message: "switch fade must contain at least one sample".to_owned(),
            });
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
                warmup_output: vec![0.0; PIPELINE_FRAME_SAMPLES],
            },
        ))
    }

    /// Descriptor of the currently audible stage.
    #[must_use]
    pub fn active_descriptor(&self) -> StageDescriptor {
        self.current.descriptor()
    }

    /// Monotonic generation assigned when the current stage was published.
    #[must_use]
    pub const fn active_generation(&self) -> u64 {
        self.current_generation
    }

    /// Process one shared pipeline frame and advance any switch transition.
    ///
    /// # Errors
    ///
    /// Returns a stage error from the active or warming model.
    pub fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        validate_frame_lengths(self.current.descriptor(), input, output)?;
        if self.transition == Transition::Stable {
            if let Some(prepared) = self.queue.pop() {
                self.pending = Some(prepared);
                self.transition = Transition::FadeOut {
                    remaining: self.fade_samples,
                };
            }
        }

        self.current.process_frame(input, output)?;
        match self.transition {
            Transition::Stable => {}
            Transition::FadeOut { mut remaining } => {
                if let Some(prepared) = &mut self.pending {
                    prepared
                        .stage
                        .process_frame(input, &mut self.warmup_output)?;
                }
                for sample in output.iter_mut() {
                    let gain = ratio(remaining, self.fade_samples);
                    *sample *= gain;
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
                    let gain = ratio(completed, self.fade_samples);
                    *sample *= gain;
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
        let mut prepared = self
            .pending
            .take()
            .ok_or_else(|| StageError::InvalidConfiguration {
                stage: self.current.descriptor().id,
                message: "fade-out completed without a pending stage".to_owned(),
            })?;
        std::mem::swap(&mut self.current, &mut prepared.stage);
        self.current_generation = prepared.generation;
        Ok(())
    }
}

fn validate_pipeline_contract(descriptor: StageDescriptor) -> Result<(), StageError> {
    if descriptor.sample_rate != PIPELINE_SAMPLE_RATE
        || descriptor.frame_samples != PIPELINE_FRAME_SAMPLES
    {
        return Err(StageError::InvalidConfiguration {
            stage: descriptor.id,
            message: format!(
                "switchable stages must use {PIPELINE_SAMPLE_RATE} Hz and \
                 {PIPELINE_FRAME_SAMPLES} samples, got {} Hz and {} samples",
                descriptor.sample_rate, descriptor.frame_samples
            ),
        });
    }
    Ok(())
}

fn ratio(numerator: usize, denominator: usize) -> f32 {
    let numerator = u16::try_from(numerator.min(denominator)).map_or(u16::MAX, |value| value);
    let denominator = u16::try_from(denominator).map_or(u16::MAX, |value| value);
    f32::from(numerator) / f32::from(denominator)
}
