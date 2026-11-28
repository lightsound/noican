//! See docs/adr/0007-dsp-stages.md#hush-wideband-band-split.

use std::collections::VecDeque;
use std::ops::Range;
use std::path::Path;

use noican_core::resample::{Decimator, Interpolator};
use noican_core::{ENGINE_SAMPLE_RATE, FrameProcessor, StageError};

use super::dfn_tract::{DfTractStage, HUSH_MAKEUP_GAIN_DB, apply_gain, db_to_linear};
use super::leveler::{HUSH_TARGET_LEVEL_DBFS, InputLeveler};

const CORE_SAMPLE_RATE: u32 = 16_000;

const FACTOR: usize = 3;

const HOP: usize = 160;

const BAND_GAIN_BINS: Range<usize> = 80..141;

fn primed_deque(len: usize, capacity: usize) -> VecDeque<f32> {
    let mut deque = VecDeque::with_capacity(capacity);
    deque.extend(std::iter::repeat_n(0.0, len));
    deque
}

#[derive(Debug)]
struct BandRecombiner {
    decimator: Decimator,
    reference: Interpolator,
    enhanced: Interpolator,
    input_delay: VecDeque<f32>,
    high_delay: VecDeque<f32>,
    low: Vec<f32>,
    low_ref: Vec<f32>,
    up: Vec<f32>,
    frame_len: usize,
    filter_delay: usize,
    core_delay: usize,
    previous_gain: f32,
}

impl BandRecombiner {
    fn new(core_delay: usize) -> Self {
        let frame_len = HOP * FACTOR;
        let decimator = Decimator::new(FACTOR, frame_len);
        let reference = Interpolator::new(FACTOR, HOP);
        let enhanced = Interpolator::new(FACTOR, HOP);
        let filter_delay = decimator.delay_input_samples() + reference.delay_output_samples();
        Self {
            decimator,
            reference,
            enhanced,
            input_delay: primed_deque(filter_delay, filter_delay + frame_len),
            high_delay: primed_deque(core_delay, core_delay + frame_len),
            low: Vec::with_capacity(HOP),
            low_ref: Vec::with_capacity(frame_len),
            up: Vec::with_capacity(frame_len),
            frame_len,
            filter_delay,
            core_delay,
            previous_gain: 0.0,
        }
    }

    const fn output_delay(&self) -> usize {
        self.filter_delay + self.core_delay
    }

    fn split(&mut self, input: &[f32]) -> &[f32] {
        debug_assert_eq!(input.len(), self.frame_len);
        self.low.clear();
        self.decimator.process(input, &mut self.low);
        self.low_ref.clear();
        self.reference.process(&self.low, &mut self.low_ref);
        for (x, low) in input.iter().zip(&self.low_ref) {
            self.input_delay.push_back(*x);
            let delayed = self.input_delay.pop_front().unwrap_or(0.0);
            self.high_delay.push_back(delayed - low);
        }
        &self.low
    }

    fn merge(&mut self, enhanced: &[f32], gain: f32, output: &mut [f32]) {
        debug_assert_eq!(enhanced.len(), HOP);
        debug_assert_eq!(output.len(), self.frame_len);
        self.up.clear();
        self.enhanced.process(enhanced, &mut self.up);
        #[expect(
            clippy::cast_precision_loss,
            reason = "frame length is 480; exact in f32"
        )]
        let step = (gain - self.previous_gain) / self.frame_len as f32;
        let mut g = self.previous_gain;
        for (out, up) in output.iter_mut().zip(&self.up) {
            g += step;
            let high = self.high_delay.pop_front().unwrap_or(0.0);
            *out = g.mul_add(high, *up);
        }
        self.previous_gain = gain;
    }

    fn reset(&mut self) {
        self.decimator.reset();
        self.reference.reset();
        self.enhanced.reset();
        self.input_delay.clear();
        self.input_delay
            .extend(std::iter::repeat_n(0.0, self.filter_delay));
        self.high_delay.clear();
        self.high_delay
            .extend(std::iter::repeat_n(0.0, self.core_delay));
        self.previous_gain = 0.0;
    }
}

/// See docs/adr/0007-dsp-stages.md#hush-wideband-band-split.
pub struct HushWidebandStage {
    id: String,
    core: DfTractStage,
    recombiner: BandRecombiner,
    leveler: InputLeveler,
    leveled: Vec<f32>,
    core_out: Vec<f32>,
    makeup_gain: f32,
}

