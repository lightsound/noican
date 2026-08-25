//! noican CLI: model weight management and batch WAV processing.
//!
//! The same engine stages used by the real-time pipeline are driven here in
//! file mode, giving strictly identical conditions for model comparison
//! (docs/tech-research.md §12, Phase 0).

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "user-facing CLI output is this binary's job"
)]

mod process;
mod wav;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use noican_models::{ALL_MODELS, ModelSpec, PASSTHROUGH_ID};

#[derive(Parser)]
#[command(name = "noican", version, about = "noican audio engine CLI")]
struct Cli {
    /// Directory holding downloaded model weights.
    #[arg(long, global = true, default_value = "models")]
    models_dir: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List supported models and their fetch status.
    Models,
    /// Download model weights from their official distribution points.
    Fetch {
        /// Model ids to fetch (default: all).
        ids: Vec<String>,
    },
    /// Process WAV files through models; outputs are organized per input
    /// file for side-by-side comparison.
    Process {
        /// Input WAV files.
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// Output directory (default: out).
        #[arg(long, default_value = "out")]
        out_dir: PathBuf,
        /// Model ids to run (default: passthrough + all fetched models).
        #[arg(long, value_delimiter = ',')]
        models: Vec<String>,
        /// Enrollment WAV of the target speaker (3–10 s of clean speech).
        /// Required by models that need speaker enrollment (tse-48k);
        /// they are skipped otherwise.
        #[arg(long)]
        enroll: Option<PathBuf>,
    },
}

/// Computes the 192-dim ECAPA-TDNN enrollment embedding from a WAV of the
/// target speaker: mono 48 kHz conversion, decimation to 16 kHz, then the
/// SpeechBrain-compatible fbank + ONNX pipeline.
fn enrollment_embedding(
    wav_path: &std::path::Path,
    models_dir: &std::path::Path,
) -> anyhow::Result<Vec<f32>> {
    use anyhow::Context as _;
    let spec = ModelSpec::find("ecapa-tdnn").context("ecapa-tdnn missing from registry")?;
    if !noican_models::fetch::is_fetched(models_dir, spec) {
        anyhow::bail!("ecapa-tdnn model not fetched; run: noican fetch ecapa-tdnn");
    }
    let audio_48k = wav::read_mono_48k(wav_path)?;
    let mut decimator = noican_core::resample::Decimator::new(3, audio_48k.len().max(3));
    let mut audio_16k = Vec::with_capacity(audio_48k.len() / 3);
    let usable = audio_48k.len() - audio_48k.len() % 3;
    decimator.process(&audio_48k[..usable], &mut audio_16k);
    if audio_16k.len() < 16_000 {
        anyhow::bail!("enrollment clip too short: need at least 1 s of audio");
    }
    let onnx_path = noican_models::fetch::model_dir(models_dir, spec).join(spec.files[0].name);
    let mut embedder = noican_models::embedding::EcapaEmbedder::new(&onnx_path)
        .map_err(|e| anyhow::anyhow!("loading ECAPA model: {e}"))?;
    let embedding = embedder
        .embed(&audio_16k)
        .map_err(|e| anyhow::anyhow!("computing enrollment embedding: {e}"))?;
    println!(
        "enrollment: {} -> {}-dim embedding",
        wav_path.display(),
        embedding.len()
    );
    Ok(embedding)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Models => {
            println!(
                "ID                 NAME                   FAMILY      RATE  FETCHED  LICENSE"
            );
            println!(
                "{PASSTHROUGH_ID:<18} {:<22} {:<9} {:>6}  {:<8} -",
                "Passthrough (bypass)", "-", 48_000, "builtin"
            );
            for model in ALL_MODELS {
                let fetched = noican_models::fetch::is_fetched(&cli.models_dir, model);
                println!(
                    "{:<18} {:<22} {:<9?} {:>6}  {:<8} {}",
                    model.id,
                    model.display_name,
                    model.family,
                    model.sample_rate,
                    if fetched { "yes" } else { "no" },
                    model.license
                );
            }
            Ok(())
        }
        Command::Fetch { ids } => {
            let explicit = !ids.is_empty();
            let targets: Vec<&ModelSpec> = if explicit {
                ids.iter()
                    .map(|id| {
                        ModelSpec::find(id).ok_or_else(|| anyhow::anyhow!("unknown model id: {id}"))
                    })
                    .collect::<Result<_, _>>()?
            } else {
                ALL_MODELS.iter().collect()
            };
            let mut failures = Vec::new();
            for model in targets {
                if let Some(note) = model.fetch_note
                    && !explicit
                {
                    println!("{}: skipped — {note}", model.id);
                    continue;
                }
                if let Err(e) = noican_models::fetch::fetch_model(&cli.models_dir, model, |line| {
                    println!("{line}");
                }) {
                    eprintln!("{}: FAILED — {e}", model.id);
                    if let Some(note) = model.fetch_note {
                        eprintln!("{}: note — {note}", model.id);
                    }
                    failures.push(model.id);
                }
            }
            if failures.is_empty() {
                Ok(())
            } else {
                anyhow::bail!("failed to fetch: {}", failures.join(", "))
            }
        }
        Command::Process {
            inputs,
            out_dir,
            models,
            enroll,
        } => {
            let options = noican_models::StageOptions {
                enrollment: enroll
                    .as_deref()
                    .map(|path| enrollment_embedding(path, &cli.models_dir))
                    .transpose()?,
            };
            let model_ids: Vec<String> = if models.is_empty() {
                std::iter::once(PASSTHROUGH_ID.to_owned())
                    .chain(
                        ModelSpec::stages()
                            .filter(|m| noican_models::fetch::is_fetched(&cli.models_dir, m))
                            .filter(|m| !m.needs_enrollment || options.enrollment.is_some())
                            .map(|m| m.id.to_owned()),
                    )
                    .collect()
            } else {
                models
            };
            println!("models: {}", model_ids.join(", "));
            for input in &inputs {
                process::process_file(
                    input,
                    &out_dir,
                    &model_ids,
                    |id| {
                        noican_models::create_stage(id, &cli.models_dir, &options)
                            .map_err(|e| anyhow::anyhow!("cannot create stage {id}: {e}"))
                    },
                    |line| println!("{line}"),
                )?;
            }
            Ok(())
        }
    }
}
