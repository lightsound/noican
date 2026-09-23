//! Hush with 48 kHz output: a band-split wrapper around the 16 kHz Hush
//! core (`hush-48k` in the registry).
//!
//! Hush is trained and shipped at 16 kHz only, so the plain `hush` stage
//! runs it through the 3:1 [`Decimator`] / [`Interpolator`] pair and
//! everything above 8 kHz is gone from the output. This stage keeps the
//! same core and the same latency, and adds the input's own upper band
//! back — gated by what Hush decided for the adjacent band:
//!
//! ```text
//! x ──┬─ decim(3) ─ Hush(16 kHz) ─ interp(3) ───────────────┐
//!     │        │                                            ├─(+)─ × makeup ─ out
//!     │        └─ interp(3) ─(−)─ delay(480) ─ × g_hi(t) ───┘
//!     └───── delay(120) ────┘
//! ```
//!
//! `x delayed 120 − interp(decim(x))` is the part of the input the model
//! never saw (the 8–24 kHz band plus the resampler's transition region),
//! exactly time-aligned with what it did see, so when the core is the
//! identity and `g_hi = 1` the stage reduces to a pure 120-sample delay
//! (a unit test pins this). `g_hi` is the broadband gain Hush applied to
//! the top of its own band (`BAND_GAIN_BINS`, 4–7 kHz), read from the model's
//! noisy and enhanced spectra after each frame (see
//! `DfTractStage::band_gain`) and ramped linearly across the 10 ms
//! frame. When Hush mutes a frame the added band is muted with it; when
//! Hush passes the frame the band passes at Hush's own gain.
//!
//! # Decision record (2026-09-11)
//!
//! Options weighed for getting Hush's suppression at 48 kHz output
//! quality, against the axes in docs/tech-research.md §6.4 (high-band
//! retention, suppression preserved — above all no high-band leak —
//! own-voice fidelity, added latency and 10 ms block cost,
//! implementation and maintenance cost, dependencies and licences,
//! fail-safe behaviour, compatibility with the lock-free switch and the
//! dry/wet compensation):
//!
//! - **Status quo** (`hush`, 16 kHz output): keeps the suppression, loses
//!   everything above 8 kHz (`HF keep` ≈ −80 dB on the harness). Rejected
//!   as the default candidate; the entry stays for comparison.
//! - **A. Band-split with an ungated high band**: transparent above
//!   8 kHz but leaks the interferer's sibilants whenever Hush mutes —
//!   the one failure the evaluation treats as disqualifying. Rejected on
//!   its own, kept as the frame of the chosen design.
//! - **B. Sidechain, external gain**: run a separate STFT on the 16 kHz
//!   input and output and derive a gain. Works but duplicates the
//!   model's analysis, and its frame alignment with Hush's internal
//!   STFT would be approximate. Superseded by C′.
//! - **C. Sidechain via the model's mask (fork `deep_filter`)**: expose
//!   the ERB mask. Rejected: the upstream crate already exposes the
//!   post-mask spectra publicly (`DfTract::get_spec_noisy` /
//!   `get_spec_enh`, `libDF/src/tract.rs`), so no fork is needed.
//! - **C′ (chosen, with A's frame)**: gate the input's high band with the
//!   gain Hush applied in 4–7 kHz, read from those public spectra. No
//!   extra analysis, exact frame alignment, zero added latency, no new
//!   dependency, and the fail-safe is the plain `hush` output (gain 0 →
//!   identical signal up to the makeup gain).
//! - **D. Run Hush at 48 kHz by configuration**: rejected on inspection —
//!   the tarball's `config.ini` fixes the STFT (`fft_size` 320, `hop`
//!   160, `sr` 16000), the ERB filterbank and the network input widths;
//!   `DfParams` only reads them.
//! - **E. Wait for an upstream 48 kHz Hush**: checked 2026-09-11 —
//!   `weya-ai/hush` on Hugging Face ships the 16 kHz tarball only and its
//!   README lists a retrain as "coming soon"; `pulp-vision/Hush` has one
//!   release (v1.0.0, 2026-03-20) and its 2026-07-18 commit *removed* an
//!   embedded 48 kHz DeepFilterNet3 (not Hush). Nothing to adopt; the
//!   registry can add it as another entry if it ever lands.
//! - **F. Neural bandwidth extension after Hush**: no streaming,
//!   sub-10 ms CPU model with a permissive licence is available, and a
//!   generative upper band is the wrong tool when the true upper band is
//!   in the input. Rejected.
//!
//! A second exploration round (heterodyning a second Hush pass onto the
//! upper band; mapping the ERB mask through `erb_fb` per bin; running the
//! tract graphs at 48 kHz) produced nothing better than C′ and stopped.
//!
//! **High-band source** (dry input vs. a light 48 kHz denoiser): the dry
//! input adds no latency and no second weight file; a denoiser in the
//! upper path would push the stage's latency to its own delay plus
//! alignment (≈ 31 ms total for a ~21 ms denoiser) and roughly double
//! the block cost. The dry route is chosen; if hiss in the upper
//! band proves audible in listening, a denoised upper path is the
//! documented follow-up.
//!
//! **Makeup gain**: the core runs without `HUSH_MAKEUP_GAIN_DB` and the
//! gain is applied once to the recombined output, so both bands share
//! Hush's measured level characteristic (the high band is scaled by
//! Hush's own gain via `g_hi`, hence needs the same correction).
//!
//! **Level dependence** (harness, 2026-09-11): Hush attenuates a lone
//! voice more the hotter it is (parity at −37 dBFS, −2…−17 dB at
//! −25 dBFS). Normalising the core's input level is out of scope here
//! and recorded as a follow-up; `noican eval --target-level-dbfs` exists
//! to measure it.

