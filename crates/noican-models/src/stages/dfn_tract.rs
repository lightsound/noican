//! See docs/adr/0008-model-runtime.md#deepfilternet-tract-runtime.

use std::path::Path;

use df::tract::{DfParams, DfTract, ReduceMask, RuntimeParams};
use noican_core::{FrameProcessor, StageError};

pub(crate) const HUSH_MAKEUP_GAIN_DB: f32 = 2.45;

const SILENT_FRAME_MEAN_SQUARE: f32 = 1e-7;

pub(crate) fn db_to_linear(gain_db: f32) -> f32 {
    10.0_f32.powf(gain_db / 20.0)
}

pub(crate) fn apply_gain(samples: &mut [f32], linear_gain: f32) {
    for sample in samples {
        *sample *= linear_gain;
    }
}

fn mean_square(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "frame lengths are a few hundred samples"
    )]
    let len = samples.len() as f32;
    samples.iter().map(|s| s * s).sum::<f32>() / len
}

fn magnitude_ratio(noisy_energy: f32, enhanced_energy: f32) -> f32 {
    if noisy_energy <= f32::MIN_POSITIVE {
        return 0.0;
    }
    (enhanced_energy / noisy_energy).sqrt().clamp(0.0, 1.0)
}

fn band_energy(
    spectrum: ndarray::ArrayView1<'_, df::Complex32>,
    bins: &std::ops::Range<usize>,
) -> f32 {
    spectrum
        .iter()
        .skip(bins.start)
        .take(bins.len())
        .map(|c| c.re.mul_add(c.re, c.im * c.im))
        .sum()
}

struct SendModel(DfTract);

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
    output_gain: f32,
    params: DfParams,
    runtime: RuntimeParams,
    last_frame_silent: bool,
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
    /// See docs/adr/0008-model-runtime.md#deepfilternet-tract-runtime.
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

    /// See docs/adr/0008-model-runtime.md#deepfilternet-tract-runtime.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] when the tarball cannot be loaded.
    pub fn deepfilternet3_ll(id: &str, tarball: &Path) -> Result<Self, StageError> {
        let params = DfParams::new(tarball.to_path_buf())
            .map_err(|e| StageError::Inference(format!("loading {}: {e}", tarball.display())))?;
        Self::from_params(id, params, RuntimeParams::default_with_ch(1), 1.0)
    }

    /// See docs/adr/0008-model-runtime.md#deepfilternet-tract-runtime.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] when the tarball cannot be loaded.
    pub fn hush(id: &str, tarball: &Path) -> Result<Self, StageError> {
        Self::hush_with_gain(id, tarball, db_to_linear(HUSH_MAKEUP_GAIN_DB))
    }

    /// See docs/adr/0007-dsp-stages.md#hush-makeup-gain.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] when the tarball cannot be loaded.
    pub fn hush_unity_gain(id: &str, tarball: &Path) -> Result<Self, StageError> {
        Self::hush_with_gain(id, tarball, 1.0)
    }

    fn hush_with_gain(id: &str, tarball: &Path, output_gain: f32) -> Result<Self, StageError> {
        let params = DfParams::new(tarball.to_path_buf())
            .map_err(|e| StageError::Inference(format!("loading {}: {e}", tarball.display())))?;
        let runtime = RuntimeParams::new(1, 0.0, 100.0, -15.0, 35.0, 35.0, ReduceMask::MEAN);
        Self::from_params(id, params, runtime, output_gain)
    }

    pub(crate) const fn n_freqs(&self) -> usize {
        self.model.0.n_freqs
    }

    pub(crate) fn band_gain(&self, bins: std::ops::Range<usize>) -> f32 {
        assert!(bins.end <= self.n_freqs(), "bin range past the spectrum");
        if self.last_frame_silent {
            return 0.0;
        }
        let noisy = band_energy(self.model.0.get_spec_noisy().row(0), &bins);
        let enhanced = band_energy(self.model.0.get_spec_enh().row(0), &bins);
        magnitude_ratio(noisy, enhanced)
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
            last_frame_silent: true,
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
        self.last_frame_silent = mean_square(input) < SILENT_FRAME_MEAN_SQUARE;
        let noisy = ndarray::ArrayView2::from_shape((1, self.hop), input)
            .map_err(|e| StageError::Inference(format!("bad input shape: {e}")))?;
        let enh = ndarray::ArrayViewMut2::from_shape((1, self.hop), output)
            .map_err(|e| StageError::Inference(format!("bad output shape: {e}")))?;
        self.model
            .0
            .process(noisy, enh)
            .map_err(|e| StageError::Inference(format!("DfTract process failed: {e}")))?;
        if (self.output_gain - 1.0).abs() > f32::EPSILON {
            apply_gain(output, self.output_gain);
        }
        Ok(())
    }

    fn reset(&mut self) {
        if let Ok(model) = build(&self.params, &self.runtime) {
            self.model = SendModel(model);
        }
        self.last_frame_silent = true;
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
        assert!((db_to_linear(6.020_6) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn hush_makeup_gain_cancels_every_measured_deficit() {
        let measured_deficits_db = [-3.4_f32, -2.5, -2.0, -1.9, -1.5];
        for deficit_db in measured_deficits_db {
            let residual_db = deficit_db + HUSH_MAKEUP_GAIN_DB;
            assert!(
                residual_db.abs() <= 1.0,
                "makeup gain {HUSH_MAKEUP_GAIN_DB} dB leaves {residual_db} dB \
                 residual on the {deficit_db} dB material"
            );
        }
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
    fn mean_square_matches_upstream_silence_criterion() {
        assert!(mean_square(&[]) < SILENT_FRAME_MEAN_SQUARE);
        assert!(mean_square(&[1e-4; 160]) < SILENT_FRAME_MEAN_SQUARE);
        assert!(mean_square(&[1e-3; 160]) >= SILENT_FRAME_MEAN_SQUARE);
        assert!((mean_square(&[0.5, -0.5]) - 0.25).abs() < 1e-7);
    }

    #[test]
    fn magnitude_ratio_is_the_clamped_amplitude_ratio() {
        assert!((magnitude_ratio(4.0, 1.0) - 0.5).abs() < 1e-6);
        assert!((magnitude_ratio(1.0, 1.0) - 1.0).abs() < 1e-6);
        assert!((magnitude_ratio(1.0, 1.5) - 1.0).abs() < 1e-6);
        assert!(magnitude_ratio(0.0, 1.0).abs() < 1e-6);
        assert!(magnitude_ratio(1.0, 0.0).abs() < 1e-6);
    }

    #[test]
    fn band_energy_sums_only_the_requested_bins() {
        let spectrum: Vec<df::Complex32> = (0..8)
            .map(|k| df::Complex32::new(1.0, if k >= 4 { 1.0 } else { 0.0 }))
            .collect();
        let view = ndarray::ArrayView1::from(&spectrum);
        assert!((band_energy(view, &(0..4)) - 4.0).abs() < 1e-6);
        assert!((band_energy(view, &(4..8)) - 8.0).abs() < 1e-6);
        assert!(band_energy(view, &(3..3)).abs() < 1e-6);
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
        assert!(
            crate::fetch::is_fetched(&models_dir, spec),
            "hush: weights not fetched under {dir} (run: noican --models-dir {dir} fetch hush)",
            dir = models_dir.display()
        );
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
