//! Offline processing of a clip through a stage.
//!
//! This is the same [`noican_core::StageRunner`] the live engine uses, driven
//! from a file instead of a device. Running one engine for both paths is the
//! point: a comparison made here is a comparison of what will actually be
//! heard, not of a separate offline code path that might differ.

use std::time::Instant;

use anyhow::{Context as _, Result};
use noican_core::{RationalResampler, Stage, StageRunner};

use crate::wav::Clip;

/// Host block size used for offline runs.
///
/// Deliberately in the same range as a real device buffer (128–256 frames per
/// `docs/tech-research.md` §4.1) so that the block-boundary behaviour exercised
/// offline matches the live path.
pub(crate) const OFFLINE_BLOCK: usize = 256;

/// How the output should be aligned with the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Alignment {
    /// Measure the delay from the signal itself, by cross-correlation.
    ///
    /// The default, because it is exact for the clip at hand and does not
    /// depend on any recorded figure being right.
    Measured,
    /// Trim the delay the runner reports.
    Reported,
    /// Leave the output as produced.
    None,
}

/// Result of running one stage over one clip.
#[derive(Debug)]
pub(crate) struct Outcome {
    /// The processed audio, aligned according to the requested strategy.
    pub(crate) clip: Clip,
    /// Delay the runner reported before processing.
    pub(crate) reported_delay: usize,
    /// Delay measured from the signal, when it could be determined.
    pub(crate) measured_delay: Option<usize>,
    /// Samples trimmed from the head to align the output.
    pub(crate) trimmed: usize,
    /// Times faster than real time, so 20.0 means a 1 s clip took 50 ms.
    pub(crate) speed_factor: f64,
    /// How often the runner's output queue ran dry. Should be zero.
    pub(crate) underruns: u64,
    /// Delay attributable to the model itself, in samples at its native rate.
    ///
    /// Derived by subtracting the runner's own overhead from the measured
    /// end-to-end delay. This is what the latency table records.
    pub(crate) model_delay_native: Option<usize>,
}

/// Runs `stage` over `clip` at `host_rate`.
///
/// The clip is padded with enough silence to flush the pipeline, so the tail of
/// the input is not lost to the model's own delay.
///
/// # Errors
///
/// Returns an error if the runner cannot be built or the stage fails.
pub(crate) fn run(
    stage: Box<dyn Stage>,
    clip: &Clip,
    host_rate: u32,
    alignment: Alignment,
) -> Result<Outcome> {
    let stage_spec = stage.spec();
    let mut runner =
        StageRunner::new(stage, host_rate, OFFLINE_BLOCK).context("building the stage runner")?;
    let reported_delay = runner.latency_samples();
    let overhead = runner.overhead_latency_samples();

    // Enough trailing silence to push everything through: the reported delay
    // plus a block, plus one stage block expressed at the host rate.
    let flush = reported_delay
        + OFFLINE_BLOCK
        + scale(stage_spec.block_size, stage_spec.sample_rate, host_rate);
    let total = clip.samples.len() + flush;

    let mut produced = Vec::with_capacity(total);
    let mut block_in = vec![0.0f32; OFFLINE_BLOCK];
    let mut block_out = vec![0.0f32; OFFLINE_BLOCK];

    let started = Instant::now();
    let mut offset = 0;
    while offset < total {
        let take = OFFLINE_BLOCK.min(total - offset);
        let input = &mut block_in[..take];
        input.fill(0.0);
        if offset < clip.samples.len() {
            let end = (offset + take).min(clip.samples.len());
            input[..end - offset].copy_from_slice(&clip.samples[offset..end]);
        }

        let output = &mut block_out[..take];
        runner.process(input, output).context("running the stage")?;
        produced.extend_from_slice(output);
        offset += take;
    }
    let elapsed = started.elapsed().as_secs_f64();

    // Search past the delay the runner reports, plus a margin: the reported
    // figure already accounts for priming and resampling, so the truth is near
    // it, and a fixed ceiling cannot reach a block stage at all.
    let search_limit = reported_delay + host_rate as usize / 4;
    let measured_delay = measure_delay(&clip.samples, &produced, host_rate, search_limit);
    let trimmed = match alignment {
        Alignment::Measured => measured_delay.unwrap_or(reported_delay),
        Alignment::Reported => reported_delay,
        Alignment::None => 0,
    };

    let mut samples = produced;
    if trimmed > 0 {
        samples.drain(..trimmed.min(samples.len()));
    }
    samples.truncate(clip.samples.len());
    samples.resize(clip.samples.len(), 0.0);

    let model_delay_native = measured_delay.map(|measured| {
        let attributable = measured.saturating_sub(overhead);
        scale(attributable, host_rate, stage_spec.sample_rate)
    });

    let speed_factor = if elapsed > 0.0 {
        clip.duration_seconds() / elapsed
    } else {
        f64::INFINITY
    };

    Ok(Outcome {
        clip: Clip {
            samples,
            sample_rate: host_rate,
        },
        reported_delay,
        measured_delay,
        trimmed,
        speed_factor,
        underruns: runner.underruns(),
        model_delay_native,
    })
}

