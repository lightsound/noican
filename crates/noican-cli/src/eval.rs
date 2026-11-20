use std::ops::Range;

use noican_core::ENGINE_SAMPLE_RATE;
use realfft::RealFftPlanner;
use realfft::num_complex::Complex32;

pub(crate) const HIGH_BAND_HZ: f64 = 8_000.0;

pub(crate) const REGION_GUARD: usize = ENGINE_SAMPLE_RATE as usize / 4;

const TRIM_FRAME: usize = 480;

const TRIM_THRESHOLD_DBFS: f64 = -50.0;

const BAND_FFT_LEN: usize = 1024;

const DB_FLOOR: f64 = -120.0;

const SI_SDR_CEILING_DB: f64 = 100.0;

pub(crate) fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let energy: f64 = samples.iter().map(|s| f64::from(*s).powi(2)).sum();
    (energy / to_f64(samples.len())).sqrt()
}

pub(crate) fn amplitude_db(ratio: f64) -> f64 {
    if ratio <= 0.0 {
        DB_FLOOR
    } else {
        (20.0 * ratio.log10()).max(DB_FLOOR)
    }
}

pub(crate) fn power_db(ratio: f64) -> f64 {
    if ratio <= 0.0 {
        DB_FLOOR
    } else {
        (10.0 * ratio.log10()).max(DB_FLOOR)
    }
}

pub(crate) fn trim_silence(samples: &[f32]) -> &[f32] {
    let threshold = 10f64.powf(TRIM_THRESHOLD_DBFS / 20.0);
    let frames = samples.len() / TRIM_FRAME;
    let loud =
        |frame: usize| rms(&samples[frame * TRIM_FRAME..(frame + 1) * TRIM_FRAME]) >= threshold;
    let Some(first) = (0..frames).find(|&f| loud(f)) else {
        return &samples[..0];
    };
    let last = (0..frames).rev().find(|&f| loud(f)).unwrap_or(first);
    &samples[first * TRIM_FRAME..(last + 1) * TRIM_FRAME]
}

pub(crate) fn sir_gain(target: &[f32], interferer: &[f32], sir_db: f64) -> f32 {
    let interferer_rms = rms(interferer);
    if interferer_rms == 0.0 {
        return 0.0;
    }
    let wanted = rms(target) / 10f64.powf(sir_db / 20.0);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "audio gains are far inside f32 range"
    )]
    let gain = (wanted / interferer_rms) as f32;
    gain
}

