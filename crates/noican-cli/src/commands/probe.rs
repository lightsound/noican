//! `noican probe` — write the synthetic probe signal to a WAV file.
//!
//! Exists so that smoke tests and CI have an input without a binary checked
//! into the repository. It is a synthetic signal, so it is good for confirming
//! that the pipeline runs; it is not a substitute for a real recording when
//! judging quality or measuring delay (see `docs/models.md`).

use anyhow::Result;

use crate::wav;

/// Arguments for `noican probe`.
#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Where to write the WAV file.
    pub(crate) output: std::path::PathBuf,

    /// Sample rate in hertz.
    #[arg(long, default_value_t = noican_core::HOST_SAMPLE_RATE)]
    pub(crate) rate: u32,

    /// Length in seconds.
    #[arg(long, default_value_t = 4)]
    pub(crate) seconds: u32,
}

/// Writes the probe.
///
/// # Errors
///
/// Returns an error if the file cannot be written.
pub(crate) fn run(args: &Args) -> Result<()> {
    let clip = super::latency::synthetic_probe(args.rate, args.seconds);
    wav::write(&args.output, &clip)?;
    println!(
        "wrote {} ({:.1} s at {} Hz, peak {:.3})",
        args.output.display(),
        clip.duration_seconds(),
        clip.sample_rate,
        clip.peak()
    );
    Ok(())
}