use std::collections::VecDeque;
use std::ops::Range;
use std::path::Path;

use noican_core::resample::{Decimator, Interpolator};
use noican_core::{ENGINE_SAMPLE_RATE, FrameProcessor, StageError};

use super::dfn_tract::{DfTractStage, HUSH_MAKEUP_GAIN_DB, apply_gain, db_to_linear};

/// Hush's native rate; the core is rejected at construction if the
/// tarball says otherwise.
const CORE_SAMPLE_RATE: u32 = 16_000;

/// Engine rate over Hush's rate (48 000 / 16 000; a unit test pins the
/// arithmetic).
const FACTOR: usize = 3;

/// Hush's hop at 16 kHz; one frame of this stage is `HOP × FACTOR` at
/// 48 kHz (10 ms either way).
const HOP: usize = 160;

/// STFT bins of Hush's spectrum whose gain steers the added band:
/// 4.0–7.0 kHz inclusive (`bin × 16 000 / 320` Hz, 50 Hz per bin).
///
/// Below 4 kHz Hush's ERB bands follow individual formants and pitch
/// harmonics — decisions that say little about the 8–24 kHz band. Above
/// ≈ 7.2 kHz the decimation lowpass (cutoff 0.45 × 8 kHz) has already
/// rolled the model's input off, so those bins carry little energy. The
/// octave in between is where the model's judgement most resembles what
/// it would do to the band being reconstructed. Tuning variants
/// (2–7 kHz, 6–7.2 kHz, and a 30 ms release) were measured with `noican
/// eval`; the results are recorded in the pull request that introduced
/// this stage.
const BAND_GAIN_BINS: Range<usize> = 80..141;

/// A delay line holding `len` zeros with room for `capacity` samples, so
/// the first frame's pushes (which precede the pops) never reallocate on
/// the inference thread.
fn primed_deque(len: usize, capacity: usize) -> VecDeque<f32> {
    let mut deque = VecDeque::with_capacity(capacity);
    deque.extend(std::iter::repeat_n(0.0, len));
    deque
}

/// Splits the engine-rate input into Hush's band and the remainder, and
/// recombines Hush's output with the (gated) remainder.
///
/// Pure DSP, no model: the core sits between [`Self::split`] and
/// [`Self::merge`] so the reconstruction can be tested with an identity
/// core.
#[derive(Debug)]
struct BandRecombiner {
    decimator: Decimator,
    /// Interpolates the *decimated input* to form the low band reference.
    reference: Interpolator,
    /// Interpolates the core's output.
    enhanced: Interpolator,
    /// Input delayed by the decimator + interpolator group delay.
    input_delay: VecDeque<f32>,
    /// The high band, delayed by the core's own output delay (at 48 kHz).
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
    /// `core_delay` is the core's [`FrameProcessor::output_delay`]
    /// scaled to 48 kHz.
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

    /// Total delay of the recombined output relative to the input, at
    /// 48 kHz.
    const fn output_delay(&self) -> usize {
        self.filter_delay + self.core_delay
    }

    /// Consumes one 480-sample frame: returns the 160-sample low band for
    /// the core and queues the aligned high band for [`Self::merge`].
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

    /// Interpolates the core's 160-sample output and adds the queued high
    /// band scaled by a linear ramp from the previous frame's gain to
    /// `gain`.
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

/// Hush with the input's upper band added back under Hush's own gate.
///
/// A 48 kHz [`FrameProcessor`] with 480-sample frames; wrapped by
/// [`noican_core::FramedStage`] it reports the same latency as `hush`
/// (1080 samples, 22.5 ms).
pub struct HushWidebandStage {
    id: String,
    core: DfTractStage,
    recombiner: BandRecombiner,
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
    /// Loads the Hush core from its tarball (the `hush` registry entry's
    /// file) and wraps it.
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
        Ok(Self {
            id: id.to_owned(),
            core,
            recombiner,
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
        let low = self.recombiner.split(input);
        self.core.process_frame(low, &mut self.core_out)?;
        let gain = self.core.band_gain(BAND_GAIN_BINS);
        self.recombiner.merge(&self.core_out, gain, output);
        apply_gain(output, self.makeup_gain);
        Ok(())
    }

    fn reset(&mut self) {
        self.core.reset();
        self.recombiner.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: usize = HOP * FACTOR;

    /// Wideband test signal: a low tone the 16 kHz path keeps, a 12 kHz
    /// tone only the reconstructed band can carry, and a slow envelope.
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

    /// Runs the recombiner around a core that is the identity delayed by
    /// `core_delay_low` samples at 16 kHz, with a constant gain.
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
            // Skip the priming region; compare against the delayed input.
            let start = delay + FRAME;
            let err = max_abs_diff(&out[start..], &input[start - delay..input.len() - delay]);
            assert!(
                err < 1e-5,
                "core delay {core_delay_low}: reconstruction error {err}"
            );
        }
    }