pub(crate) fn level_gain(samples: &[f32], level_dbfs: f64) -> f32 {
    let current = rms(samples);
    if current == 0.0 {
        return 1.0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "audio gains are far inside f32 range"
    )]
    let gain = (10f64.powf(level_dbfs / 20.0) / current) as f32;
    gain
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Regions {
    pub(crate) target_only: Range<usize>,
    pub(crate) both: Range<usize>,
    pub(crate) interferer_only: Range<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct Mixture {
    pub(crate) input: Vec<f32>,
    pub(crate) target: Vec<f32>,
    pub(crate) regions: Regions,
    pub(crate) interferer_gain: f32,
}

pub(crate) fn build_mixture(
    target: &[f32],
    interferer: &[f32],
    sir_db: f64,
    segment_len: usize,
) -> Result<Mixture, String> {
    if segment_len == 0 {
        return Err("segment length must be positive".to_owned());
    }
    if target.len() < 2 * segment_len {
        return Err(format!(
            "target recording too short: {} samples, need {}",
            target.len(),
            2 * segment_len
        ));
    }
    if interferer.len() < 2 * segment_len {
        return Err(format!(
            "interferer recording too short: {} samples, need {}",
            interferer.len(),
            2 * segment_len
        ));
    }
    let l = segment_len;
    let gain = sir_gain(&target[l..2 * l], &interferer[..l], sir_db);
    let total = 3 * l;
    let mut target_track = vec![0.0_f32; total];
    let mut interferer_track = vec![0.0_f32; total];
    target_track[..2 * l].copy_from_slice(&target[..2 * l]);
    for (dst, src) in interferer_track[l..].iter_mut().zip(&interferer[..2 * l]) {
        *dst = src * gain;
    }
    let input = target_track
        .iter()
        .zip(&interferer_track)
        .map(|(t, i)| t + i)
        .collect();
    Ok(Mixture {
        input,
        target: target_track,
        regions: Regions {
            target_only: 0..l,
            both: l..2 * l,
            interferer_only: 2 * l..3 * l,
        },
        interferer_gain: gain,
    })
}

pub(crate) fn band_energy(samples: &[f32], lo_hz: f64, hi_hz: f64) -> f64 {
    if samples.len() < BAND_FFT_LEN {
        return 0.0;
    }
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(BAND_FFT_LEN);
    let window: Vec<f32> = (0..BAND_FFT_LEN)
        .map(|n| {
            #[expect(clippy::cast_precision_loss, reason = "window index is tiny")]
            let phase = 2.0 * std::f64::consts::PI * n as f64 / BAND_FFT_LEN as f64;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "window values are in 0..=1"
            )]
            let w = (-0.5_f64).mul_add(phase.cos(), 0.5) as f32;
            w
        })
        .collect();
    let bin_hz = f64::from(ENGINE_SAMPLE_RATE) / to_f64(BAND_FFT_LEN);
    let mut frame = vec![0.0_f32; BAND_FFT_LEN];
    let mut spectrum = vec![Complex32::new(0.0, 0.0); BAND_FFT_LEN / 2 + 1];
    let mut scratch = fft.make_scratch_vec();
    let mut energy = 0.0_f64;
    for chunk in samples.windows(BAND_FFT_LEN).step_by(BAND_FFT_LEN / 2) {
        for ((f, x), w) in frame.iter_mut().zip(chunk).zip(&window) {
            *f = x * w;
        }
        if fft
            .process_with_scratch(&mut frame, &mut spectrum, &mut scratch)
            .is_err()
        {
            return 0.0;
        }
        for (k, c) in spectrum.iter().enumerate() {
            let hz = to_f64(k) * bin_hz;
            if hz >= lo_hz && hz < hi_hz {
                energy += f64::from(c.norm_sqr());
            }
        }
    }
    energy
}

pub(crate) fn high_band_energy(samples: &[f32]) -> f64 {
    band_energy(samples, HIGH_BAND_HZ, f64::from(ENGINE_SAMPLE_RATE) / 2.0)
}

pub(crate) fn si_sdr_db(estimate: &[f32], reference: &[f32]) -> f64 {
    let len = estimate.len().min(reference.len());
    let mut dot = 0.0_f64;
    let mut ref_energy = 0.0_f64;
    for (e, r) in estimate[..len].iter().zip(&reference[..len]) {
        let (e, r) = (f64::from(*e), f64::from(*r));
        dot = e.mul_add(r, dot);
        ref_energy = r.mul_add(r, ref_energy);
    }
    if ref_energy == 0.0 || dot <= 0.0 {
        return DB_FLOOR;
    }
    let alpha = dot / ref_energy;
    let mut target_energy = 0.0_f64;
    let mut error_energy = 0.0_f64;
    for (e, r) in estimate[..len].iter().zip(&reference[..len]) {
        let projected = alpha * f64::from(*r);
        let error = f64::from(*e) - projected;
        target_energy = projected.mul_add(projected, target_energy);
        error_energy = error.mul_add(error, error_energy);
    }
    if error_energy == 0.0 {
        return SI_SDR_CEILING_DB;
    }
    power_db(target_energy / error_energy).min(SI_SDR_CEILING_DB)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Metrics {
    pub(crate) high_band_retention: f64,
    pub(crate) own_voice_level: f64,
    pub(crate) own_voice_si_sdr: f64,
    pub(crate) both_si_sdr: f64,
    pub(crate) interferer_residual: f64,
    pub(crate) interferer_residual_high: f64,
}

fn measured(region: &Range<usize>, len: usize) -> Range<usize> {
    let start = (region.start + REGION_GUARD).min(region.end).min(len);
    start..region.end.min(len)
}

pub(crate) fn evaluate(output: &[f32], mixture: &Mixture) -> Metrics {
    let len = output.len().min(mixture.input.len());
    let you = measured(&mixture.regions.target_only, len);
    let both = measured(&mixture.regions.both, len);
    let other = measured(&mixture.regions.interferer_only, len);

    let high_band_retention = power_db(
        high_band_energy(&output[you.clone()]) / high_band_energy(&mixture.target[you.clone()]),
    );
    let own_voice_level =
        amplitude_db(rms(&output[you.clone()]) / rms(&mixture.target[you.clone()]));
    let own_voice_si_sdr = si_sdr_db(&output[you.clone()], &mixture.target[you]);
    let both_si_sdr = si_sdr_db(&output[both.clone()], &mixture.target[both]);
    let interferer_residual =
        amplitude_db(rms(&output[other.clone()]) / rms(&mixture.input[other.clone()]));
    let interferer_residual_high = power_db(
        high_band_energy(&output[other.clone()]) / high_band_energy(&mixture.input[other]),
    );
    Metrics {
        high_band_retention,
        own_voice_level,
        own_voice_si_sdr,
        both_si_sdr,
        interferer_residual,
        interferer_residual_high,
    }
}

pub(crate) fn high_band_share_db(samples: &[f32]) -> f64 {
    power_db(
        high_band_energy(samples) / band_energy(samples, 0.0, f64::from(ENGINE_SAMPLE_RATE) / 2.0),
    )
}

pub(crate) fn quantile<T: Copy>(sorted: &[T], q: f64) -> T {
    assert!(!sorted.is_empty(), "quantile of an empty sample");
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "index arithmetic on a small vector; q is clamped to 0..=1"
    )]
    let index = ((sorted.len() - 1) as f64 * q.clamp(0.0, 1.0)).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

