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
        Self::from_params(id, DfParams::default(), RuntimeParams::default_with_ch(1))
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
        Self::from_params(id, params, runtime)
    }

    fn from_params(id: &str, params: DfParams, runtime: RuntimeParams) -> Result<Self, StageError> {
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
