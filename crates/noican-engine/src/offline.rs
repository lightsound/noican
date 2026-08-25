//! Deterministic whole-file orchestration over the streaming stage contract.

use crate::{AudioStage, StageError, PIPELINE_FRAME_SAMPLES, PIPELINE_SAMPLE_RATE};

/// Delay handling for offline comparison output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DelayCompensation {
    /// Keep the live-path delay as leading silence.
    Preserve,
    /// Drain the stage and remove its declared delay for aligned comparison.
    Remove,
}

/// Process one mono 48 kHz clip through a prepared stage.
///
/// The function uses the same 10 ms calls as the live engine, pads only the
/// final partial frame, and feeds the declared tail before truncating back to
/// the input length.
///
/// # Errors
///
/// Returns a model or configuration error from `stage`.
pub fn process_clip(
    stage: &mut dyn AudioStage,
    input: &[f32],
    delay_compensation: DelayCompensation,
) -> Result<Vec<f32>, StageError> {
    let descriptor = stage.descriptor();
    if descriptor.sample_rate != PIPELINE_SAMPLE_RATE
        || descriptor.frame_samples != PIPELINE_FRAME_SAMPLES
    {
        return Err(StageError::InvalidConfiguration {
            stage: descriptor.id,
            message: "offline runner requires a pipeline-adapted stage".to_owned(),
        });
    }
    stage.reset()?;

    let input_frames = input.len().div_ceil(PIPELINE_FRAME_SAMPLES);
    let delay_frames = descriptor
        .algorithmic_delay_samples
        .div_ceil(PIPELINE_FRAME_SAMPLES);
    let drain_frames = descriptor
        .tail_frames
        .checked_add(delay_frames)
        .and_then(|frames| frames.checked_add(1))
        .ok_or_else(|| StageError::InvalidConfiguration {
            stage: descriptor.id,
            message: "offline drain length overflow".to_owned(),
        })?;
    let total_frames =
        input_frames
            .checked_add(drain_frames)
            .ok_or_else(|| StageError::InvalidConfiguration {
                stage: descriptor.id,
                message: "offline frame count overflow".to_owned(),
            })?;
    let capacity = total_frames
        .checked_mul(PIPELINE_FRAME_SAMPLES)
        .ok_or_else(|| StageError::InvalidConfiguration {
            stage: descriptor.id,
            message: "offline output capacity overflow".to_owned(),
        })?;
    let mut rendered = Vec::with_capacity(capacity);
    let mut frame_in = [0.0_f32; PIPELINE_FRAME_SAMPLES];
    let mut frame_out = [0.0_f32; PIPELINE_FRAME_SAMPLES];

    for frame_index in 0..total_frames {
        frame_in.fill(0.0);
        if frame_index < input_frames {
            let start = frame_index * PIPELINE_FRAME_SAMPLES;
            let end = input.len().min(start + PIPELINE_FRAME_SAMPLES);
            frame_in[..end - start].copy_from_slice(&input[start..end]);
        }
        stage.process_frame(&frame_in, &mut frame_out)?;
        rendered.extend_from_slice(&frame_out);
    }

    let start = match delay_compensation {
        DelayCompensation::Preserve => 0,
        DelayCompensation::Remove => descriptor.algorithmic_delay_samples,
    }
    .min(rendered.len());
    let available = &rendered[start..];
    let mut output = Vec::with_capacity(input.len());
    output.extend(available.iter().take(input.len()).copied());
    output.resize(input.len(), 0.0);
    Ok(output)
}