impl std::fmt::Debug for HushWidebandStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HushWidebandStage")
            .field("id", &self.id)
            .field("core", &self.core)
            .finish_non_exhaustive()
    }
}

impl HushWidebandStage {
    /// See docs/adr/0007-dsp-stages.md#hush-core-contract.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] when the tarball cannot be loaded
    /// and [`StageError::Unsupported`] when it is not the 16 kHz /
    /// 160-hop model this wrapper is built around.
    pub fn new(id: &str, tarball: &Path) -> Result<Self, StageError> {
        let core = DfTractStage::hush_unity_gain(id, tarball)?;
        if core.sample_rate() != CORE_SAMPLE_RATE || core.frame_len() != HOP {
            return Err(StageError::Unsupported(format!(
                "{id}: expected a {CORE_SAMPLE_RATE} Hz / {HOP}-hop core, got {} Hz / {}-hop",
                core.sample_rate(),
                core.frame_len()
            )));
        }
        if BAND_GAIN_BINS.end > core.n_freqs() {
            return Err(StageError::Unsupported(format!(
                "{id}: core has {} bins, fewer than the {} the band gate reads",
                core.n_freqs(),
                BAND_GAIN_BINS.end
            )));
        }
        let recombiner = BandRecombiner::new(core.output_delay() * FACTOR);
        let leveler = InputLeveler::new(
            HUSH_TARGET_LEVEL_DBFS,
            ENGINE_SAMPLE_RATE,
            HOP * FACTOR,
            recombiner.output_delay(),
        );
        Ok(Self {
            id: id.to_owned(),
            core,
            recombiner,
            leveler,
            leveled: vec![0.0; HOP * FACTOR],
            core_out: vec![0.0; HOP],
            makeup_gain: db_to_linear(HUSH_MAKEUP_GAIN_DB),
        })
    }
}

impl FrameProcessor for HushWidebandStage {
    fn id(&self) -> &str {
        &self.id
    }

    fn sample_rate(&self) -> u32 {
        ENGINE_SAMPLE_RATE
    }

    fn frame_len(&self) -> usize {
        HOP * FACTOR
    }

    fn output_delay(&self) -> usize {
        self.recombiner.output_delay()
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        self.leveled.copy_from_slice(input);
        self.leveler.apply(&mut self.leveled);
        let low = self.recombiner.split(&self.leveled);
        let result = self.core.process_frame(low, &mut self.core_out);
        let gain = if result.is_ok() {
            self.core.band_gain(BAND_GAIN_BINS)
        } else {
            self.core_out.fill(0.0);
            0.0
        };
        self.recombiner.merge(&self.core_out, gain, output);
        self.leveler.restore(output);
        apply_gain(output, self.makeup_gain);
        result
    }

