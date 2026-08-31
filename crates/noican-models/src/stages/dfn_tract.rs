//! DeepFilterNet-family stages via the upstream `deep_filter` (tract) crate.
//!
//! DeepFilterNet3 and Hush ship their networks as three ONNX graphs
//! (`enc`/`erb_dec`/`df_dec`) with a free time axis and *implicit*
//! recurrent state, which ONNX Runtime cannot stream one hop at a time.
//! The upstream `deep_filter` crate solves this with tract-pulse (the same
//! route Hush's own `libweya_nc` takes), so these two models run on tract
//! while every other model runs on ONNX Runtime — hidden behind the same
//! [`FrameProcessor`] interface.

use std::path::Path;

use df::tract::{DfParams, DfTract, ReduceMask, RuntimeParams};
use noican_core::{FrameProcessor, StageError};

/// Fixed makeup gain applied to Hush's output, in dB.
///
/// Hush's network attenuates speech, not just noise. Measurement (same
/// discipline as `LOOKAHEAD_HOPS` in the DPDFNet stage — an empirical
/// constant with its measurement recorded): the aligned CLI batch
/// (espeak-ng speech → 48 kHz mono → `process` → voiced-frame RMS over
/// 10 ms frames whose own level exceeds −50 dB, compared against
/// passthrough) measured a **−3.4 dB to −1.5 dB voiced-frame RMS deficit
/// depending on the speech material and level** (−3.4 dB on the
/// 2026-08-31 hardware-follow-up corpus at 0.70 peak; −2.5/−2.0/−1.9 dB
/// on two other espeak voices at 0.70 peak; −1.5 dB at 0.35 peak —
/// `x86_64` CLI builds). UL-UNAS on the same 16 kHz resampling path
/// measured ±0.4 dB, so the deficit is Hush's own gain characteristic,
/// not the rate conversion. This constant is the midpoint of the
/// measured deficit range, which brings every measured material within
/// the ±1 dB band around passthrough: residuals across the corpus above
/// are −0.95 dB to +0.95 dB after applying it (re-measured +0.5, −0.1
/// and +1.0 dB on the three re-runnable materials).
///
/// No clipping guard follows the gain, by design. The worst measured
/// output/input peak ratio after the gain is ≈1.16 (raw Hush peak 0.61
/// on a 0.70-peak input, ×1.33), so input peaking above ≈0.86 can push
/// isolated output samples past 0 dBFS in the all-`f32` engine path.
/// That path has no hard ceiling (floats do not wrap; downstream
/// consumers clamp on conversion), a microphone captured that hot is
/// already at its own clipping point, and a limiter here would distort
/// every loud syllable to guard a corner case — so the overshoot is
/// accepted and documented instead.
const HUSH_MAKEUP_GAIN_DB: f32 = 2.45;

/// Converts a dB gain into the linear factor applied per sample.
fn db_to_linear(gain_db: f32) -> f32 {
    10.0_f32.powf(gain_db / 20.0)
}

/// Applies a fixed linear gain in place (the Hush makeup gain hook;
/// separated from the model call so it is testable without weights).
fn apply_gain(samples: &mut [f32], linear_gain: f32) {
    for sample in samples {
        *sample *= linear_gain;
    }
}

/// Moves a whole [`DfTract`] across threads.
///
/// `DfTract` is not `Send` because tract's stateful plans hold `Rc`s
/// internally. Those `Rc`s are created by and confined to the `DfTract`
/// instance: we never clone the model (its `Clone` impl is unused here),
/// never hand out references to its internals, and only access it through
/// `&mut self` from one thread at a time — the whole object graph migrates
/// together, which is sound for `Rc`. Upstream ships the same pattern in
/// its C API (`DFState` is used from arbitrary caller threads).
struct SendModel(DfTract);

// SAFETY: see the type-level comment — no `Rc` inside the wrapped value
// can be observed from more than one thread because the value is only
// accessible via `&mut` and is never cloned or shared.
#[expect(
    unsafe_code,
    clippy::non_send_fields_in_send_ty,
    reason = "tract's stateful plans are thread-confined here; see SendModel docs"
)]
unsafe impl Send for SendModel {}

