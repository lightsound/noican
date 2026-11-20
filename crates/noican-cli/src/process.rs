use std::path::Path;
use std::time::Instant;

use anyhow::Context as _;
use noican_core::Stage;

use crate::audio;

pub(crate) const BLOCK_LEN: usize = 480;

pub(crate) fn run_stage_aligned(stage: &mut dyn Stage, input: &[f32]) -> anyhow::Result<Vec<f32>> {
    run_stage_aligned_timed(stage, input).map(|(output, _)| output)
}

pub(crate) fn run_stage_aligned_timed(
    stage: &mut dyn Stage,
    input: &[f32],
) -> anyhow::Result<(Vec<f32>, Vec<u128>)> {
    let latency = stage.latency_samples();
    let padded_len = input.len() + latency;
    let mut output = vec![0.0_f32; padded_len.next_multiple_of(BLOCK_LEN)];
    let mut padded = vec![0.0_f32; output.len()];
    padded[..input.len()].copy_from_slice(input);
    let mut block_times_ns = Vec::with_capacity(output.len() / BLOCK_LEN);

    for (in_block, out_block) in padded.chunks(BLOCK_LEN).zip(output.chunks_mut(BLOCK_LEN)) {
        let start = Instant::now();
        stage
            .process_block(in_block, out_block)
            .with_context(|| format!("stage {} failed", stage.id()))?;
        block_times_ns.push(start.elapsed().as_nanos());
    }
    output.drain(..latency);
    output.truncate(input.len());
    Ok((output, block_times_ns))
}

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

    let input = audio::read_mono_48k(input_path)?;
    let reference_path = file_dir.join("reference.wav");
    audio::write_mono_48k(&reference_path, &input)?;
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
        audio::write_mono_48k(&out_path, &output)?;
        progress(&format!("{stem}: {id} -> {}", out_path.display()));
    }
    Ok(())
}