/// Resamples `clip` to `target_rate`, or returns it unchanged if it already is.
///
/// # Errors
///
/// Returns an error if the rate ratio cannot be represented.
pub(crate) fn resample(clip: &Clip, target_rate: u32) -> Result<Clip> {
    if clip.sample_rate == target_rate {
        return Ok(clip.clone());
    }
    let mut resampler = RationalResampler::new(clip.sample_rate, target_rate)
        .with_context(|| format!("resampling {} Hz to {target_rate} Hz", clip.sample_rate))?;
    let mut samples = vec![0.0; resampler.max_output_len(clip.samples.len())];
    let written = resampler
        .process(&clip.samples, &mut samples)
        .context("resampling")?;
    samples.truncate(written);
    Ok(Clip {
        samples,
        sample_rate: target_rate,
    })
}

/// Finds the lag at which `processed` best matches `reference`.
///
/// Uses a window from the middle of the clip so that neither the model's
/// warm-up nor the flush tail dominates. Returns `None` when the clip is too
/// short to measure or the correlation is too weak to trust — an honest
/// "unknown" rather than a plausible wrong number.
///
/// `max_lag` is a parameter rather than a constant because a block stage's
/// delay is its whole block: the eight-second blocks the `DeepFilterNet` family
/// needs are far past any plausible fixed ceiling, and a ceiling that cannot
/// reach the answer reports "unmeasurable" for a model that is working fine.
#[must_use]
pub(crate) fn measure_delay(
    reference: &[f32],
    processed: &[f32],
    sample_rate: u32,
    max_lag: usize,
) -> Option<usize> {
    let window = (sample_rate as usize * 2).min(reference.len() / 2);
    if window < 1_024 || processed.len() < window + max_lag {
        return None;
    }

    let start = (reference.len() / 2).saturating_sub(window / 2);
    let start = start.min(reference.len().saturating_sub(window));
    let reference = &reference[start..start + window];

    let reference_energy: f64 = reference
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum();
    if reference_energy <= 0.0 {
        return None;
    }

    let mut best_lag = 0usize;
    let mut best_score = f64::NEG_INFINITY;
    for lag in 0..=max_lag {
        let from = start + lag;
        if from + window > processed.len() {
            break;
        }
        let candidate = &processed[from..from + window];
        let mut dot = 0.0f64;
        let mut energy = 0.0f64;
        for (a, b) in reference.iter().zip(candidate) {
            dot = f64::from(*a).mul_add(f64::from(*b), dot);
            energy = f64::from(*b).mul_add(f64::from(*b), energy);
        }
        if energy <= 0.0 {
            continue;
        }
        let score = dot / (reference_energy * energy).sqrt();
        if score > best_score {
            best_score = score;
            best_lag = lag;
        }
    }

    // A denoiser changes the signal, so perfect correlation is not expected;
    // but below this the peak is not distinguishable from noise.
    if best_score < 0.2 {
        None
    } else {
        Some(best_lag)
    }
}

