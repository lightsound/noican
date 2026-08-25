//! Stage for `DeepFilterNet`-family models: `DeepFilterNet3` and Hush.
//!
//! # Why this is a block stage
//!
//! The published exports are *sequence* graphs. Their GRUs take no initial
//! state and return none, so feeding them one frame at a time would reset the
//! recurrence on every frame and produce nonsense. `DeepFilterNet`'s own
//! runtime avoids this by using tract's pulse transformation, which rewrites a
//! sequence graph into a streaming one; ONNX Runtime cannot do that.
//!
//! So the stage consumes a block of frames at once, which is exactly right for
//! offline comparison and costs the block's duration in latency when used live.
//! Each block is preceded by [`CONTEXT_FRAMES`] frames of the previous block,
//! whose output is discarded: without that the recurrence would restart at
//! every block boundary and leave an audible seam.
//!
//! Promoting this to a true streaming stage means re-exporting the graphs with
//! their GRU states as explicit inputs and outputs. That is recorded as Phase 1
//! work in `docs/tech-research.md` §5.5.

use std::path::Path;

use noican_core::{
    Result as CoreResult, Spectrum, Stage, StageCapability, StageSpec, StftAnalyzer, StftConfig,
    StftSynthesizer, WindowKind,
};
use ort::session::Session;
use ort::value::TensorRef;

use crate::dfn::{DfnConfig, DfnFeatures, apply_band_gains, apply_deep_filter};
use crate::error::{Error, Result};

/// Frames of the previous block replayed to warm the recurrence.
///
/// The GRUs settle within a few frames; 32 is comfortably past that and costs
/// well under a third of a block of extra compute.
const CONTEXT_FRAMES: usize = 400;

/// Frames processed per block.
///
/// 100 frames is one second at either model's hop, long enough that the
/// warm-up context is a small overhead and short enough to stay usable.
const BLOCK_FRAMES: usize = 800;

/// A `DeepFilterNet`-family model.
pub struct DeepFilterNetStage {
    model_id: String,
    config: DfnConfig,
    spec: StageSpec,

    encoder: Session,
    erb_decoder: Session,
    df_decoder: Session,

    analyzer: StftAnalyzer,
    synthesizer: StftSynthesizer,
    features: DfnFeatures,

    /// Spectra of the current block, preceded by the warm-up context.
    spectra: Vec<Spectrum>,
    /// Per-frame ERB features, laid out frame-major.
    erb_features: Vec<f32>,
    /// Real parts of the complex feature, frame-major.
    spectral_real: Vec<f32>,
    /// Imaginary parts of the complex feature, frame-major.
    spectral_imaginary: Vec<f32>,
    /// The two above concatenated, which is the layout the encoder wants.
    spectral_scratch: Vec<f32>,
    /// Frames of context carried over from the previous block.
    carried: usize,
    /// Scratch for one enhanced frame.
    enhanced: Spectrum,
}