pub(crate) fn blind_order(n: usize, seed: u64) -> Vec<usize> {
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let mut order: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the modulus is a small usize, so the remainder fits"
        )]
        let j = (next() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    order
}

const fn to_f64(value: usize) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "sample counts are far below 2^53"
    )]
    let value = value as f64;
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(got: f64, want: f64, tolerance: f64) {
        assert!(
            (got - want).abs() <= tolerance,
            "expected {want} ± {tolerance}, got {got}"
        );
    }

    fn sine(freq: f64, amplitude: f64, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| {
                let phase =
                    2.0 * std::f64::consts::PI * freq * to_f64(n) / f64::from(ENGINE_SAMPLE_RATE);
                #[expect(clippy::cast_possible_truncation, reason = "test signal to f32")]
                let s = (amplitude * phase.sin()) as f32;
                s
            })
            .collect()
    }

    fn noise(amplitude: f32, len: usize, mut state: u32) -> Vec<f32> {
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                #[expect(clippy::cast_precision_loss, reason = "uniform noise")]
                let uniform = state as f32 / u32::MAX as f32;
                uniform.mul_add(2.0, -1.0) * amplitude
            })
            .collect()
    }

    #[test]
    fn rms_of_a_sine_is_amplitude_over_root_two() {
        let x = sine(1000.0, 0.5, 48_000);
        assert!((rms(&x) - 0.5 / std::f64::consts::SQRT_2).abs() < 1e-3);
        assert_close(rms(&[]), 0.0, 0.0);
    }

    #[test]
    fn decibel_helpers_match_known_points() {
        assert!((amplitude_db(10.0) - 20.0).abs() < 1e-9);
        assert!((power_db(10.0) - 10.0).abs() < 1e-9);
        assert_close(amplitude_db(0.0), DB_FLOOR, 0.0);
        assert_close(power_db(-1.0), DB_FLOOR, 0.0);
    }

    #[test]
    fn trim_silence_removes_only_the_quiet_ends() {
        let mut x = vec![0.0_f32; 480 * 5];
        x.extend(sine(440.0, 0.3, 480 * 10));
        x.extend(vec![0.0_f32; 480 * 3]);
        x.extend(vec![0.0_f32; 480 * 2]);
        x.extend(sine(440.0, 0.3, 480 * 4));
        x.extend(vec![0.0_f32; 480 * 7 + 100]);
        let trimmed = trim_silence(&x);
        assert_eq!(trimmed.len(), 480 * (10 + 3 + 2 + 4));
        assert!((trimmed[0] - x[480 * 5]).abs() < 1e-9);
    }

    #[test]
    fn trim_silence_of_silence_is_empty() {
        let x = vec![1e-6_f32; 4800];
        assert_eq!(trim_silence(&x), []);
    }

    #[test]
    fn sir_gain_produces_the_requested_ratio() {
        let target = sine(300.0, 0.4, 48_000);
        let interferer = noise(0.9, 48_000, 7);
        for sir in [12.0_f64, 6.0, 0.0, -6.0] {
            let gain = sir_gain(&target, &interferer, sir);
            let scaled: Vec<f32> = interferer.iter().map(|s| s * gain).collect();
            let measured = amplitude_db(rms(&target) / rms(&scaled));
            assert!((measured - sir).abs() < 0.01, "SIR {sir}: got {measured}");
        }
        assert_close(f64::from(sir_gain(&target, &[0.0; 100], 0.0)), 0.0, 0.0);
    }

    #[test]
    fn level_gain_normalizes_rms() {
        let x = sine(300.0, 0.1, 48_000);
        let gain = level_gain(&x, -20.0);
        let scaled: Vec<f32> = x.iter().map(|s| s * gain).collect();
        assert_close(amplitude_db(rms(&scaled)), -20.0, 0.01);
        assert_close(f64::from(level_gain(&[0.0; 10], -20.0)), 1.0, 0.0);
    }

    #[test]
    fn build_mixture_lays_out_three_regions_at_the_exact_sir() {
        let l = 4800;
        let target = sine(250.0, 0.3, 3 * l);
        let interferer = noise(0.5, 3 * l, 11);
        let mixture = build_mixture(&target, &interferer, 6.0, l).expect("valid");
        assert_eq!(mixture.input.len(), 3 * l);
        assert_eq!(
            mixture.regions,
            Regions {
                target_only: 0..l,
                both: l..2 * l,
                interferer_only: 2 * l..3 * l,
            }
        );
        for (got, want) in mixture.input[..l].iter().zip(&target[..l]) {
            assert_close(f64::from(*got), f64::from(*want), 0.0);
        }
        assert!(mixture.target[2 * l..].iter().all(|s| *s == 0.0));
        let interferer_track: Vec<f32> = mixture
            .input
            .iter()
            .zip(&mixture.target)
            .map(|(i, t)| i - t)
            .collect();
        assert!(interferer_track[..l].iter().all(|s| *s == 0.0));
        let sir = amplitude_db(rms(&mixture.target[l..2 * l]) / rms(&interferer_track[l..2 * l]));
        assert!((sir - 6.0).abs() < 0.01, "SIR in the overlap: {sir}");
        for (got, want) in interferer_track[2 * l..].iter().zip(&interferer[l..2 * l]) {
            assert_close(
                f64::from(*got),
                f64::from(want * mixture.interferer_gain),
                1e-6,
            );
        }
    }

    #[test]
    fn build_mixture_rejects_short_material() {
        let short = sine(250.0, 0.3, 100);
        let long = sine(250.0, 0.3, 10_000);
        assert!(build_mixture(&short, &long, 0.0, 1000).is_err());
        assert!(build_mixture(&long, &short, 0.0, 1000).is_err());
        assert!(build_mixture(&long, &long, 0.0, 0).is_err());
    }

    #[test]
    fn band_energy_separates_low_and_high_tones() {
        let low = sine(1000.0, 0.5, 48_000);
        let high = sine(12_000.0, 0.5, 48_000);
        let low_share = high_band_share_db(&low);
        let high_share = high_band_share_db(&high);
        assert!(
            low_share < -80.0,
            "1 kHz tone leaks into the high band: {low_share} dB"
        );
        assert!(
            high_share > -0.01,
            "12 kHz tone lost from the high band: {high_share} dB"
        );
        let ratio = power_db(high_band_energy(&high) / band_energy(&low, 0.0, HIGH_BAND_HZ));
        assert!(ratio.abs() < 0.1, "band energies differ by {ratio} dB");
        assert_close(band_energy(&low[..100], 0.0, 24_000.0), 0.0, 0.0);
    }

    #[test]
    fn si_sdr_is_scale_invariant_and_tracks_noise_level() {
        let reference = sine(440.0, 0.5, 48_000);
        assert_close(si_sdr_db(&reference, &reference), SI_SDR_CEILING_DB, 0.0);
        let half: Vec<f32> = reference.iter().map(|s| s * 0.5).collect();
        assert_close(si_sdr_db(&half, &reference), SI_SDR_CEILING_DB, 0.0);
        let sine_rms = 0.5 / std::f64::consts::SQRT_2;
        #[expect(clippy::cast_possible_truncation, reason = "test amplitude")]
        let noise_amp = (sine_rms / 10.0 * 3.0_f64.sqrt()) as f32;
        let noisy: Vec<f32> = reference
            .iter()
            .zip(noise(noise_amp, 48_000, 3))
            .map(|(s, n)| s + n)
            .collect();
        let sdr = si_sdr_db(&noisy, &reference);
        assert!((sdr - 20.0).abs() < 0.5, "SI-SDR {sdr} dB, expected ≈ 20");
        assert_close(si_sdr_db(&vec![0.0; 48_000], &reference), DB_FLOOR, 0.0);
        assert_close(si_sdr_db(&reference, &vec![0.0; 48_000]), DB_FLOOR, 0.0);
    }

    #[test]
    fn evaluate_scores_the_identity_and_a_perfect_suppressor() {
        let l = 48_000;
        let target = {
            let mut t = sine(300.0, 0.3, 2 * l);
            for (s, h) in t.iter_mut().zip(sine(10_000.0, 0.1, 2 * l)) {
                *s += h;
            }
            t
        };
        let interferer = noise(0.4, 2 * l, 5);
        let mixture = build_mixture(&target, &interferer, 6.0, l).expect("valid");

        let identity = evaluate(&mixture.input, &mixture);
        assert!(identity.high_band_retention.abs() < 0.01);
        assert!(identity.own_voice_level.abs() < 0.01);
        assert_close(identity.own_voice_si_sdr, SI_SDR_CEILING_DB, 0.0);
        assert!((identity.both_si_sdr - 6.0).abs() < 0.3, "{identity:?}");
        assert!(identity.interferer_residual.abs() < 0.01);
        assert!(identity.interferer_residual_high.abs() < 0.01);

        let perfect = evaluate(&mixture.target, &mixture);
        assert_close(perfect.both_si_sdr, SI_SDR_CEILING_DB, 0.0);
        assert_close(perfect.interferer_residual, DB_FLOOR, 0.0);
        assert_close(perfect.interferer_residual_high, DB_FLOOR, 0.0);
    }

    #[test]
    fn evaluate_sees_a_band_limited_output_lose_the_high_band() {
        let l = 48_000;
        let mut target = sine(300.0, 0.3, 2 * l);
        for (s, h) in target.iter_mut().zip(sine(10_000.0, 0.1, 2 * l)) {
            *s += h;
        }
        let interferer = noise(0.4, 2 * l, 9);
        let mixture = build_mixture(&target, &interferer, 6.0, l).expect("valid");
        let mut low_only = sine(300.0, 0.3, 2 * l);
        low_only.resize(3 * l, 0.0);
        let metrics = evaluate(&low_only, &mixture);
        assert!(metrics.high_band_retention < -60.0, "{metrics:?}");
        assert!((metrics.own_voice_level + 0.46).abs() < 0.05, "{metrics:?}");
        assert!((metrics.own_voice_si_sdr - 9.54).abs() < 0.2, "{metrics:?}");
    }

    #[test]
    fn evaluate_skips_the_region_guard() {
        let l = 48_000;
        let target = sine(300.0, 0.3, 2 * l);
        let interferer = noise(0.4, 2 * l, 13);
        let mixture = build_mixture(&target, &interferer, 6.0, l).expect("valid");
        let mut output = mixture.target.clone();
        for s in &mut output[2 * l..2 * l + 9600] {
            *s = 1.0;
        }
        let metrics = evaluate(&output, &mixture);
        assert_close(metrics.interferer_residual, DB_FLOOR, 0.0);
    }

    #[test]
    fn quantile_picks_the_expected_ranks() {
        let sorted = [1_u128, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        assert_eq!(quantile(&sorted, 0.0), 1);
        assert_eq!(quantile(&sorted, 0.5), 6);
        assert_eq!(quantile(&sorted, 1.0), 10);
        assert_eq!(quantile(&[42_u128], 0.99), 42);
    }

    #[test]
    fn blind_order_is_a_deterministic_permutation() {
        let a = blind_order(7, 1);
        let b = blind_order(7, 1);
        assert_eq!(a, b);
        let mut sorted = a.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..7).collect::<Vec<_>>());
        assert_ne!(blind_order(7, 2), a);
        assert_eq!(blind_order(0, 1), Vec::<usize>::new());
        assert_eq!(blind_order(1, 1), vec![0]);
    }
}