/// Converts a sample count between two rates, rounding to nearest.
fn scale(samples: usize, from_rate: u32, to_rate: u32) -> usize {
    if from_rate == to_rate {
        return samples;
    }
    let Ok(samples) = u64::try_from(samples) else {
        return usize::MAX;
    };
    let numerator = samples
        .saturating_mul(u64::from(to_rate))
        .saturating_add(u64::from(from_rate) / 2);
    usize::try_from(numerator / u64::from(from_rate)).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::{Alignment, measure_delay, resample, run, scale};
    use crate::wav::Clip;
    use noican_core::stage::Passthrough;

    /// The same probe `noican latency` uses: harmonics under a slow envelope,
    /// which cross-correlates to a sharp peak.
    fn speech_like(rate: u32, seconds: u32) -> Vec<f32> {
        crate::commands::latency::synthetic_probe(rate, seconds).samples
    }

    #[test]
    fn scale_rounds_to_nearest() {
        assert_eq!(scale(480, 48_000, 16_000), 160);
        assert_eq!(scale(160, 16_000, 48_000), 480);
        assert_eq!(scale(7, 48_000, 48_000), 7);
    }

    /// A large delay is found only when the search limit reaches it, which is
    /// why the limit is derived from the runner's own figure rather than fixed.
    /// Below the true delay the answer is wrong rather than absent, because a
    /// quasi-periodic signal correlates with itself at shorter lags — so the
    /// caller has to supply a range that contains the truth.
    #[test]
    fn a_large_delay_needs_a_search_limit_that_reaches_it() {
        let rate = 48_000;
        let reference = speech_like(rate, 5);
        let delay = 30_000;
        let mut processed = vec![0.0; delay];
        processed.extend_from_slice(&reference);

        assert_eq!(
            measure_delay(&reference, &processed, rate, delay + 100),
            Some(delay)
        );
        let short = measure_delay(&reference, &processed, rate, 1_000);
        assert!(
            short.is_none_or(|lag| lag <= 1_000),
            "the reported lag escaped the search limit: {short:?}"
        );
    }

    #[test]
    fn measures_an_injected_delay() {
        let rate = 48_000;
        let reference = speech_like(rate, 5);
        let delay = 1_234;
        let mut processed = vec![0.0; delay];
        processed.extend_from_slice(&reference);
        assert_eq!(
            measure_delay(&reference, &processed, rate, 4_000),
            Some(delay)
        );
    }

    #[test]
    fn refuses_to_guess_on_silence_or_short_clips() {
        let rate = 48_000;
        assert!(measure_delay(&[0.0; 100], &[0.0; 100], rate, 4_000).is_none());
        let silence = vec![0.0f32; rate as usize * 5];
        assert!(measure_delay(&silence, &silence, rate, 4_000).is_none());
    }

    #[test]
    fn passthrough_output_matches_the_input_after_alignment() {
        let rate = 48_000;
        let clip = Clip {
            samples: speech_like(rate, 4),
            sample_rate: rate,
        };
        let outcome = run(
            Box::new(Passthrough::new(rate, 480)),
            &clip,
            rate,
            Alignment::Measured,
        )
        .unwrap();

        assert_eq!(outcome.underruns, 0);
        assert_eq!(outcome.clip.samples.len(), clip.samples.len());
        assert_eq!(outcome.measured_delay, Some(outcome.reported_delay));
        assert_eq!(outcome.model_delay_native, Some(0));

        // A passthrough must come back sample-exact once aligned.
        for (index, (actual, expected)) in outcome
            .clip
            .samples
            .iter()
            .zip(&clip.samples)
            .enumerate()
            .take(clip.samples.len() - outcome.trimmed)
        {
            assert!(
                (actual - expected).abs() < 1e-6,
                "sample {index}: {actual} vs {expected}"
            );
        }
    }

    #[test]
    fn a_resampled_stage_round_trips_within_tolerance() {
        let rate = 48_000;
        let clip = Clip {
            samples: speech_like(rate, 4),
            sample_rate: rate,
        };
        let outcome = run(
            Box::new(Passthrough::new(16_000, 160)),
            &clip,
            rate,
            Alignment::Measured,
        )
        .unwrap();
        assert_eq!(outcome.underruns, 0);
        // The measurement should land close to what the runner reports; the
        // signal has no content above 1 kHz so a trip through 16 kHz is benign.
        let reported = outcome.reported_delay;
        let measured = outcome.measured_delay.expect("measurable");
        assert!(
            measured.abs_diff(reported) <= 4,
            "measured {measured}, reported {reported}"
        );
    }

    #[test]
    fn alignment_modes_differ_as_documented() {
        let rate = 48_000;
        let clip = Clip {
            samples: speech_like(rate, 3),
            sample_rate: rate,
        };
        let none = run(
            Box::new(Passthrough::new(rate, 480)),
            &clip,
            rate,
            Alignment::None,
        )
        .unwrap();
        assert_eq!(none.trimmed, 0);

        let reported = run(
            Box::new(Passthrough::new(rate, 480)),
            &clip,
            rate,
            Alignment::Reported,
        )
        .unwrap();
        assert_eq!(reported.trimmed, reported.reported_delay);
    }

    #[test]
    fn resampling_a_clip_changes_its_rate() {
        let clip = Clip {
            samples: speech_like(48_000, 1),
            sample_rate: 48_000,
        };
        let converted = resample(&clip, 16_000).unwrap();
        assert_eq!(converted.sample_rate, 16_000);
        assert_eq!(converted.samples.len(), 16_000);
        let unchanged = resample(&clip, 48_000).unwrap();
        assert_eq!(unchanged.samples.len(), clip.samples.len());
    }
}
