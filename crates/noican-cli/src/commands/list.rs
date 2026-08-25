//! `noican list` — show the catalog and what is downloaded.

use noican_models::ModelStore;

/// Arguments for `noican list`.
#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Show licence, source, and notes for each model.
    #[arg(long)]
    pub(crate) detail: bool,
}

/// Prints the catalog.
pub(crate) fn run(args: &Args, store: &ModelStore) {
    println!("model weights directory: {}", store.root().display());
    println!();
    println!(
        "{:<18} {:<28} {:>6}  {:<20} WEIGHTS",
        "ID", "NAME", "RATE", "KIND"
    );

    for model in noican_models::CATALOG {
        let state = if store.is_present(model) {
            "present"
        } else {
            "not downloaded"
        };
        println!(
            "{:<18} {:<28} {:>6}  {:<20} {}",
            model.id,
            model.display_name,
            format!("{} k", model.sample_rate / 1_000),
            model.kind.label(),
            state
        );
        if args.detail {
            println!("    licence: {}", model.license);
            println!("    source:  {}", model.source);
            println!("    notes:   {}", model.notes);
            for artifact in model.artifacts {
                println!("    file:    {}", artifact.file_name);
            }
            println!();
        }
    }

    if !args.detail {
        println!();
        println!("Pass --detail for licence, source, and notes.");
    }
}