// `ort::session::Session` is not `Debug`.
impl std::fmt::Debug for DeepFilterNetStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepFilterNetStage")
            .field("model_id", &self.model_id)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl DeepFilterNetStage {
    /// Loads a bundle previously extracted into `directory`.
    ///
    /// The directory must hold `enc.onnx`, `erb_dec.onnx`, `df_dec.onnx`, and
    /// `config.ini`, which is what both published bundles contain.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if a file is missing, [`Error::Metadata`] if the
    /// configuration lacks a key inference needs, or [`Error::Runtime`] if a
    /// graph cannot be loaded.
    pub fn load(model_id: &str, directory: &Path, latency_samples: usize) -> Result<Self> {
        let config_path = directory.join("config.ini");
        let text = std::fs::read_to_string(&config_path).map_err(|source| Error::Io {
            operation: "read",
            path: config_path,
            source,
        })?;
        let config = DfnConfig::parse(model_id, &text)?;

        let stft = StftConfig {
            n_fft: config.n_fft,
            hop: config.hop,
            window: WindowKind::Vorbis,
        };
        let bins = stft.bins();

        Ok(Self {
            encoder: open(directory, "enc.onnx")?,
            erb_decoder: open(directory, "erb_dec.onnx")?,
            df_decoder: open(directory, "df_dec.onnx")?,
            analyzer: StftAnalyzer::new(stft)?,
            synthesizer: StftSynthesizer::new(stft)?,
            features: DfnFeatures::new(&config),
            spectra: Vec::with_capacity(CONTEXT_FRAMES + BLOCK_FRAMES),
            erb_features: Vec::with_capacity((CONTEXT_FRAMES + BLOCK_FRAMES) * config.erb_bands),
            spectral_real: Vec::with_capacity((CONTEXT_FRAMES + BLOCK_FRAMES) * config.df_bins),
            spectral_imaginary: Vec::with_capacity(
                (CONTEXT_FRAMES + BLOCK_FRAMES) * config.df_bins,
            ),
            spectral_scratch: Vec::with_capacity(
                (CONTEXT_FRAMES + BLOCK_FRAMES) * config.df_bins * 2,
            ),
            carried: 0,
            enhanced: Spectrum::zeroed(bins),
            spec: StageSpec::streaming(config.sample_rate, config.hop * BLOCK_FRAMES)
                .with_capability(StageCapability::Block)
                .with_latency(latency_samples),
            model_id: model_id.to_owned(),
            config,
        })
    }

    /// The parsed bundle configuration.
    #[must_use]
    pub const fn config(&self) -> DfnConfig {
        self.config
    }

    /// Runs the three graphs over the frames currently buffered.
    ///
    /// Returns the ERB gains and the deep-filter coefficients, both frame-major.
    fn infer(&mut self) -> Result<(Vec<f32>, Vec<f32>)> {
        let frames = self.spectra.len();
        let bands = self.config.erb_bands;
        let df_bins = self.config.df_bins;

        self.spectral_scratch.clear();
        self.spectral_scratch.extend_from_slice(&self.spectral_real);
        self.spectral_scratch
            .extend_from_slice(&self.spectral_imaginary);

        let frame_count = i64::try_from(frames).unwrap_or(i64::MAX);
        let erb_shape = [1i64, 1, frame_count, i64::try_from(bands).unwrap_or(0)];
        // Real and imaginary parts are separate channels here rather than an
        // interleaved trailing axis. This is the one model family in the catalog
        // that wants that layout, and it is not documented anywhere upstream.
        let spectral_shape = [1i64, 2, frame_count, i64::try_from(df_bins).unwrap_or(0)];

        // Cloned before the encoder borrows `self.encoder`, so the error paths
        // below do not have to reach back into `self`.
        let model_id = self.model_id.clone();
        let order = self.config.df_order;

        let erb = TensorRef::from_array_view((&erb_shape[..], self.erb_features.as_slice()))?;
        let spectral =
            TensorRef::from_array_view((&spectral_shape[..], self.spectral_scratch.as_slice()))?;

        let encoded = self
            .encoder
            .run(ort::inputs!["feat_erb" => erb, "feat_spec" => spectral])?;
        let encoder_output = |name: &str| -> Result<&ort::value::DynValue> {
            encoded.get(name).ok_or_else(|| Error::UnexpectedSignature {
                model: model_id.clone(),
                detail: format!("the encoder produced no output named `{name}`"),
            })
        };

        let mask = self.erb_decoder.run(ort::inputs![
            "emb" => encoder_output("emb")?,
            "e3" => encoder_output("e3")?,
            "e2" => encoder_output("e2")?,
            "e1" => encoder_output("e1")?,
            "e0" => encoder_output("e0")?,
        ])?;
        let (_, gains) = mask[0].try_extract_tensor::<f32>()?;
        let gains = gains.to_vec();
        drop(mask);

        let filtered = self.df_decoder.run(ort::inputs![
            "emb" => encoder_output("emb")?,
            "c0" => encoder_output("c0")?,
        ])?;
        let (_, coefficients) = filtered[0].try_extract_tensor::<f32>()?;
        let coefficients = coefficients.to_vec();

        let expected_gains = frames * bands;
        let expected_coefficients = frames * df_bins * order * 2;
        if gains.len() != expected_gains || coefficients.len() != expected_coefficients {
            return Err(Error::UnexpectedSignature {
                model: model_id,
                detail: format!(
                    "expected {expected_gains} gains and {expected_coefficients} coefficients for \
                     {frames} frames, got {} and {}",
                    gains.len(),
                    coefficients.len()
                ),
            });
        }
        Ok((gains, coefficients))
    }

    /// Enhances the buffered frames and writes the new ones to `output`.
    fn enhance(&mut self, output: &mut [f32]) -> Result<()> {
        let (gains, coefficients) = self.infer()?;
        let bands = self.config.erb_bands;
        let df_bins = self.config.df_bins;
        let order = self.config.df_order;
        let coefficients_per_frame = df_bins * order * 2;
        let inverse_scale = 1.0 / self.config.analysis_scale();
        let band_widths = self.features.band_widths().to_vec();

        // The reference emits the frame `lookahead` behind the newest, which is
        // how the trained lookahead is realised.
        let lookahead = self.config.lookahead();
        let mut written = 0;

        for index in self.carried..self.spectra.len() {
            let target = index.saturating_sub(lookahead);
            self.enhanced.copy_from(&self.spectra[target]);
            apply_band_gains(
                &mut self.enhanced,
                &gains[index * bands..(index + 1) * bands],
                &band_widths,
            );

            // The filter reads the noisy spectra rather than the masked ones. At
            // the very start there are fewer than `order` of them; the missing
            // older frames were silence, which is exactly what the reference's
            // zero-initialised history represents. Skipping the filter instead
            // would leave those frames unsuppressed and far louder than the
            // rest.
            let start = (index + 1).saturating_sub(order);
            apply_deep_filter(
                &mut self.enhanced,
                &self.spectra[start..=index],
                &coefficients[index * coefficients_per_frame..(index + 1) * coefficients_per_frame],
                df_bins,
                order,
            );

            for value in self.enhanced.as_interleaved_mut() {
                *value *= inverse_scale;
            }
            let hop = self.config.hop;
            self.synthesizer
                .process(&self.enhanced, &mut output[written..written + hop])?;
            written += hop;
        }

        debug_assert_eq!(written, output.len());
        Ok(())
    }

    /// Keeps the newest [`CONTEXT_FRAMES`] frames and drops the rest.
    fn retain_context(&mut self) {
        let keep = CONTEXT_FRAMES.min(self.spectra.len());
        let drop_count = self.spectra.len() - keep;

        self.spectra.drain(..drop_count);
        self.erb_features
            .drain(..drop_count * self.config.erb_bands);
        self.spectral_real.drain(..drop_count * self.config.df_bins);
        self.spectral_imaginary
            .drain(..drop_count * self.config.df_bins);
        self.carried = keep;
    }

    /// Appends one frame's spectrum and features.
    fn push_frame(&mut self, frame: &Spectrum) {
        let bands = self.config.erb_bands;
        let df_bins = self.config.df_bins;

        let mut erb = vec![0.0f32; bands];
        let mut real = vec![0.0f32; df_bins];
        let mut imaginary = vec![0.0f32; df_bins];

        let mut scaled = frame.clone();
        let scale = self.config.analysis_scale();
        for value in scaled.as_interleaved_mut() {
            *value *= scale;
        }

        self.features.erb_feature(&scaled, &mut erb);
        self.features
            .spectral_feature(&scaled, &mut real, &mut imaginary);

        self.spectra.push(scaled);
        self.erb_features.extend_from_slice(&erb);
        self.spectral_real.extend_from_slice(&real);
        self.spectral_imaginary.extend_from_slice(&imaginary);
    }
}

