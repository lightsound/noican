//! `noican fetch` and `noican verify` — model-weight acquisition.

use std::io::Write as _;

use anyhow::{Context as _, Result};
use noican_models::{ModelStore, Progress};

use crate::commands::{human_bytes, select};

/// Arguments for `noican fetch`.
#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Models to download. Omit to download all of them.
    pub(crate) models: Vec<String>,

    /// Suppress the progress line.
    #[arg(long)]
    pub(crate) quiet: bool,
}

/// Arguments for `noican verify`.
#[derive(Debug, clap::Args)]
pub(crate) struct VerifyArgs {
    /// Models to verify. Omit to verify all downloaded ones.
    pub(crate) models: Vec<String>,
}

/// Downloads the selected models' weights.
///
/// # Errors
///
/// Returns an error if a model identifier is unknown, a download fails, or a
/// downloaded file does not match its recorded digest.
pub(crate) fn run(args: &Args, store: &ModelStore) -> Result<()> {
    let models = select(&args.models)?;
    for model in models {
        let already = store.is_present(model);
        if !args.quiet {
            println!(
                "{} {}",
                if already { "checking" } else { "fetching " },
                model.display_name
            );
        }

        let mut last_reported = 0u64;
        store
            .fetch(model, &mut |artifact, progress: Progress| {
                if args.quiet {
                    return;
                }
                // Redraw at most every 4 MiB so a slow link does not flood the
                // terminal.
                if progress.downloaded - last_reported < 4 * 1024 * 1024
                    && Some(progress.downloaded) != progress.total
                {
                    return;
                }
                last_reported = progress.downloaded;
                let total = progress
                    .total
                    .map_or_else(|| "unknown".to_owned(), human_bytes);
                print!(
                    "\r    {} {} / {}   ",
                    artifact.file_name,
                    human_bytes(progress.downloaded),
                    total
                );
                drop(std::io::stdout().flush());
            })
            .with_context(|| format!("fetching {}", model.id))?;

        if !args.quiet {
            println!("\r    {} verified          ", model.id);
        }
    }
    Ok(())
}

/// Re-verifies checksums of already-downloaded weights.
///
/// # Errors
///
/// Returns an error if a model identifier is unknown or a file fails
/// verification. Models that were never downloaded are reported and skipped,
/// because "not downloaded" is not a failure.
pub(crate) fn verify(args: &VerifyArgs, store: &ModelStore) -> Result<()> {
    let models = select(&args.models)?;
    let mut checked = 0usize;
    let mut skipped = 0usize;

    for model in models {
        if !store.is_present(model) {
            println!("{:<18} skipped (not downloaded)", model.id);
            skipped += 1;
            continue;
        }
        store
            .verify(model)
            .with_context(|| format!("verifying {}", model.id))?;
        println!("{:<18} ok", model.id);
        checked += 1;
    }

    println!();
    println!("{checked} verified, {skipped} not downloaded");
    Ok(())
}
