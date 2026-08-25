//! `noican latency` — measure each model's algorithmic delay.
//!
//! No published export declares its delay, so it has to be measured. The
//! measurement is a cross-correlation between a probe signal and what the model
//! returns, with the runner's own contribution (resampler group delays and
//! priming) subtracted so that what is left is attributable to the model.
//!
//! The probe is synthetic by default: harmonics under an amplitude envelope,
//! which a speech enhancer passes through largely intact and which correlates
//! sharply. A real recording can be supplied instead when a model turns out to
//! suppress the synthetic probe.

use anyhow::{Context as _, Result};
use noican_models::ModelStore;

use crate::commands::select;
use crate::offline::{self, Alignment};
use crate::wav::{self, Clip};

/// Length of the synthetic probe, in seconds.
const PROBE_SECONDS: u32 = 6;

/// Arguments for `noican latency`.
#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Models to measure. Omit to measure all of them.
    pub(crate) models: Vec<String>,

    /// Use this WAV file as the probe instead of the synthetic one.
    #[arg(long)]
    pub(crate) probe: Option<std::path::PathBuf>,

    /// Host sample rate for the measurement.
    #[arg(long, default_value_t = noican_core::HOST_SAMPLE_RATE)]
    pub(crate) rate: u32,
}

/// Measures and prints each model's delay.
///
/// # Errors
///
/// Returns an error if a model identifier is unknown, weights are missing, or a
/// probe file cannot be read.
pub(crate) fn run(args: &Args, store: &ModelStore) -> Result<()> {
    let models = select(&args.models)?;
    let probe = match &args.probe {
        Some(path) => offline::resample(&wav::read(path)?, args.rate)?,
        None => synthetic_probe(args.rate, PROBE_SECONDS),
    };

    println!(
        "probe: {:.1} s at {} Hz",
        probe.duration_seconds(),
        args.rate
    );
    println!();
    println!(
        "{:<18} {:>10} {:>12} {:>14}",
        "MODEL", "NATIVE", "MODEL DELAY", "END TO END"
    );

    for model in models {
        let stage = noican_models::build_stage(model, store)
            .with_context(|| format!("loading {}", model.id))?;
        let outcome = offline::run(stage, &probe, args.rate, Alignment::None)?;

        let native = outcome.model_delay_native.map_or_else(
            || "unmeasurable".to_owned(),
            |samples| format!("{samples} smp"),
        );
        #[expect(
            clippy::cast_precision_loss,
            reason = "delays are a few thousand samples at most"
        )]
        let end_to_end = outcome.measured_delay.map_or_else(
            || "unmeasurable".to_owned(),
            |samples| format!("{:.1} ms", samples as f64 * 1_000.0 / f64::from(args.rate)),
        );

        println!(
            "{:<18} {:>10} {:>12} {:>14}",
            model.id,
            format!("{} Hz", model.sample_rate),
            native,
            end_to_end
        );
    }

    println!();
    println!(
        "`MODEL DELAY` is the figure to record in noican-models' latency table: the measured \
         end-to-end delay minus the runner's own resampling and priming overhead, expressed at \
         the model's native rate."
    );
    Ok(())
}

/// Harmonics of the synthetic probe, as `(gain, frequency in hertz)`.
const HARMONICS: [(f32, f32); 3] = [(1.0, 180.0), (0.6, 420.0), (0.3, 900.0)];

/// Rate of the probe's amplitude envelope, in hertz.
const ENVELOPE_HZ: f32 = 2.5;

/// Builds the synthetic probe: three harmonics under a slow envelope.
pub(crate) fn synthetic_probe(rate: u32, seconds: u32) -> Clip {
    let length = (rate * seconds) as usize;
    let samples = (0..length)
        .map(|index| {
            #[expect(clippy::cast_precision_loss, reason = "probe index is bounded")]
            let t = index as f32 / rate as f32;
            let envelope = 0.5f32.mul_add(sine(ENVELOPE_HZ, t), 0.5);
            let tone: f32 = HARMONICS
                .iter()
                .map(|&(gain, frequency)| gain * sine(frequency, t))
                .sum();
            0.25 * envelope * tone
        })
        .collect();
    Clip {
        samples,
        sample_rate: rate,
    }
}

/// One sample of a unit sine at `frequency`, at time `t` seconds.
fn sine(frequency: f32, t: f32) -> f32 {
    (2.0 * core::f32::consts::PI * frequency * t).sin()
}

#[cfg(test)]
mod tests {
    use super::synthetic_probe;

    #[test]
    fn probe_is_long_and_loud_enough_to_correlate() {
        let probe = synthetic_probe(48_000, 6);
        assert_eq!(probe.samples.len(), 48_000 * 6);
        assert!(probe.peak() > 0.1, "probe peak is only {}", probe.peak());
        assert!(probe.peak() < 1.0, "probe clips at {}", probe.peak());
    }
}
