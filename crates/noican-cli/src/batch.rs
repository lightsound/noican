//! Reproducible multi-model WAV comparison runs.

use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

use anyhow::{bail, Context, Result};
use noican_engine::{process_clip, resample_clip, DelayCompensation, PIPELINE_SAMPLE_RATE};
use noican_models::{
    assets::{FetchOptions, ModelAsset, ModelStore},
    ecapa::{Ecapa, SAMPLE_RATE as ECAPA_SAMPLE_RATE},
    load_pipeline_stage, LoadRequest, ModelId, EMBEDDING_DIMENSIONS,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{model_store, selected_models, wav, ProcessArgs};

#[derive(Serialize)]
struct ComparisonManifest {
    schema_version: u32,
    input_path: String,
    input_sha256: String,
    sample_rate: u32,
    input_samples: usize,
    delay_compensation: &'static str,
    models: Vec<ModelResult>,
}

#[derive(Serialize)]
struct ModelResult {
    model: &'static str,
    display_name: &'static str,
    native_sample_rate: u32,
    status: ModelStatus,
    output: Option<String>,
    output_sha256: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum ModelStatus {
    Complete,
    Failed,
}

pub fn run(args: &ProcessArgs) -> Result<()> {
    let store = model_store(args.model_dir.clone())?;
    let mut models = selected_models(&args.models, args.all_models);
    deduplicate(&mut models);
    let token = std::env::var("NOICAN_HF_TOKEN").ok();
    let embedding = enrollment_embedding(&args, &store, token.as_deref())?;
    let delay_compensation = if args.preserve_delay {
        DelayCompensation::Preserve
    } else {
        DelayCompensation::Remove
    };
    let mut failed_runs = 0_usize;
    for input in &args.inputs {
        failed_runs += process_input(
            input,
            &args.output_dir,
            &store,
            &models,
            &embedding,
            token.as_deref(),
            delay_compensation,
        )?;
    }
    if failed_runs > 0 {
        bail!(
            "{failed_runs} model run(s) failed; successful outputs and failure details were retained"
        );
    }
    Ok(())
}

fn process_input(
    input_path: &Path,
    output_root: &Path,
    store: &ModelStore,
    models: &[ModelId],
    embedding: &Option<[f32; EMBEDDING_DIMENSIONS]>,
    token: Option<&str>,
    delay_compensation: DelayCompensation,
) -> Result<usize> {
    let source_hash = sha256_file(input_path)?;
    let decoded = wav::read_mono(input_path)?;
    let input = resample_clip(&decoded.samples, decoded.sample_rate, PIPELINE_SAMPLE_RATE)
        .with_context(|| format!("resample {}", input_path.display()))?;
    let run_directory = output_root.join(input_identifier(input_path, &source_hash));
    fs::create_dir_all(&run_directory)
        .with_context(|| format!("create output directory {}", run_directory.display()))?;

    let mut results = Vec::with_capacity(models.len());
    for model in models {
        eprintln!("processing {} with {}", input_path.display(), model.slug());
        let request = LoadRequest {
            model: *model,
            store,
            hugging_face_token: token,
            speaker_embedding: embedding.as_ref(),
        };
        let result = process_model(&run_directory, *model, &request, &input, delay_compensation);
        results.push(match result {
            Ok((output, output_sha256)) => ModelResult {
                model: model.slug(),
                display_name: model.display_name(),
                native_sample_rate: model.native_sample_rate(),
                status: ModelStatus::Complete,
                output: Some(output),
                output_sha256: Some(output_sha256),
                error: None,
            },
            Err(error) => ModelResult {
                model: model.slug(),
                display_name: model.display_name(),
                native_sample_rate: model.native_sample_rate(),
                status: ModelStatus::Failed,
                output: None,
                output_sha256: None,
                error: Some(format!("{error:#}")),
            },
        });
    }
    let failures = results
        .iter()
        .filter(|result| matches!(result.status, ModelStatus::Failed))
        .count();
    let manifest = ComparisonManifest {
        schema_version: 1,
        input_path: input_path.display().to_string(),
        input_sha256: source_hash,
        sample_rate: PIPELINE_SAMPLE_RATE,
        input_samples: input.len(),
        delay_compensation: match delay_compensation {
            DelayCompensation::Preserve => "preserve",
            DelayCompensation::Remove => "remove",
        },
        models: results,
    };
    write_manifest(&run_directory.join("comparison.json"), &manifest)?;
    println!("{}", run_directory.display());
    Ok(failures)
}

fn process_model(
    run_directory: &Path,
    model: ModelId,
    request: &LoadRequest<'_>,
    input: &[f32],
    delay_compensation: DelayCompensation,
) -> Result<(String, String)> {
    let mut stage =
        load_pipeline_stage(request).with_context(|| format!("prepare model {}", model.slug()))?;
    let output = process_clip(stage.as_mut(), input, delay_compensation)
        .with_context(|| format!("run model {}", model.slug()))?;
    if output.iter().any(|sample| !sample.is_finite()) {
        bail!("model {} produced non-finite audio", model.slug());
    }
    let model_directory = run_directory.join(model.slug());
    fs::create_dir_all(&model_directory)
        .with_context(|| format!("create model output {}", model_directory.display()))?;
    let path = model_directory.join("output.wav");
    wav::write_mono(&path, PIPELINE_SAMPLE_RATE, &output)?;
    let digest = sha256_file(&path)?;
    Ok((format!("{}/output.wav", model.slug()), digest))
}

fn enrollment_embedding(
    args: &ProcessArgs,
    store: &ModelStore,
    token: Option<&str>,
) -> Result<Option<[f32; EMBEDDING_DIMENSIONS]>> {
    if let Some(path) = &args.embedding_json {
        let values: Vec<f32> = serde_json::from_reader(
            File::open(path).with_context(|| format!("open embedding {}", path.display()))?,
        )
        .with_context(|| format!("parse embedding {}", path.display()))?;
        let embedding: [f32; EMBEDDING_DIMENSIONS] =
            values.try_into().map_err(|values: Vec<f32>| {
                anyhow::anyhow!(
                    "embedding {} contains {} values, expected {}",
                    path.display(),
                    values.len(),
                    EMBEDDING_DIMENSIONS
                )
            })?;
        return Ok(Some(embedding));
    }
    let Some(path) = &args.enrollment_wav else {
        return Ok(None);
    };
    let options = FetchOptions {
        hugging_face_token: token,
    };
    let model_path = store.ensure(ModelAsset::Ecapa, options)?;
    let filterbank_path = store.ensure(ModelAsset::EcapaFilterbank, options)?;
    let decoded = wav::read_mono(path)?;
    let audio = resample_clip(&decoded.samples, decoded.sample_rate, ECAPA_SAMPLE_RATE)
        .with_context(|| format!("resample enrollment {}", path.display()))?;
    let mut ecapa = Ecapa::load(model_path, filterbank_path)?;
    Ok(Some(ecapa.embed(&audio)?))
}

fn write_manifest(path: &Path, manifest: &ComparisonManifest) -> Result<()> {
    let mut file =
        File::create(path).with_context(|| format!("create manifest {}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, manifest)
        .with_context(|| format!("serialize manifest {}", path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("finish manifest {}", path.display()))
}

fn input_identifier(path: &Path, digest: &str) -> String {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("input");
    let sanitized: String = stem
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let prefix = digest.get(..8).unwrap_or(digest);
    format!("{sanitized}-{prefix}")
}

fn deduplicate(models: &mut Vec<ModelId>) {
    let mut seen = HashSet::new();
    models.retain(|model| seen.insert(*model));
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("hash {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let count = file
            .read(&mut buffer)
            .with_context(|| format!("hash {}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hexadecimal(&hasher.finalize()))
}

fn hexadecimal(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_identifier_is_stable_and_safe() {
        let identifier = input_identifier(
            Path::new("room sample?.wav"),
            "0123456789abcdef0123456789abcdef",
        );
        assert_eq!(identifier, "room_sample_-01234567");
    }

    #[test]
    fn model_selection_deduplicates_in_place() {
        let mut models = vec![ModelId::UlUnas, ModelId::UlUnas, ModelId::DeepFilterNet3];
        deduplicate(&mut models);
        assert_eq!(models, [ModelId::UlUnas, ModelId::DeepFilterNet3]);
    }
}
