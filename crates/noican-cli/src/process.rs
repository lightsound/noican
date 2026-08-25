//! Batch WAV processing: run inputs through selected models and organize
//! outputs for side-by-side comparison.

use std::path::Path;

use anyhow::Context as _;
use noican_core::Stage;

use crate::wav;

/// Block size (48 kHz samples) used to drive stages. Mirrors a realistic
/// real-time block so offline results match live behavior.
pub(crate) const BLOCK_LEN: usize = 480;

/// Runs `input` through `stage`, compensating the stage's internal buffering
/// latency so the output is time-aligned with the input.
///
/// # Errors
///
/// Propagates stage processing failures.
pub(crate) fn run_stage_aligned(stage: &mut dyn Stage, input: &[f32]) -> anyhow::Result<Vec<f32>> {
    let latency = stage.latency_samples();
    let padded_len = input.len() + latency;
    let mut output = vec![0.0_f32; padded_len.next_multiple_of(BLOCK_LEN)];
    let mut padded = vec![0.0_f32; output.len()];
    padded[..input.len()].copy_from_slice(input);

    for (in_block, out_block) in padded.chunks(BLOCK_LEN).zip(output.chunks_mut(BLOCK_LEN)) {
        stage
            .process_block(in_block, out_block)
            .with_context(|| format!("stage {} failed", stage.id()))?;
    }
    output.drain(..latency);
    output.truncate(input.len());
    Ok(output)
}

/// Processes `input_path` through every model in `model_ids`, writing
/// `out_dir/<input_stem>/<model_id>.wav` plus a `reference.wav` (the input
/// converted to mono 48 kHz) for fair comparison.
///
/// `make_stage` maps a model id to a fresh stage (state is never shared
/// between files).
///
/// # Errors
///
/// Fails on I/O errors or when a stage cannot be created/run.
pub(crate) fn process_file(
    input_path: &Path,
    out_dir: &Path,
    model_ids: &[String],
    mut make_stage: impl FnMut(&str) -> anyhow::Result<Box<dyn Stage>>,
    mut progress: impl FnMut(&str),
) -> anyhow::Result<()> {
    let stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .context("input file has no valid stem")?;
    let file_dir = out_dir.join(stem);
    std::fs::create_dir_all(&file_dir)
        .with_context(|| format!("cannot create {}", file_dir.display()))?;

    let input = wav::read_mono_48k(input_path)?;
    let reference_path = file_dir.join("reference.wav");
    wav::write_mono_48k(&reference_path, &input)?;
    progress(&format!(
        "{}: {} samples @48k -> {}",
        stem,
        input.len(),
        reference_path.display()
    ));

    for id in model_ids {
        let mut stage = make_stage(id)?;
        let output = run_stage_aligned(stage.as_mut(), &input)?;
        let out_path = file_dir.join(format!("{id}.wav"));
        wav::write_mono_48k(&out_path, &output)?;
        progress(&format!("{stem}: {id} -> {}", out_path.display()));
    }
    Ok(())
}
