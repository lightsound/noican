//! Stage for models that take one spectrum frame and return one spectrum frame.
//!
//! Covers three families that differ only in bookkeeping:
//!
//! * **`DPDFNet`** carries every transform parameter in its own ONNX metadata,
//!   including the seed values for the part of its state tensor that holds a
//!   normalisation history. That seed matters: starting it at zero makes the
//!   first second of output audibly wrong while the running averages converge.
//! * **GTCRN** and **UL-UNAS** share an identical signature but no metadata, so
//!   their transform parameters come from the catalog. They were trained with
//!   different windows, which is the entire difference between them here.
//!
//! The ERB analysis and the mask application both live inside these graphs, so
//! all this stage owes them is a transform and its state.

use noican_core::{
    Result as CoreResult, Spectrum, Stage, StageSpec, StftAnalyzer, StftConfig, StftSynthesizer,
    WindowKind,
};

use crate::catalog::SpectralParams;
use crate::error::{Error, Result};
use crate::session::CachedSession;

/// Metadata keys a `DPDFNet` graph is expected to carry.
mod dpdfnet_keys {
    pub(super) const MODEL_TYPE: &str = "model_type";
    pub(super) const SAMPLE_RATE: &str = "sample_rate";
    pub(super) const N_FFT: &str = "n_fft";
    pub(super) const HOP_LENGTH: &str = "hop_length";
    pub(super) const WINDOW_TYPE: &str = "window_type";
    pub(super) const ERB_NORM_STATE_SIZE: &str = "erb_norm_state_size";
    pub(super) const ERB_NORM_INIT: &str = "erb_norm_init";
    pub(super) const SPEC_NORM_INIT: &str = "spec_norm_init";
}

/// A spectrum-in, spectrum-out streaming model.
#[derive(Debug)]
pub struct SpectralStage {
    session: CachedSession,
    spec: StageSpec,
    analyzer: StftAnalyzer,
    synthesizer: StftSynthesizer,
    frame: Spectrum,
    enhanced: Spectrum,
    spec_shape: Vec<i64>,
}

impl SpectralStage {
    /// Loads a `DPDFNet`-style graph, taking every parameter from its metadata.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Metadata`] if a required key is absent or malformed,
    /// [`Error::UnexpectedSignature`] if the graph is not the expected shape, or
    /// [`Error::Runtime`] if it cannot be loaded.
    pub fn load_self_describing(model_id: &str, path: &std::path::Path) -> Result<Self> {
        let mut session = CachedSession::load(model_id, path, 1, 1)?;

        let model_type = session.metadata_string(dpdfnet_keys::MODEL_TYPE)?;
        if model_type != "dpdfnet" {
            return Err(Error::UnexpectedSignature {
                model: model_id.to_owned(),
                detail: format!("expected metadata model_type `dpdfnet`, found `{model_type}`"),
            });
        }
        if session.state_count() != 1 {
            return Err(Error::UnexpectedSignature {
                model: model_id.to_owned(),
                detail: format!(
                    "a dpdfnet graph carries exactly one state tensor, this one has {}",
                    session.state_count()
                ),
            });
        }

        let sample_rate = u32::try_from(session.metadata_usize(dpdfnet_keys::SAMPLE_RATE)?)
            .map_err(|_| Error::Metadata {
                model: model_id.to_owned(),
                key: dpdfnet_keys::SAMPLE_RATE.to_owned(),
            })?;
        let n_fft = session.metadata_usize(dpdfnet_keys::N_FFT)?;
        let hop = session.metadata_usize(dpdfnet_keys::HOP_LENGTH)?;
        let window_name = session.metadata_string(dpdfnet_keys::WINDOW_TYPE)?;
        let window =
            WindowKind::from_onnx_metadata_name(&window_name).ok_or_else(|| Error::Metadata {
                model: model_id.to_owned(),
                key: format!(
                    "{} (`{window_name}` is unsupported)",
                    dpdfnet_keys::WINDOW_TYPE
                ),
            })?;

        // The state tensor begins with the ERB normalisation history, followed
        // by the spectral one, followed by the recurrent state. Only the two
        // histories have non-zero seeds.
        let erb_offset = session.metadata_usize(dpdfnet_keys::ERB_NORM_STATE_SIZE)?;
        let erb_init = session.metadata_floats(dpdfnet_keys::ERB_NORM_INIT)?;
        let spec_init = session.metadata_floats(dpdfnet_keys::SPEC_NORM_INIT)?;
        if erb_init.len() != erb_offset {
            return Err(Error::Metadata {
                model: model_id.to_owned(),
                key: format!(
                    "{} ({} values for a declared size of {erb_offset})",
                    dpdfnet_keys::ERB_NORM_INIT,
                    erb_init.len()
                ),
            });
        }

        let state_size = session.state_shape(0).iter().product::<i64>();
        let state_size = usize::try_from(state_size).map_err(|_| Error::UnexpectedSignature {
            model: model_id.to_owned(),
            detail: format!("state tensor has an unusable element count {state_size}"),
        })?;
        if erb_init.len() + spec_init.len() > state_size {
            return Err(Error::UnexpectedSignature {
                model: model_id.to_owned(),
                detail: format!(
                    "normalisation seeds need {} elements but the state tensor holds {state_size}",
                    erb_init.len() + spec_init.len()
                ),
            });
        }

        let mut initial_state = vec![0.0f32; state_size];
        initial_state[..erb_init.len()].copy_from_slice(&erb_init);
        initial_state[erb_offset..erb_offset + spec_init.len()].copy_from_slice(&spec_init);
        session.set_initial_state(0, initial_state)?;

        // The reference implementation drops the first `2 * n_fft` output
        // samples, which is four hops at these settings; that is the model's
        // algorithmic delay.
        let latency = n_fft * 2;
        Self::assemble(
            session,
            sample_rate,
            StftConfig { n_fft, hop, window },
            latency,
            model_id,
        )
    }

