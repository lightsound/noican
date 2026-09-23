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

mod audio;
mod eval;
mod eval_run;
mod process;

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
    /// Process audio files through models; outputs are organized per input
    /// file for side-by-side comparison.
    Process {
        /// Input audio files (WAV, AIFF/AIFC, CAF, FLAC, M4A; output is WAV).
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// Output directory (default: out).
        #[arg(long, default_value = "out")]
        out_dir: PathBuf,
        /// Model ids to run (default: passthrough + all fetched models).
        #[arg(long, value_delimiter = ',')]
        models: Vec<String>,
    },
    /// Score speaker-suppression candidates on synthetic mixtures of your
    /// clean voice and an interfering speaker: high-band retention,
    /// own-voice SI-SDR, interferer residual (full band and ≥ 8 kHz),
    /// latency and block time per model and SIR. Also writes a blind
    /// listening set (docs/hush-48k-eval.md).
    Eval {
        /// Clean recording(s) of your own voice (48 kHz); several files
        /// are trimmed and concatenated in order, and the first two
        /// segments are taken from the result.
        #[arg(long, required = true, num_args = 1..)]
        target: Vec<PathBuf>,
        /// Recording(s) of the interfering speaker; several files are
        /// trimmed and concatenated in order.
        #[arg(long, required = true, num_args = 1..)]
        interferer: Vec<PathBuf>,
        /// Signal-to-interference ratios in dB for the overlap segment
        /// (voice RMS over interferer RMS).
        #[arg(
            long,
            value_delimiter = ',',
            allow_negative_numbers = true,
            default_value = "12,6,0"
        )]
        sir: Vec<f64>,
        /// Normalize the voice recording to this RMS level (dBFS) before
        /// mixing. Default: keep the recorded level. Hush's behavior
        /// depends on the absolute input level, so compare candidates at
        /// the level your microphone actually delivers (see
        /// docs/hush-48k-eval.md).
        #[arg(long, allow_negative_numbers = true)]
        target_level_dbfs: Option<f64>,
        /// Segment length in seconds; each mixture is three segments
        /// (you / you + other / other). Clamped to the material.
        #[arg(long, default_value_t = 20.0)]
        segment_seconds: f64,
        /// Model ids to score (default: passthrough + all fetched models).
        #[arg(long, value_delimiter = ',')]
        models: Vec<String>,
        /// Output directory for mixtures, outputs, metrics.csv and the
        /// blind set.
        #[arg(long, default_value = "out/eval")]
        out_dir: PathBuf,
        /// Seed for lettering the blind listening set.
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
}

/// The default model list for `process` and `eval`: the bypass plus every
/// fetched model.
fn default_model_ids(models_dir: &std::path::Path) -> Vec<String> {
    std::iter::once(PASSTHROUGH_ID.to_owned())
        .chain(
            ALL_MODELS
                .iter()
                .filter(|m| noican_models::fetch::is_fetched(models_dir, m))
                .map(|m| m.id.to_owned()),
        )
        .collect()
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Models => {
            list_models(&cli.models_dir);
            Ok(())
        }
        Command::Fetch { ids } => fetch(&cli.models_dir, &ids),
        Command::Process {
            inputs,
            out_dir,
            models,
        } => {
            let model_ids: Vec<String> = if models.is_empty() {
                default_model_ids(&cli.models_dir)
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
                        noican_models::create_stage(id, &cli.models_dir)
                            .map_err(|e| anyhow::anyhow!("cannot create stage {id}: {e}"))
                    },
                    |line| println!("{line}"),
                )?;
            }
            Ok(())
        }
        Command::Eval {
            target,
            interferer,
            sir,
            target_level_dbfs,
            segment_seconds,
            models,
            out_dir,
            seed,
        } => {
            let model_ids = if models.is_empty() {
                default_model_ids(&cli.models_dir)
            } else {
                models
            };
            println!("models: {}", model_ids.join(", "));
            eval_run::run(
                &eval_run::EvalRequest {
                    targets: target,
                    interferers: interferer,
                    sirs_db: sir,
                    target_level_dbfs,
                    segment_seconds,
                    model_ids,
                    out_dir,
                    seed,
                },
                |id| {
                    noican_models::create_stage(id, &cli.models_dir)
                        .map_err(|e| anyhow::anyhow!("cannot create stage {id}: {e}"))
                },
                |line| println!("{line}"),
            )
        }
    }
}

/// `noican models`: the registry with per-model fetch status.
fn list_models(models_dir: &std::path::Path) {
    println!("ID                 NAME                   FAMILY      RATE  FETCHED  LICENSE");
    println!(
        "{PASSTHROUGH_ID:<18} {:<22} {:<9} {:>6}  {:<8} -",
        "Passthrough (bypass)", "-", 48_000, "builtin"
    );
    for model in ALL_MODELS {
        let fetched = noican_models::fetch::is_fetched(models_dir, model);
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
}

/// `noican fetch [ids…]`: downloads weights (all models when no id is
/// given).
fn fetch(models_dir: &std::path::Path, ids: &[String]) -> anyhow::Result<()> {
    let targets: Vec<&ModelSpec> = if ids.is_empty() {
        ALL_MODELS.iter().collect()
    } else {
        ids.iter()
            .map(|id| ModelSpec::find(id).ok_or_else(|| anyhow::anyhow!("unknown model id: {id}")))
            .collect::<Result<_, _>>()?
    };
    let mut failures = Vec::new();
    for model in targets {
        if let Err(e) = noican_models::fetch::fetch_model(models_dir, model, |line| {
            println!("{line}");
        }) {
            eprintln!("{}: FAILED — {e}", model.id);
            failures.push(model.id);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("failed to fetch: {}", failures.join(", "))
    }
}
