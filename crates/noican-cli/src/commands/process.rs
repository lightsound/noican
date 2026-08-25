//! `noican process` — run a WAV file through one or more models.
//!
//! The output layout is built for A/B listening: one directory per input file,
//! the unprocessed reference alongside the results, every file at the same rate
//! and the same length, and a manifest recording what was measured. Without
//! that discipline a "comparison" ends up comparing loudness and time offsets
//! rather than the models.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use noican_models::ModelStore;

use crate::commands::select;
use crate::offline::{self, Alignment};
use crate::wav::{self, Clip};

/// Name of the unprocessed reference file written next to the results.
const REFERENCE_NAME: &str = "00-reference-unprocessed.wav";

/// Name of the manifest describing a comparison run.
const MANIFEST_NAME: &str = "manifest.md";

/// Arguments for `noican process`.
#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Input WAV file.
    pub(crate) input: PathBuf,

    /// Directory to write results into. A subdirectory is created per input.
    #[arg(short, long, default_value = "out")]
    pub(crate) output: PathBuf,

    /// Models to run. Omit to run every model in the catalog.
    #[arg(short, long)]
    pub(crate) models: Vec<String>,

    /// How to align each output with the input.
    #[arg(long, value_enum, default_value_t = Alignment::Measured)]
    pub(crate) align: Alignment,

    /// Host sample rate for the comparison. Every output is written at this
    /// rate regardless of the model's native rate.
    #[arg(long, default_value_t = noican_core::HOST_SAMPLE_RATE)]
    pub(crate) rate: u32,

    /// Keep going when one model fails, instead of stopping.
    #[arg(long)]
    pub(crate) keep_going: bool,
}

/// One row of the comparison manifest.
struct Row {
    id: &'static str,
    display_name: &'static str,
    native_rate: u32,
    file_name: String,
    reported_delay_ms: f64,
    measured_delay_ms: Option<f64>,
    model_delay_native: Option<usize>,
    speed_factor: f64,
    peak: f32,
    rms_dbfs: Option<f32>,
    underruns: u64,
    aligned_by: usize,
}

/// Runs the selected models over the input file.
///
/// # Errors
///
/// Returns an error if the input cannot be read, a model identifier is unknown,
/// weights are missing, or (unless `--keep-going`) any model fails.
pub(crate) fn run(args: &Args, store: &ModelStore) -> Result<()> {
    let models = select(&args.models)?;
    let source = wav::read(&args.input)?;
    if source.samples.is_empty() {
        bail!("{} contains no samples", args.input.display());
    }

    let reference = offline::resample(&source, args.rate)?;
    let stem = args.input.file_stem().map_or_else(
        || "input".to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    );
    let directory = args.output.join(&stem);

    println!(
        "input   {} ({:.2} s, {} Hz -> {} Hz, peak {:.3})",
        args.input.display(),
        source.duration_seconds(),
        source.sample_rate,
        args.rate,
        source.peak()
    );
    println!("output  {}", directory.display());
    println!();

    wav::write(&directory.join(REFERENCE_NAME), &reference)?;

    let mut rows = Vec::new();
    let mut failures = Vec::new();

    for model in models {
        match process_one(model, store, &reference, args, &directory) {
            Ok(row) => {
                println!(
                    "  {:<18} {:>7.1}x realtime   delay {:>6.1} ms   peak {:.3}",
                    row.id,
                    row.speed_factor,
                    row.measured_delay_ms.unwrap_or(row.reported_delay_ms),
                    row.peak
                );
                rows.push(row);
            }
            Err(error) => {
                println!("  {:<18} FAILED: {error:#}", model.id);
                failures.push((model.id, error));
                if !args.keep_going {
                    break;
                }
            }
        }
    }

    let manifest = render_manifest(&args.input, &reference, &rows);
    let manifest_path = directory.join(MANIFEST_NAME);
    std::fs::write(&manifest_path, manifest)
        .with_context(|| format!("writing {}", manifest_path.display()))?;

    println!();
    println!(
        "{} model{} written to {}",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" },
        directory.display()
    );

    if let Some((id, error)) = failures.into_iter().next() {
        return Err(error).with_context(|| format!("{id} failed"));
    }
    Ok(())
}