    /// Loads a graph whose transform parameters come from the catalog.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedSignature`] if the graph is not the expected
    /// shape, or [`Error::Runtime`] if it cannot be loaded.
    pub fn load_with_params(
        model_id: &str,
        path: &std::path::Path,
        sample_rate: u32,
        params: SpectralParams,
        latency_samples: usize,
    ) -> Result<Self> {
        let session = CachedSession::load(model_id, path, 1, 1)?;
        Self::assemble(
            session,
            sample_rate,
            StftConfig {
                n_fft: params.n_fft,
                hop: params.hop,
                window: params.window,
            },
            latency_samples,
            model_id,
        )
    }

    fn assemble(
        session: CachedSession,
        sample_rate: u32,
        config: StftConfig,
        latency_samples: usize,
        model_id: &str,
    ) -> Result<Self> {
        let bins = config.bins();
        let spec_shape = session.primary_input_shape(0).to_vec();

        // The graphs differ in axis order — `[1, 1, F, 2]` for DPDFNet,
        // `[1, F, 1, 2]` for GTCRN and UL-UNAS — so rather than assume a
        // layout, check that the bin count appears somewhere and that the
        // trailing axis is the real/imaginary pair. Both hold for every export
        // in the catalog, and a mismatch means the configured transform does
        // not fit this graph.
        let expected_bins = i64::try_from(bins).unwrap_or(i64::MAX);
        if !spec_shape.contains(&expected_bins) || spec_shape.last() != Some(&2) {
            return Err(Error::UnexpectedSignature {
                model: model_id.to_owned(),
                detail: format!(
                    "an n_fft of {} implies {bins} interleaved complex bins, which does not fit \
                     the graph's spectrum shape {spec_shape:?}",
                    config.n_fft
                ),
            });
        }

        Ok(Self {
            spec: StageSpec::streaming(sample_rate, config.hop).with_latency(latency_samples),
            analyzer: StftAnalyzer::new(config)?,
            synthesizer: StftSynthesizer::new(config)?,
            frame: Spectrum::zeroed(bins),
            enhanced: Spectrum::zeroed(bins),
            session,
            spec_shape,
        })
    }
}

impl Stage for SpectralStage {
    fn spec(&self) -> StageSpec {
        self.spec
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) -> CoreResult<()> {
        self.analyzer.process(input, &mut self.frame)?;

        let mut destinations = [self.enhanced.as_interleaved_mut()];
        self.session
            .run(
                &[(self.spec_shape.as_slice(), self.frame.as_interleaved())],
                &mut destinations,
            )
            .map_err(|error| noican_core::Error::Stage(error.to_string()))?;

        self.synthesizer.process(&self.enhanced, output)
    }

    fn reset(&mut self) {
        self.session.reset();
        self.analyzer.reset();
        self.synthesizer.reset();
    }
}
