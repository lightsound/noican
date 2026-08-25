//! Hush 16 kHz background-speaker suppression stage.

use std::collections::VecDeque;

use hush_vani::Hush as HushModel;
use noican_engine::{
    validate_frame_lengths, AudioStage, EnrollmentRequirement, StageDescriptor, StageError,
    StageKind,
};

const FRAME_SAMPLES: usize = 160;
const CONTEXT_FRAMES: usize = 20;
const CONTEXT_SAMPLES: usize = FRAME_SAMPLES * CONTEXT_FRAMES;

const DESCRIPTOR: StageDescriptor = StageDescriptor {
    id: "hush",
    display_name: "Hush 16 kHz",
    kind: StageKind::SpeakerSuppression,
    sample_rate: 16_000,
    frame_samples: FRAME_SAMPLES,
    algorithmic_delay_samples: HushModel::LATENCY_SAMPLES,
    tail_frames: 1,
    enrollment: EnrollmentRequirement::None,
};

/// Hush stage with a rolling causal context.
///
/// `hush-vani` currently exposes utterance inference rather than recurrent
/// state. Running a bounded 200 ms causal window preserves low latency and
/// avoids restarting from a single 10 ms frame.
pub struct Hush {
    model: HushModel,
    context: VecDeque<f32>,
}

impl Hush {
    /// Load the embedded Apache-2.0 Hush weights.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Backend`] if the embedded weight manifest is
    /// inconsistent.
    pub fn load() -> Result<Self, StageError> {
        let model = HushModel::new().map_err(backend_error)?;
        let mut stage = Self {
            model,
            context: VecDeque::with_capacity(CONTEXT_SAMPLES),
        };
        stage.prime_context();
        Ok(stage)
    }

    fn prime_context(&mut self) {
        self.context.clear();
        self.context
            .extend(std::iter::repeat_n(0.0, CONTEXT_SAMPLES - FRAME_SAMPLES));
    }
}

impl AudioStage for Hush {
    fn descriptor(&self) -> StageDescriptor {
        DESCRIPTOR
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        validate_frame_lengths(DESCRIPTOR, input, output)?;
        self.context.extend(input);
        while self.context.len() > CONTEXT_SAMPLES {
            self.context.pop_front();
        }
        let context: Vec<f32> = self.context.iter().copied().collect();
        let enhanced = self.model.enhance(&context).map_err(backend_error)?;
        let start = enhanced
            .len()
            .checked_sub(FRAME_SAMPLES)
            .ok_or_else(|| backend_error("Hush returned less than one frame"))?;
        output.copy_from_slice(&enhanced[start..]);
        Ok(())
    }

    fn reset(&mut self) -> Result<(), StageError> {
        self.prime_context();
        Ok(())
    }
}

fn backend_error(error: impl std::fmt::Display) -> StageError {
    StageError::Backend {
        stage: DESCRIPTOR.id,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_model_processes_one_frame() -> Result<(), StageError> {
        let mut stage = Hush::load()?;
        let mut output = [0.0_f32; FRAME_SAMPLES];
        stage.process_frame(&[0.0; FRAME_SAMPLES], &mut output)?;
        assert!(output.iter().all(|sample| sample.is_finite()));
        Ok(())
    }
}