/// Runs one model and writes its output file.
fn process_one(
    model: &'static noican_models::ModelDescriptor,
    store: &ModelStore,
    reference: &Clip,
    args: &Args,
    directory: &Path,
) -> Result<Row> {
    let stage = noican_models::build_stage(model, store)
        .with_context(|| format!("loading {}", model.id))?;
    let outcome = offline::run(stage, reference, args.rate, args.align)?;

    let file_name = format!("{}.wav", model.id);
    wav::write(&directory.join(&file_name), &outcome.clip)?;

    #[expect(
        clippy::cast_precision_loss,
        reason = "sample counts in a comparison clip are far below f64's exact-integer limit"
    )]
    let to_ms = |samples: usize| samples as f64 * 1_000.0 / f64::from(args.rate);

    Ok(Row {
        id: model.id,
        display_name: model.display_name,
        native_rate: model.sample_rate,
        file_name,
        reported_delay_ms: to_ms(outcome.reported_delay),
        measured_delay_ms: outcome.measured_delay.map(to_ms),
        model_delay_native: outcome.model_delay_native,
        speed_factor: outcome.speed_factor,
        peak: outcome.clip.peak(),
        rms_dbfs: outcome.clip.rms_dbfs(),
        underruns: outcome.underruns,
        aligned_by: outcome.trimmed,
    })
}

/// Renders the comparison manifest.
fn render_manifest(input: &Path, reference: &Clip, rows: &[Row]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# noican comparison");
    let _ = writeln!(out);
    let _ = writeln!(out, "- Input: `{}`", input.display());
    let _ = writeln!(
        out,
        "- Duration: {:.2} s at {} Hz",
        reference.duration_seconds(),
        reference.sample_rate
    );
    let _ = writeln!(
        out,
        "- Reference (unprocessed, same rate and length): `{REFERENCE_NAME}`"
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Every output is aligned to the reference, so switching between files compares the \
         models rather than a time offset. `Speed` is how many times faster than real time the \
         model ran on this machine; anything below 1.0 cannot keep up live."
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "| File | Model | Native rate | Measured delay | Model delay | Aligned by | Speed | \
         Peak | RMS |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|");

    for row in rows {
        let measured = row
            .measured_delay_ms
            .map_or_else(|| "not measurable".to_owned(), |ms| format!("{ms:.1} ms"));
        let model_delay = row.model_delay_native.map_or_else(
            || "unknown".to_owned(),
            |samples| format!("{samples} samples @ {} Hz", row.native_rate),
        );
        let rms = row
            .rms_dbfs
            .map_or_else(|| "silent".to_owned(), |db| format!("{db:.1} dBFS"));
        let _ = writeln!(
            out,
            "| `{}` | {} | {} kHz | {} | {} | {} samples | {:.1}x | {:.3} | {} |",
            row.file_name,
            row.display_name,
            row.native_rate / 1_000,
            measured,
            model_delay,
            row.aligned_by,
            row.speed_factor,
            row.peak,
            rms
        );
    }

    let glitches: u64 = rows.iter().map(|row| row.underruns).sum();
    let _ = writeln!(out);
    if glitches == 0 {
        let _ = writeln!(
            out,
            "No output underruns: every host block produced a full host block."
        );
    } else {
        let _ = writeln!(
            out,
            "**{glitches} output underruns.** The runner had to emit silence, which means its \
             priming estimate is too small for one of these rate ratios."
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{REFERENCE_NAME, Row, render_manifest};
    use crate::wav::Clip;

    fn row(underruns: u64) -> Row {
        Row {
            id: "fastenhancer-t",
            display_name: "FastEnhancer T (48 kHz)",
            native_rate: 48_000,
            file_name: "fastenhancer-t.wav".to_owned(),
            reported_delay_ms: 20.0,
            measured_delay_ms: Some(21.3),
            model_delay_native: Some(512),
            speed_factor: 32.5,
            peak: 0.812,
            rms_dbfs: Some(-18.4),
            underruns,
            aligned_by: 1_024,
        }
    }

    #[test]
    fn manifest_records_the_reference_and_each_row() {
        let reference = Clip {
            samples: vec![0.0; 48_000],
            sample_rate: 48_000,
        };
        let manifest = render_manifest(
            std::path::Path::new("/tmp/sample.wav"),
            &reference,
            &[row(0)],
        );
        assert!(manifest.contains(REFERENCE_NAME));
        assert!(manifest.contains("fastenhancer-t.wav"));
        assert!(manifest.contains("512 samples @ 48000 Hz"));
        assert!(manifest.contains("32.5x"));
        assert!(manifest.contains("No output underruns"));
    }

    #[test]
    fn manifest_calls_out_underruns() {
        let reference = Clip {
            samples: vec![0.0; 48_000],
            sample_rate: 48_000,
        };
        let manifest = render_manifest(std::path::Path::new("in.wav"), &reference, &[row(3)]);
        assert!(manifest.contains("**3 output underruns.**"), "{manifest}");
    }

    #[test]
    fn manifest_admits_when_a_delay_could_not_be_measured() {
        let reference = Clip {
            samples: vec![0.0; 4_800],
            sample_rate: 48_000,
        };
        let mut unmeasured = row(0);
        unmeasured.measured_delay_ms = None;
        unmeasured.model_delay_native = None;
        let manifest = render_manifest(std::path::Path::new("in.wav"), &reference, &[unmeasured]);
        assert!(manifest.contains("not measurable"));
        assert!(manifest.contains("unknown"));
    }
}