    fn reset(&mut self) {
        self.core.reset();
        self.recombiner.reset();
        self.leveler.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: usize = HOP * FACTOR;

    fn wideband_signal(frames: usize) -> Vec<f32> {
        (0..frames * FRAME)
            .map(|n| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "sample indices fit f32 for a short test signal"
                )]
                let t = n as f32 / 48_000.0;
                let low = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                let high = (2.0 * std::f32::consts::PI * 12_000.0 * t).sin();
                0.3f32.mul_add(high, 0.5 * low)
            })
            .collect()
    }

    fn speech_like_signal(frames: usize) -> Vec<f32> {
        const FORMANTS: [(f32, f32); 3] = [(700.0, 130.0), (1_220.0, 170.0), (2_600.0, 250.0)];
        let tau = std::f32::consts::TAU;
        let mut phase = 0.0_f32;
        (0..frames * FRAME)
            .map(|n| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "sample indices fit f32 for a short test signal"
                )]
                let t = n as f32 / 48_000.0;
                let pitch = 130.0 * 0.12f32.mul_add((tau * 0.7 * t).sin(), 1.0);
                phase = (phase + tau * pitch / 48_000.0) % tau;
                let voiced: f32 = (1..=48_u8)
                    .map(|harmonic| {
                        let harmonic = f32::from(harmonic);
                        let envelope: f32 = FORMANTS
                            .iter()
                            .map(|(centre, width)| {
                                let sweep = (tau * 1.3).mul_add(t, *centre).sin();
                                let centre = centre * 0.25f32.mul_add(sweep, 1.0);
                                let offset = harmonic.mul_add(pitch, -centre) / width;
                                1.0 / offset.mul_add(offset, 1.0)
                            })
                            .sum();
                        envelope / harmonic.sqrt() * (harmonic * phase).sin()
                    })
                    .sum();
                let syllable = 0.5 * (1.0 - (tau * 3.7 * t).cos());
                voiced * syllable.powf(1.5)
            })
            .collect()
    }

    fn run_identity(input: &[f32], core_delay_low: usize, gain: f32) -> (Vec<f32>, usize) {
        let mut rec = BandRecombiner::new(core_delay_low * FACTOR);
        let mut core_fifo: VecDeque<f32> = std::iter::repeat_n(0.0, core_delay_low).collect();
        let mut out = vec![0.0_f32; input.len()];
        let mut enhanced = vec![0.0_f32; HOP];
        for (frame_in, frame_out) in input.chunks(FRAME).zip(out.chunks_mut(FRAME)) {
            let low = rec.split(frame_in).to_vec();
            core_fifo.extend(low);
            for e in &mut enhanced {
                *e = core_fifo.pop_front().unwrap_or(0.0);
            }
            rec.merge(&enhanced, gain, frame_out);
        }
        (out, rec.output_delay())
    }

    fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b)
            .fold(0.0_f32, |m, (x, y)| m.max((x - y).abs()))
    }

    #[test]
    fn output_delay_is_the_filters_plus_the_core() {
        assert_eq!(BandRecombiner::new(0).output_delay(), 120);
        assert_eq!(BandRecombiner::new(480).output_delay(), 600);
    }

    #[test]
    fn identity_core_and_unity_gain_reconstruct_the_input_exactly() {
        let input = wideband_signal(20);
        for core_delay_low in [0, 160] {
            let (out, delay) = run_identity(&input, core_delay_low, 1.0);
            let start = delay + FRAME;
            let err = max_abs_diff(&out[start..], &input[start - delay..input.len() - delay]);
            assert!(
                err < 1e-5,
                "core delay {core_delay_low}: reconstruction error {err}"
            );
        }
    }

    #[test]
    fn leveler_and_recombiner_are_transparent_with_an_identity_core() {
        let input: Vec<f32> = wideband_signal(400).iter().map(|s| s * 0.6).collect();
        let mut rec = BandRecombiner::new(480);
        let delay = rec.output_delay();
        let mut leveler =
            InputLeveler::new(HUSH_TARGET_LEVEL_DBFS, ENGINE_SAMPLE_RATE, FRAME, delay);
        let mut core_fifo: VecDeque<f32> = std::iter::repeat_n(0.0, 160).collect();
        let mut out = vec![0.0_f32; input.len()];
        let mut trimmed = vec![0.0_f32; FRAME];
        let mut enhanced = vec![0.0_f32; HOP];
        let mut deepest = 0.0_f32;
        for (frame_in, frame_out) in input.chunks(FRAME).zip(out.chunks_mut(FRAME)) {
            trimmed.copy_from_slice(frame_in);
            leveler.apply(&mut trimmed);
            let low = rec.split(&trimmed).to_vec();
            core_fifo.extend(low);
            for e in &mut enhanced {
                *e = core_fifo.pop_front().unwrap_or(0.0);
            }
            rec.merge(&enhanced, 1.0, frame_out);
            leveler.restore(frame_out);
            deepest = deepest.min(leveler.gain_db());
        }
        assert!(deepest < -15.0, "the leveler never engaged ({deepest} dB)");
        let start = delay + FRAME;
        let err = max_abs_diff(&out[start..], &input[start - delay..input.len() - delay]);
        assert!(
            err < 1e-4,
            "reconstruction error {err} with the leveler engaged"
        );
    }

    #[test]
    fn zero_gain_leaves_only_the_band_limited_path() {
        let input = wideband_signal(20);
        let (out, delay) = run_identity(&input, 0, 0.0);
        let start = delay + FRAME;
        let peak = out[start..].iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!((0.45..0.56).contains(&peak), "peak {peak}");
        let expected: Vec<f32> = (0..out.len())
            .map(|n| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "sample indices fit f32 for a short test signal"
                )]
                let t = (n as f32 - delay as f32) / 48_000.0;
                0.5 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            })
            .collect();
        let err = max_abs_diff(&out[start..], &expected[start..]);
        assert!(err < 2e-3, "band-limited path error {err}");
    }

    #[test]
    fn gain_ramps_linearly_across_the_frame() {
        let input: Vec<f32> = (0..6 * FRAME)
            .map(|n| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "sample indices fit f32 for a short test signal"
                )]
                let t = n as f32 / 48_000.0;
                0.3 * (2.0 * std::f32::consts::PI * 12_000.0 * t).sin()
            })
            .collect();
        let mut rec = BandRecombiner::new(0);
        let mut unity = BandRecombiner::new(0);
        let mut out = vec![0.0_f32; FRAME];
        let mut reference = vec![0.0_f32; FRAME];
        for (i, frame) in input.chunks(FRAME).enumerate() {
            let low = rec.split(frame).to_vec();
            let low_unity = unity.split(frame).to_vec();
            let gain = if i >= 3 { 1.0 } else { 0.0 };
            rec.merge(&low, gain, &mut out);
            unity.merge(&low_unity, 1.0, &mut reference);
            if i == 3 {
                for (n, ramp) in [(119, 0.25), (239, 0.5), (359, 0.75), (479, 1.0)] {
                    let want = reference[n] * ramp;
                    assert!(
                        (out[n] - want).abs() < 1e-3,
                        "sample {n}: got {}, want {want}",
                        out[n]
                    );
                }
            }
            if i == 4 {
                let err = max_abs_diff(&out, &reference);
                assert!(err < 1e-5, "post-ramp frame differs by {err}");
            }
        }
    }

    #[test]
    fn reset_returns_to_the_primed_state() {
        let input = wideband_signal(4);
        let mut rec = BandRecombiner::new(480);
        let mut out = vec![0.0_f32; FRAME];
        for frame in input.chunks(FRAME) {
            let low = rec.split(frame).to_vec();
            rec.merge(&low, 1.0, &mut out);
        }
        rec.reset();
        let fresh = BandRecombiner::new(480);
        assert_eq!(rec.input_delay.len(), fresh.input_delay.len());
        assert_eq!(rec.high_delay.len(), fresh.high_delay.len());
        assert!(rec.input_delay.iter().all(|s| *s == 0.0));
        assert!(rec.high_delay.iter().all(|s| *s == 0.0));
        assert!(rec.previous_gain.abs() < f32::EPSILON);
        let silence = vec![0.0_f32; FRAME];
        let low = rec.split(&silence).to_vec();
        rec.merge(&low, 1.0, &mut out);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn factor_links_the_core_rate_to_the_engine_rate() {
        assert_eq!(
            u64::from(CORE_SAMPLE_RATE) * u64::try_from(FACTOR).expect("small constant"),
            u64::from(ENGINE_SAMPLE_RATE)
        );
    }

    #[test]
    fn band_gain_bins_cover_four_to_seven_kilohertz() {
        let hz_per_bin = 16_000 / 320;
        assert_eq!(BAND_GAIN_BINS.start * hz_per_bin, 4_000);
        assert_eq!((BAND_GAIN_BINS.end - 1) * hz_per_bin, 7_000);
        const { assert!(BAND_GAIN_BINS.end <= 320 / 2 + 1) };
    }

    #[test]
    fn delay_lines_never_reallocate_after_construction() {
        let input = wideband_signal(3);
        let mut rec = BandRecombiner::new(480);
        let (input_cap, high_cap) = (rec.input_delay.capacity(), rec.high_delay.capacity());
        let mut out = vec![0.0_f32; FRAME];
        for frame in input.chunks(FRAME) {
            let low = rec.split(frame).to_vec();
            assert_eq!(
                rec.input_delay.capacity(),
                input_cap,
                "input delay grew in split"
            );
            assert_eq!(
                rec.high_delay.capacity(),
                high_cap,
                "high delay grew in split"
            );
            rec.merge(&low, 1.0, &mut out);
        }
        assert_eq!(rec.input_delay.capacity(), input_cap);
        assert_eq!(rec.high_delay.capacity(), high_cap);
    }

    fn hush_tarball() -> std::path::PathBuf {
        let models_dir = std::env::var_os("NOICAN_MODELS_DIR").map_or_else(
            || {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join("models")
            },
            std::path::PathBuf::from,
        );
        let spec = crate::manifest::ModelSpec::find("hush").expect("hush is in the registry");
        assert!(
            crate::fetch::is_fetched(&models_dir, spec),
            "hush: weights not fetched under {dir} (run: noican --models-dir {dir} fetch hush)",
            dir = models_dir.display()
        );
        crate::fetch::model_dir(&models_dir, spec).join(spec.files[0].name)
    }

    #[test]
    #[ignore = "requires downloaded model weights (run: noican fetch hush-48k)"]
    fn gate_closes_when_the_core_hears_nothing() {
        use noican_core::{FramedStage, Stage as _};

        let tarball = hush_tarball();
        let mut stage = HushWidebandStage::new("hush-48k", &tarball).expect("stage should load");
        let tone: Vec<f32> = (0..60 * FRAME)
            .map(|n| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "sample indices fit f32 for a short test signal"
                )]
                let t = n as f32 / 48_000.0;
                0.3 * (2.0 * std::f32::consts::PI * 12_000.0 * t).sin()
            })
            .collect();
        let mut out = vec![0.0_f32; FRAME];
        for (i, frame) in tone.chunks(FRAME).enumerate() {
            stage
                .process_frame(frame, &mut out)
                .expect("processing should succeed");
            if i >= 1 {
                assert!(
                    stage.core.band_gain(BAND_GAIN_BINS).abs() < f32::EPSILON,
                    "the core saw nothing yet reported a gain at frame {i}"
                );
            }
        }
        let mut framed = FramedStage::new(stage, crate::factory::MAX_BLOCK_LEN)
            .expect("48 kHz divides the engine rate");
        let mut output = vec![0.0_f32; tone.len()];
        for (i, o) in tone.chunks(FRAME).zip(output.chunks_mut(FRAME)) {
            framed
                .process_block(i, o)
                .expect("processing should succeed");
        }
        let rms = |s: &[f32]| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "sample counts of a short test signal fit f32"
            )]
            let len = s.len() as f32;
            (s.iter().map(|x| x * x).sum::<f32>() / len).sqrt()
        };
        let (in_rms, out_rms) = (rms(&tone[10 * FRAME..]), rms(&output[10 * FRAME..]));
        assert!(
            out_rms < in_rms * 0.01,
            "gate leaked: output RMS {out_rms} vs input {in_rms}"
        );
    }

    #[test]
    #[ignore = "requires downloaded model weights (run: noican fetch hush-48k)"]
    fn band_gain_is_bounded_and_zero_on_silence() {
        let tarball = hush_tarball();
        let mut core = DfTractStage::hush_unity_gain("hush", &tarball).expect("stage should load");
        let mut out = vec![0.0_f32; HOP];
        let mut opened = false;
        for block in 0..50 {
            #[expect(
                clippy::cast_precision_loss,
                reason = "sample indices fit f32 for a short test signal"
            )]
            let frame: Vec<f32> = (0..HOP)
                .map(|n| {
                    let t = (block * HOP + n) as f32 / 16_000.0;
                    0.02f32.mul_add(
                        (2.0 * std::f32::consts::PI * 5_000.0 * t).sin(),
                        0.05 * (2.0 * std::f32::consts::PI * 220.0 * t).sin(),
                    )
                })
                .collect();
            core.process_frame(&frame, &mut out)
                .expect("processing should succeed");
            let gain = core.band_gain(BAND_GAIN_BINS);
            assert!((0.0..=1.0).contains(&gain), "gain {gain} out of range");
            opened |= gain > 0.0;
        }
        assert!(opened, "the gate never opened on a steady tone pair");
        core.process_frame(&vec![0.0; HOP], &mut out)
            .expect("processing should succeed");
        assert!(core.band_gain(BAND_GAIN_BINS).abs() < f32::EPSILON);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    #[ignore = "requires downloaded model weights (run: noican fetch hush-48k)"]
    fn hot_input_is_trimmed_for_the_core_and_restored_at_the_output() {
        use noican_core::{FramedStage, Stage as _};

        let tarball = hush_tarball();
        let rms_dbfs = |s: &[f32]| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "sample counts of a short test signal fit f32"
            )]
            let len = s.len() as f32;
            10.0 * (s.iter().map(|x| x * x).sum::<f32>() / len).log10()
        };
        let input: Vec<f32> = (0..600 * FRAME)
            .map(|n| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "sample indices fit f32 for a short test signal"
                )]
                let t = n as f32 / 48_000.0;
                0.04f32.mul_add(
                    (2.0 * std::f32::consts::PI * 5_000.0 * t).sin(),
                    0.13 * (2.0 * std::f32::consts::PI * 220.0 * t).sin(),
                )
            })
            .collect();
        let in_level = rms_dbfs(&input);
        assert!(
            (in_level + 20.0).abs() < 1.0,
            "test signal at {in_level} dBFS"
        );

        let mut stage = HushWidebandStage::new("hush-48k", &tarball).expect("stage should load");
        let mut out = vec![0.0_f32; FRAME];
        for frame in input.chunks(FRAME) {
            stage
                .process_frame(frame, &mut out)
                .expect("processing should succeed");
        }
        let trim = stage.leveler.gain_db();
        assert!(
            (trim - (HUSH_TARGET_LEVEL_DBFS - in_level)).abs() < 1.0,
            "trim {trim} dB for input at {in_level} dBFS"
        );

        let unscaled = speech_like_signal(600);
        let to_hot = db_to_linear(-20.0 - rms_dbfs(&unscaled));
        let speech: Vec<f32> = unscaled.iter().map(|s| s * to_hot).collect();
        let mut framed = FramedStage::new(
            HushWidebandStage::new("hush-48k", &tarball).expect("stage should load"),
            crate::factory::MAX_BLOCK_LEN,
        )
        .expect("48 kHz divides the engine rate");
        assert_eq!(framed.latency_samples(), 1080);
        let mut output = vec![0.0_f32; speech.len()];
        for (i, o) in speech.chunks(FRAME).zip(output.chunks_mut(FRAME)) {
            framed
                .process_block(i, o)
                .expect("processing should succeed");
        }
        let speech_level = rms_dbfs(&speech[300 * FRAME..]);
        let out_level = rms_dbfs(&output[300 * FRAME..]);
        assert!(
            (out_level - speech_level).abs() < HUSH_MAKEUP_GAIN_DB + 3.0,
            "output at {out_level} dBFS for speech-like input at {speech_level} dBFS"
        );
    }

    #[test]
    #[ignore = "requires downloaded model weights (run: noican fetch hush-48k)"]
    fn wideband_matches_hush_on_band_limited_input() {
        use noican_core::{FramedStage, Stage as _};

        let tarball = hush_tarball();
        let mut wide = FramedStage::new(
            HushWidebandStage::new("hush-48k", &tarball).expect("stage should load"),
            crate::factory::MAX_BLOCK_LEN,
        )
        .expect("48 kHz divides the engine rate");
        let mut narrow = FramedStage::new(
            DfTractStage::hush("hush", &tarball).expect("stage should load"),
            crate::factory::MAX_BLOCK_LEN,
        )
        .expect("16 kHz divides the engine rate");
        assert_eq!(wide.latency_samples(), narrow.latency_samples());
        assert_eq!(wide.latency_samples(), 1080);

        let raw: Vec<f32> = wideband_signal(100).iter().map(|s| s * 0.03).collect();
        let mut dec = Decimator::new(FACTOR, FRAME);
        let mut int = Interpolator::new(FACTOR, HOP);
        let mut input = Vec::with_capacity(raw.len());
        let mut low = Vec::with_capacity(HOP);
        for frame in raw.chunks(FRAME) {
            low.clear();
            dec.process(frame, &mut low);
            int.process(&low, &mut input);
        }
        let mut wide_out = vec![0.0_f32; input.len()];
        let mut narrow_out = vec![0.0_f32; input.len()];
        for ((i, w), n) in input
            .chunks(FRAME)
            .zip(wide_out.chunks_mut(FRAME))
            .zip(narrow_out.chunks_mut(FRAME))
        {
            wide.process_block(i, w).expect("processing should succeed");
            narrow
                .process_block(i, n)
                .expect("processing should succeed");
        }
        assert!(wide_out.iter().all(|s| s.is_finite()));
        let peak = narrow_out.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        let err = max_abs_diff(&wide_out[FRAME * 4..], &narrow_out[FRAME * 4..]);
        assert!(
            err < peak * 0.01,
            "wideband differs from hush by {err} (peak {peak})"
        );
    }
}