impl Stage for DeepFilterNetStage {
    fn spec(&self) -> StageSpec {
        self.spec
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) -> CoreResult<()> {
        if input.len() != self.spec.block_size || output.len() != self.spec.block_size {
            return Err(noican_core::Error::BufferLength {
                expected: self.spec.block_size,
                actual: input.len().min(output.len()),
            });
        }

        let mut frame = Spectrum::zeroed(self.config.n_fft / 2 + 1);
        for block in input.chunks_exact(self.config.hop) {
            self.analyzer.process(block, &mut frame)?;
            self.push_frame(&frame);
        }

        self.enhance(output)
            .map_err(|error| noican_core::Error::Stage(error.to_string()))?;
        self.retain_context();
        Ok(())
    }

    fn reset(&mut self) {
        self.analyzer.reset();
        self.synthesizer.reset();
        self.features.reset();
        self.spectra.clear();
        self.erb_features.clear();
        self.spectral_real.clear();
        self.spectral_imaginary.clear();
        self.spectral_scratch.clear();
        self.carried = 0;
    }
}

/// Opens one graph of a bundle, single-threaded like the rest of the catalog.
fn open(directory: &Path, file_name: &str) -> Result<Session> {
    let path = directory.join(file_name);
    Ok(Session::builder()?
        .with_intra_threads(1)
        .map_err(ort::Error::from)?
        .commit_from_file(path)?)
}
