//! noican CLI: model weight management and batch WAV processing.
//!
//! The same engine stages used by the real-time pipeline are driven here in
//! file mode, giving strictly identical conditions for model comparison
//! (docs/tech-research.md §12, Phase 0).

#![allow(
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
    },
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
            let targets: Vec<&ModelSpec> = if ids.is_empty() {
                ALL_MODELS.iter().collect()
            } else {
                ids.iter()
                    .map(|id| {
                        ModelSpec::find(id).ok_or_else(|| anyhow::anyhow!("unknown model id: {id}"))
                    })
                    .collect::<Result<_, _>>()?
            };
            for model in targets {
                noican_models::fetch::fetch_model(&cli.models_dir, model, |line| {
                    println!("{line}");
                })?;
            }
            Ok(())
        }
        Command::Process {
            inputs,
            out_dir,
            models,
        } => {
            let model_ids: Vec<String> = if models.is_empty() {
                std::iter::once(PASSTHROUGH_ID.to_owned())
                    .chain(
                        ALL_MODELS
                            .iter()
                            .filter(|m| noican_models::fetch::is_fetched(&cli.models_dir, m))
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
                        noican_models::create_stage(id, &cli.models_dir)
                            .map_err(|e| anyhow::anyhow!("cannot create stage {id}: {e}"))
                    },
                    |line| println!("{line}"),
                )?;
            }
            Ok(())
        }
    }
}