    #[test]
    fn zero_gain_leaves_only_the_band_limited_path() {
        let input = wideband_signal(20);
        let (out, delay) = run_identity(&input, 0, 0.0);
        // The 12 kHz component (amplitude 0.3) must be gone; the 440 Hz
        // component (0.5) must survive: the output peak sits near 0.5,
        // well below the 0.8 the input reaches.
        let start = delay + FRAME;
        let peak = out[start..].iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!((0.45..0.56).contains(&peak), "peak {peak}");
        // And it equals interp(decim(input)) to within the resampler's
        // passband ripple; the low tone alone is a ~440 Hz sine.
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
        // A pure high-band input through an identity core with no core
        // delay: the merged output is (ramped gain) × high band. Compare
        // frame 3 (gain 0 → 1) against the unity-gain run.
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
                // The 12 kHz tone is entirely high band, so `reference`
                // is the delayed tone and `out` is that tone under the
                // ramp. Check a few points on the ramp.
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
        // Silence in, silence out after the reset.
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

    /// With real weights: the stage reports Hush's latency, produces
    /// finite output, and its band-limited part matches `hush` (the two
    /// stages share the core, so with the high band removed from the
    /// input they must agree up to the makeup gain placement — which is
    /// identical).
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

    /// The Hush tarball when the weights are fetched, else `None` after
    /// printing a skip notice.
    fn hush_tarball() -> Option<std::path::PathBuf> {
        let models_dir = std::env::var_os("NOICAN_MODELS_DIR").map_or_else(
            || {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join("models")
            },
            std::path::PathBuf::from,
        );
        let spec = crate::manifest::ModelSpec::find("hush").expect("hush is in the registry");
        if !crate::fetch::is_fetched(&models_dir, spec) {
            #[expect(
                clippy::print_stderr,
                reason = "test skip notices must be visible in the test log"
            )]
            {
                eprintln!(
                    "[skip] hush: weights not fetched under {}",
                    models_dir.display()
                );
            }
            return None;
        }
        Some(crate::fetch::model_dir(&models_dir, spec).join(spec.files[0].name))
    }

    /// With real weights: the gate closes. A 12 kHz tone lies entirely
    /// in the band the core never sees, so the core's input is below its
    /// silence threshold, `band_gain` is 0, and the tone must not reach
    /// the output — an ungated band-split (option A in the module docs)
    /// would pass it at full level.
    #[test]
    #[ignore = "requires downloaded model weights (run: noican fetch hush-48k)"]
    fn gate_closes_when_the_core_hears_nothing() {
        use noican_core::{FramedStage, Stage as _};

        let Some(tarball) = hush_tarball() else {
            return;
        };
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
            // The tone's abrupt onset is broadband and leaks through the
            // decimator for the first frame; the gate must be shut from the
            // second frame on.
            if i >= 1 {
                assert!(
                    stage.core.band_gain(BAND_GAIN_BINS).abs() < f32::EPSILON,
                    "the core saw nothing yet reported a gain at frame {i}"
                );
            }
        }
        // Steady state through the Stage interface: output RMS must sit
        // at least 40 dB under the input's.
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

    /// With real weights: on speech-like material the gate is open but
    /// never amplifies, and a silent frame reads 0 even right after a
    /// loud one.
    #[test]
    #[ignore = "requires downloaded model weights (run: noican fetch hush-48k)"]
    fn band_gain_is_bounded_and_zero_on_silence() {
        let Some(tarball) = hush_tarball() else {
            return;
        };
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
    fn wideband_matches_hush_on_band_limited_input() {
        use noican_core::{FramedStage, Stage as _};

        let Some(tarball) = hush_tarball() else {
            return;
        };
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

        // Band-limit the test signal the same way the 16 kHz path does,
        // so the high band the wideband stage would add is ~zero.
        let raw = wideband_signal(100);
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
        // The residual high band of a band-limited input is the
        // resampler's stopband (−60 dB and below).
        assert!(
            err < peak * 0.01,
            "wideband differs from hush by {err} (peak {peak})"
        );
    }
}
