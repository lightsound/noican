use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

#[cfg(unix)]
type DirectoryIdentity = (u64, u64);

#[cfg(unix)]
fn directory_identity(dir: &Path) -> std::io::Result<DirectoryIdentity> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::metadata(dir)?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
type DirectoryIdentity = PathBuf;

#[cfg(not(unix))]
fn directory_identity(dir: &Path) -> std::io::Result<DirectoryIdentity> {
    dir.canonicalize()
}

pub(crate) fn ensure_distinct_outputs(inputs: &[PathBuf], out_dir: &Path) -> anyhow::Result<()> {
    let mut seen: HashMap<DirectoryIdentity, &Path> = HashMap::new();
    for input in inputs {
        let stem = input
            .file_stem()
            .and_then(|s| s.to_str())
            .context("input file has no valid stem")?;
        let file_dir = out_dir.join(stem);
        std::fs::create_dir_all(&file_dir)
            .with_context(|| format!("cannot create {}", file_dir.display()))?;
        let identity = directory_identity(&file_dir)
            .with_context(|| format!("cannot inspect {}", file_dir.display()))?;
        if let Some(earlier) = seen.insert(identity, input) {
            anyhow::bail!(
                "{} and {} would both write to {}; rename one or process them with different --out-dir values",
                earlier.display(),
                input.display(),
                file_dir.display()
            );
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(line: u32) -> PathBuf {
        std::env::temp_dir().join(format!("noican-process-test-{}-{line}", std::process::id()))
    }

    #[test]
    fn inputs_with_distinct_stems_are_accepted() {
        let out_dir = scratch_dir(line!());
        let inputs = [PathBuf::from("a/one.wav"), PathBuf::from("a/two.wav")];
        ensure_distinct_outputs(&inputs, &out_dir).expect("distinct stems");
        assert!(out_dir.join("one").is_dir() && out_dir.join("two").is_dir());
        std::fs::remove_dir_all(&out_dir).expect("cleanup");
    }

    #[test]
    fn inputs_that_resolve_to_one_directory_are_refused_before_any_work() {
        let out_dir = scratch_dir(line!());
        for pair in [["a/take.wav", "b/take.wav"], ["take.wav", "take.flac"]] {
            let inputs = pair.map(PathBuf::from);
            let error = ensure_distinct_outputs(&inputs, &out_dir).expect_err("shared directory");
            let message = error.to_string();
            assert!(
                message.contains(pair[0]) && message.contains(pair[1]),
                "{message}"
            );
        }
        std::fs::remove_dir_all(&out_dir).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn two_names_for_one_directory_are_refused() {
        let out_dir = scratch_dir(line!());
        std::fs::create_dir_all(out_dir.join("take")).expect("temp dir");
        std::os::unix::fs::symlink(out_dir.join("take"), out_dir.join("alias")).expect("alias");
        let inputs = [PathBuf::from("take.wav"), PathBuf::from("alias.wav")];
        let error = ensure_distinct_outputs(&inputs, &out_dir).expect_err("one directory");
        assert!(error.to_string().contains("alias.wav"), "{error}");
        std::fs::remove_dir_all(&out_dir).expect("cleanup");
    }
}