/// A DeepFilterNet-architecture stage (DeepFilterNet3 or Hush).
pub struct DfTractStage {
    id: String,
    model: SendModel,
    sample_rate: u32,
    hop: usize,
    output_delay: usize,
    /// Linear output gain (1.0 for DeepFilterNet3; the measured makeup
    /// gain for Hush — see [`HUSH_MAKEUP_GAIN_DB`]).
    output_gain: f32,
    /// Kept to rebuild the model on reset (`DfTract` has no state-reset API).
    params: DfParams,
    runtime: RuntimeParams,
}

impl std::fmt::Debug for DfTractStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DfTractStage")
            .field("id", &self.id)
            .field("sample_rate", &self.sample_rate)
            .field("hop", &self.hop)
            .finish_non_exhaustive()
    }
}

fn build(params: &DfParams, runtime: &RuntimeParams) -> Result<DfTract, StageError> {
    DfTract::new(params.clone(), runtime)
        .map_err(|e| StageError::Inference(format!("DfTract init failed: {e}")))
}

impl DfTractStage {
    /// DeepFilterNet3 48 kHz baseline from the model bundle embedded in the
    /// `deep_filter` crate (no download required).
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] when tract fails to build the
    /// model.
    pub fn deepfilternet3(id: &str) -> Result<Self, StageError> {
        Self::from_params(
            id,
            DfParams::default(),
            RuntimeParams::default_with_ch(1),
            1.0,
        )
    }

    /// Hush 16 kHz from its ONNX tarball (`advanced_dfnet16k_*.tar.gz`),
    /// using the thresholds Hush's own `weya_nc` runtime uses
    /// (min −15 dB, ERB/DF max 35 dB).
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] when the tarball cannot be loaded.
    pub fn hush(id: &str, tarball: &Path) -> Result<Self, StageError> {
        let params = DfParams::new(tarball.to_path_buf())
            .map_err(|e| StageError::Inference(format!("loading {}: {e}", tarball.display())))?;
        let runtime = RuntimeParams::new(1, 0.0, 100.0, -15.0, 35.0, 35.0, ReduceMask::MEAN);
        Self::from_params(id, params, runtime, db_to_linear(HUSH_MAKEUP_GAIN_DB))
    }

    fn from_params(
        id: &str,
        params: DfParams,
        runtime: RuntimeParams,
        output_gain: f32,
    ) -> Result<Self, StageError> {
        let model = build(&params, &runtime)?;
        let sample_rate = u32::try_from(model.sr)
            .map_err(|_| StageError::Inference("bad model sample rate".to_owned()))?;
        let hop = model.hop_size;
        // Overlap-add window delay plus the model's lookahead frames.
        let output_delay = (model.fft_size - hop) + model.lookahead * hop;
        Ok(Self {
            id: id.to_owned(),
            model: SendModel(model),
            sample_rate,
            hop,
            output_delay,
            output_gain,
            params,
            runtime,
        })
    }
}

