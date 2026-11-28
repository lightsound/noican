//! See docs/adr/0010-eval-methodology.md#cli-file-mode-model-comparison.

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
use noican_models::fetch::{FetchEvent, FetchStep};
use noican_models::{ALL_MODELS, ModelSpec, PASSTHROUGH_ID};

#[derive(Parser)]
#[command(name = "noican", version, about = "noican audio engine CLI")]
struct Cli {
    #[arg(long, global = true, default_value = "models")]
    models_dir: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Models,
    Fetch {
        ids: Vec<String>,
    },
    Process {
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        #[arg(long, default_value = "out")]
        out_dir: PathBuf,
        #[arg(long, value_delimiter = ',')]
        models: Vec<String>,
    },
    Eval {
        #[arg(long, required = true, num_args = 1..)]
        target: Vec<PathBuf>,
        #[arg(long, required = true, num_args = 1..)]
        interferer: Vec<PathBuf>,
        #[arg(
            long,
            value_delimiter = ',',
            allow_negative_numbers = true,
            default_value = "12,6,0"
        )]
        sir: Vec<f64>,
        #[arg(long, allow_negative_numbers = true)]
        target_level_dbfs: Option<f64>,
        #[arg(long, default_value_t = 20.0)]
        segment_seconds: f64,
        #[arg(long, value_delimiter = ',')]
        models: Vec<String>,
        #[arg(long, default_value = "out/eval")]
        out_dir: PathBuf,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
}

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
        if let Err(e) = noican_models::fetch::fetch_model(models_dir, model, |event| {
            let FetchEvent { model, file, step } = event;
            match step {
                FetchStep::Present => println!("{model}/{file}: already present"),
                FetchStep::Repairing => {
                    println!("{model}/{file}: checksum mismatch, re-downloading");
                }
                FetchStep::Started => println!("{model}/{file}: downloading…"),
                FetchStep::Received(..) => {}
                FetchStep::Finished(bytes) => println!("{model}/{file}: done ({bytes} bytes)"),
            }
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
