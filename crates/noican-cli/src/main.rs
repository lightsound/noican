//! Command-line entry point for noican.

mod batch;
mod wav;

use std::{path::PathBuf, process::ExitCode};

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use noican_models::{
    assets::{FetchOptions, ModelAsset, ModelStore},
    ModelId,
};

#[derive(Debug, Parser)]
#[command(name = "noican", version, about = "On-device audio enhancement")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Process WAV files through one or more models.
    Process(ProcessArgs),
    /// Inspect or fetch model files.
    Models(ModelsArgs),
}

/// WAV comparison options.
#[derive(Debug, Args)]
struct ProcessArgs {
    /// Input WAV files. Each file gets its own comparison directory.
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// Model slug. Repeat the flag or separate values with commas.
    #[arg(long = "model", value_delimiter = ',')]
    models: Vec<ModelId>,
    /// Process every catalog model. This is the default when --model is absent.
    #[arg(long, conflicts_with = "models")]
    all_models: bool,
    /// Root directory for stable model-specific outputs.
    #[arg(long, default_value = "output")]
    output_dir: PathBuf,
    /// Override the platform model cache.
    #[arg(long)]
    model_dir: Option<PathBuf>,
    /// Voice enrollment WAV used to compute the TSE ECAPA embedding.
    #[arg(long)]
    enrollment_wav: Option<PathBuf>,
    /// JSON file containing exactly 192 ECAPA float values.
    #[arg(long, conflicts_with = "enrollment_wav")]
    embedding_json: Option<PathBuf>,
    /// Keep live-path latency as leading silence instead of aligning outputs.
    #[arg(long)]
    preserve_delay: bool,
}

/// Model management options.
#[derive(Debug, Args)]
struct ModelsArgs {
    #[command(subcommand)]
    command: ModelsCommand,
}

#[derive(Debug, Subcommand)]
enum ModelsCommand {
    /// List model slugs and runtime requirements.
    List,
    /// Download and checksum model assets.
    Fetch(FetchArgs),
}

#[derive(Debug, Args)]
struct FetchArgs {
    /// Model slug. Repeat the flag or separate values with commas.
    #[arg(long = "model", value_delimiter = ',')]
    models: Vec<ModelId>,
    /// Fetch every catalog model.
    #[arg(long, conflicts_with = "models")]
    all_models: bool,
    /// Also fetch the ECAPA enrollment graph and filterbank.
    #[arg(long)]
    include_enrollment: bool,
    /// Override the platform model cache.
    #[arg(long)]
    model_dir: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Process(args) => batch::run(&args),
        Command::Models(ModelsArgs {
            command: ModelsCommand::List,
        }) => {
            list_models();
            Ok(())
        }
        Command::Models(ModelsArgs {
            command: ModelsCommand::Fetch(args),
        }) => fetch_models(args),
    }
}

fn list_models() {
    println!("SLUG\tRATE\tENROLLMENT\tNAME");
    for model in ModelId::ALL {
        let enrollment = if model.requires_enrollment() {
            "ECAPA-192"
        } else {
            "none"
        };
        println!(
            "{}\t{}\t{}\t{}",
            model.slug(),
            model.native_sample_rate(),
            enrollment,
            model.display_name()
        );
    }
}

fn fetch_models(args: FetchArgs) -> Result<()> {
    let store = model_store(args.model_dir)?;
    let models = selected_models(&args.models, args.all_models);
    let token = std::env::var("NOICAN_HF_TOKEN").ok();
    let options = FetchOptions {
        hugging_face_token: token.as_deref(),
    };
    let mut assets: Vec<ModelAsset> = models
        .iter()
        .flat_map(|model| model.assets().iter().copied())
        .collect();
    if args.include_enrollment {
        assets.extend([ModelAsset::Ecapa, ModelAsset::EcapaFilterbank]);
    }
    assets.sort_by_key(|asset| asset.specification().relative_path);
    assets.dedup();
    for asset in assets {
        let path = store.ensure(asset, options)?;
        println!(
            "{}\t{}",
            asset.specification().relative_path,
            path.display()
        );
    }
    Ok(())
}

fn selected_models(explicit: &[ModelId], _all: bool) -> Vec<ModelId> {
    if explicit.is_empty() {
        ModelId::ALL.to_vec()
    } else {
        explicit.to_vec()
    }
}

fn model_store(explicit: Option<PathBuf>) -> Result<ModelStore> {
    explicit.map_or_else(
        || ModelStore::platform_default().map_err(Into::into),
        |path| Ok(ModelStore::new(path)),
    )
}