impl FrameProcessor for DfTractStage {
    fn id(&self) -> &str {
        &self.id
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn frame_len(&self) -> usize {
        self.hop
    }

    fn output_delay(&self) -> usize {
        self.output_delay
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        let noisy = ndarray::ArrayView2::from_shape((1, self.hop), input)
            .map_err(|e| StageError::Inference(format!("bad input shape: {e}")))?;
        let enh = ndarray::ArrayViewMut2::from_shape((1, self.hop), output)
            .map_err(|e| StageError::Inference(format!("bad output shape: {e}")))?;
        self.model
            .0
            .process(noisy, enh)
            .map_err(|e| StageError::Inference(format!("DfTract process failed: {e}")))?;
        // A pure per-sample gain: latency, alignment, and the
        // cross-correlation residual are unaffected.
        if (self.output_gain - 1.0).abs() > f32::EPSILON {
            apply_gain(output, self.output_gain);
        }
        Ok(())
    }

    fn reset(&mut self) {
        // DfTract exposes no state reset; rebuild the plans. This is a
        // control-plane operation (model switch / stream restart), not an
        // audio-thread one, so the cost is acceptable.
        if let Ok(model) = build(&self.params, &self.runtime) {
            self.model = SendModel(model);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_to_linear_matches_known_points() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_linear(20.0) - 10.0).abs() < 1e-5);
        assert!((db_to_linear(-20.0) - 0.1).abs() < 1e-6);
        // 6.020... dB doubles the amplitude.
        assert!((db_to_linear(6.020_6) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn hush_makeup_gain_cancels_every_measured_deficit() {
        // The measured voiced-frame RMS deficits across the corpus in
        // the constant's doc comment (dB versus passthrough). The
        // constant must bring every one of them into the ±1 dB
        // acceptance band around passthrough.
        let measured_deficits_db = [-3.4_f32, -2.5, -2.0, -1.9, -1.5];
        for deficit_db in measured_deficits_db {
            let residual_db = deficit_db + HUSH_MAKEUP_GAIN_DB;
            assert!(
                residual_db.abs() <= 1.0,
                "makeup gain {HUSH_MAKEUP_GAIN_DB} dB leaves {residual_db} dB \
                 residual on the {deficit_db} dB material"
            );
        }
        // The no-clipping-guard design decision: the worst measured raw
        // output peak (0.61 on a 0.70-peak input) must stay under full
        // scale after the gain — overshoot only enters with inputs
        // already at their own clipping point (see the doc comment).
        let gained_peak = 0.61 * db_to_linear(HUSH_MAKEUP_GAIN_DB);
        assert!(
            gained_peak < 1.0,
            "gained measured peak {gained_peak} clips at the measured input level"
        );
    }

    #[test]
    fn apply_gain_scales_every_sample_and_preserves_shape() {
        let mut block = [0.5_f32, -0.25, 0.0, 1.0, -1.0];
        apply_gain(&mut block, db_to_linear(HUSH_MAKEUP_GAIN_DB));
        let factor = db_to_linear(HUSH_MAKEUP_GAIN_DB);
        let expected = [0.5 * factor, -0.25 * factor, 0.0, factor, -factor];
        for (got, want) in block.iter().zip(expected) {
            assert!((got - want).abs() < 1e-6, "expected {want}, got {got}");
        }
    }

    #[test]
    fn unity_gain_is_an_exact_no_op() {
        let original = [0.123_f32, -0.456, 0.789];
        let mut block = original;
        apply_gain(&mut block, 1.0);
        for (got, want) in block.iter().zip(original) {
            assert!(
                got.to_bits() == want.to_bits(),
                "unity gain must be bit-exact: {got} != {want}"
            );
        }
    }

    /// With real weights, the Hush stage output must be exactly the
    /// gain-1.0 stage output scaled by the makeup gain — the gain is a
    /// pure post-multiply, so alignment and residual delay are untouched.
    #[test]
    #[ignore = "requires downloaded model weights (run: noican fetch)"]
    fn hush_stage_output_is_scaled_by_the_makeup_gain() {
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
            return;
        }
        let tarball = crate::fetch::model_dir(&models_dir, spec).join(spec.files[0].name);
        let mut gained = DfTractStage::hush("hush", &tarball).expect("stage should load");
        let params = DfParams::new(tarball).expect("tarball should load");
        let runtime = RuntimeParams::new(1, 0.0, 100.0, -15.0, 35.0, 35.0, ReduceMask::MEAN);
        let mut plain = DfTractStage::from_params("hush-plain", params, runtime, 1.0)
            .expect("stage should load");
        let hop = gained.frame_len();
        let factor = db_to_linear(HUSH_MAKEUP_GAIN_DB);
        let mut gained_out = vec![0.0_f32; hop];
        let mut plain_out = vec![0.0_f32; hop];
        for block in 0..50 {
            #[expect(
                clippy::cast_precision_loss,
                reason = "sample indices fit f32 for a short test signal"
            )]
            let input: Vec<f32> = (0..hop)
                .map(|n| {
                    let t = (block * hop + n) as f32 / 16_000.0;
                    0.4 * (2.0 * std::f32::consts::PI * 220.0 * t).sin()
                })
                .collect();
            gained
                .process_frame(&input, &mut gained_out)
                .expect("processing should succeed");
            plain
                .process_frame(&input, &mut plain_out)
                .expect("processing should succeed");
            for (g, p) in gained_out.iter().zip(&plain_out) {
                assert!(
                    (g - p * factor).abs() < 1e-5,
                    "gained output {g} != plain {p} × {factor}"
                );
            }
        }
    }
}
