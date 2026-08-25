//! `noican` — command-line front-end for model acquisition and offline
//! comparison.
//!
//! The offline mode exists because judging a denoiser by ear is the only
//! judgement that counts, and a fair comparison needs identical input, an
//! identical engine, and aligned output. All three are what this binary
//! provides; see `docs/tech-research.md` §12.

// In a binary crate `unreachable_pub` and `clippy::redundant_pub_crate` demand
// opposite things: nothing here can be reached from outside, so `pub` is always
// unreachable, and `pub(crate)` is always redundant. `unreachable_pub` is the
// one worth keeping, because it is what stops a helper from silently becoming
// API if any of this later moves into a library crate.
#![expect(
    clippy::redundant_pub_crate,
    reason = "resolves a direct conflict with unreachable_pub; see above"
)]

mod commands;
mod offline;
mod wav;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Command-line interface definition.
#[derive(Debug, Parser)]
#[command(
    name = "noican",
    version,
    about = "Noise-cancelling virtual microphone: model management and offline comparison",
    long_about = None
)]
struct Cli {
    /// Directory holding downloaded model weights.
    ///
    /// Defaults to `$NOICAN_MODEL_DIR`, or `models` under the current
    /// directory.
    #[arg(long, global = true)]
    model_dir: Option<std::path::PathBuf>,

    /// Increase log verbosity. Repeat for more.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

/// The available subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// List every model in the catalog and whether its weights are present.
    List(commands::list::Args),

    /// Download model weights and verify their checksums.
    Fetch(commands::fetch::Args),

    /// Re-verify the checksums of already-downloaded weights.
    Verify(commands::fetch::VerifyArgs),

    /// Process a WAV file through one or more models.
    Process(commands::process::Args),

    /// Measure each model's algorithmic delay from a signal.
    Latency(commands::latency::Args),

    /// Write the synthetic probe signal to a WAV file, for smoke tests.
    Probe(commands::probe::Args),

    /// Enrol a speaker so the speaker gate knows whose voice to keep.
    Enroll(commands::enroll::Args),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let store = cli
        .model_dir
        .clone()
        .map_or_else(noican_models::ModelStore::from_environment, |root| {
            noican_models::ModelStore::new(root)
        });

    match cli.command {
        Command::List(args) => {
            commands::list::run(&args, &store);
            Ok(())
        }
        Command::Fetch(args) => commands::fetch::run(&args, &store),
        Command::Verify(args) => commands::fetch::verify(&args, &store),
        Command::Process(args) => commands::process::run(&args, &store),
        Command::Latency(args) => commands::latency::run(&args, &store),
        Command::Probe(args) => commands::probe::run(&args),
        Command::Enroll(args) => commands::enroll::run(&args, &store),
    }
}

/// Configures logging: warnings by default, more when asked.
fn init_tracing(verbosity: u8) {
    let directive = match verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(directive));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}
