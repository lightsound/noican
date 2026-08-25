//! DPDFNet 48 kHz HR streaming stage.
//!
//! The graph consumes one raw complex STFT frame (`spec [1, 1, bins, 2]`)
//! and returns the enhanced frame plus an opaque flat state vector; ERB
//! features, normalization, masking, and deep filtering all live inside
//! the graph. Outside the graph (mirroring sherpa-onnx's
//! `online-speech-denoiser-stft-impl.h` and ceva-ip's `stream.py`):
//! sliding analysis window → analysis window function → unnormalized rfft
//! → run graph → irfft (scaled 1/n) → synthesis window (same window) →
//! 50% overlap-add, emitting `hop` samples per call. The window satisfies
//! squared-COLA, so no OLA normalization is needed. All STFT parameters
//! and the state-vector initialization come from the ONNX metadata.

use std::borrow::Cow;
use std::path::Path;

use noican_core::{FrameProcessor, StageError};
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;

use crate::dsp::{FftPair, sqrt_hann_window, vorbis_window};
use crate::onnx::{inference_error, load_streaming_session, required_metadata};

fn parse_meta_usize(session: &Session, key: &str) -> Result<usize, StageError> {
    required_metadata(session, key)?
        .trim()
        .parse()
        .map_err(|e| StageError::Inference(format!("bad metadata {key}: {e}")))
}

fn parse_meta_floats(session: &Session, key: &str) -> Result<Vec<f32>, StageError> {
    required_metadata(session, key)?
        .split(',')
        .map(|v| {
            v.trim()
                .parse::<f32>()
                .map_err(|e| StageError::Inference(format!("bad metadata {key}: {e}")))
        })
        .collect()
}

/// DPDFNet streaming frame processor.
#[derive(Debug)]
pub struct DpdfnetStage {
    id: String,
    session: Session,
    sample_rate: u32,
    n_fft: usize,
    hop: usize,
    window: Vec<f32>,
    fft: FftPair,
    /// Sliding analysis buffer, length `window_length`.
    analysis: Vec<f32>,
    /// Overlap-add buffer, length `window_length`.
    ola: Vec<f32>,
    spec: Vec<f32>,
    frame: Vec<f32>,
    state: Vec<f32>,
    init_state: Vec<f32>,
}

impl DpdfnetStage {
    /// Loads a DPDFNet streaming ONNX at `path`, reading all STFT and
    /// state parameters from its embedded metadata.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] on load failure, missing
    /// metadata, or an unsupported window type.
    pub fn new(id: &str, path: &Path) -> Result<Self, StageError> {
        let session = load_streaming_session(path)?;
        let sample_rate = u32::try_from(parse_meta_usize(&session, "sample_rate")?)
            .map_err(|_| StageError::Inference("bad sample_rate metadata".to_owned()))?;
        let n_fft = parse_meta_usize(&session, "n_fft")?;
        let hop = parse_meta_usize(&session, "hop_length")?;
        let win_len = parse_meta_usize(&session, "window_length")?;
        if win_len != n_fft {
            return Err(StageError::Unsupported(format!(
                "window_length {win_len} != n_fft {n_fft} is not supported"
            )));
        }
        let window_type = required_metadata(&session, "window_type")?;
        let window = match window_type.trim() {
            "vorbis" => vorbis_window(win_len),
            "hann_sqrt" => sqrt_hann_window(win_len),
            other => {
                return Err(StageError::Unsupported(format!(
                    "unknown window_type: {other}"
                )));
            }
        };
        let state_size = parse_meta_usize(&session, "state_size")?;
        let erb_init = parse_meta_floats(&session, "erb_norm_init")?;
        let spec_init = parse_meta_floats(&session, "spec_norm_init")?;
        let erb_len = parse_meta_usize(&session, "erb_norm_state_size")?;
        let spec_len = parse_meta_usize(&session, "spec_norm_state_size")?;
        if erb_init.len() != erb_len || spec_init.len() != spec_len {
            return Err(StageError::Inference(
                "state init vectors do not match their declared sizes".to_owned(),
            ));
        }
        let mut init_state = vec![0.0_f32; state_size];
        init_state[..erb_len].copy_from_slice(&erb_init);
        init_state[erb_len..erb_len + spec_len].copy_from_slice(&spec_init);
        let fft = FftPair::new(n_fft);
        let bins = fft.bins();
        Ok(Self {
            id: id.to_owned(),
            session,
            sample_rate,
            n_fft,
            hop,
            window,
            analysis: vec![0.0; win_len],
            ola: vec![0.0; win_len],
            spec: vec![0.0; 2 * bins],
            frame: vec![0.0; n_fft],
            state: init_state.clone(),
            init_state,
            fft,
        })
    }
}

impl FrameProcessor for DpdfnetStage {
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
        // One analysis window of overlap-add delay.
        self.n_fft - self.hop
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        let win_len = self.analysis.len();
        // Slide the analysis window and append the new hop.
        self.analysis.copy_within(self.hop.., 0);
        self.analysis[win_len - self.hop..].copy_from_slice(input);

        // Analysis window + forward FFT.
        for (dst, (x, w)) in self
            .frame
            .iter_mut()
            .zip(self.analysis.iter().zip(&self.window))
        {
            *dst = x * w;
        }
        let mut spec = std::mem::take(&mut self.spec);
        self.fft.forward_interleaved(&self.frame, &mut spec)?;

        let bins = self.fft.bins();
        let spec_tensor = Tensor::from_array(([1, 1, bins, 2], spec.clone()))
            .map_err(|e| inference_error("building spec tensor", &e))?;
        let state_tensor = Tensor::from_array(([self.state.len()], self.state.clone()))
            .map_err(|e| inference_error("building state tensor", &e))?;
        let inputs: Vec<(Cow<'static, str>, SessionInputValue<'static>)> = vec![
            (Cow::Borrowed("spec"), spec_tensor.into()),
            (Cow::Borrowed("state_in"), state_tensor.into()),
        ];
        let outputs = self
            .session
            .run(inputs)
            .map_err(|e| inference_error("DPDFNet inference", &e))?;

        let (_, enhanced) = outputs
            .get("spec_e")
            .ok_or_else(|| StageError::Inference("spec_e missing".to_owned()))?
            .try_extract_tensor::<f32>()
            .map_err(|e| inference_error("extracting spec_e", &e))?;
        if enhanced.len() != spec.len() {
            return Err(StageError::BufferLen {
                expected: spec.len(),
                got: enhanced.len(),
            });
        }
        spec.copy_from_slice(enhanced);

        let (_, next_state) = outputs
            .get("state_out")
            .ok_or_else(|| StageError::Inference("state_out missing".to_owned()))?
            .try_extract_tensor::<f32>()
            .map_err(|e| inference_error("extracting state_out", &e))?;
        if next_state.len() != self.state.len() {
            return Err(StageError::BufferLen {
                expected: self.state.len(),
                got: next_state.len(),
            });
        }
        self.state.copy_from_slice(next_state);

        // Inverse FFT + synthesis window + overlap-add.
        self.fft.inverse_interleaved(&spec, &mut self.frame)?;
        self.spec = spec;
        for (acc, (y, w)) in self.ola.iter_mut().zip(self.frame.iter().zip(&self.window)) {
            *acc += y * w;
        }
        output.copy_from_slice(&self.ola[..self.hop]);
        self.ola.copy_within(self.hop.., 0);
        self.ola[win_len - self.hop..].fill(0.0);
        Ok(())
    }

    fn reset(&mut self) {
        self.state.copy_from_slice(&self.init_state);
        self.analysis.fill(0.0);
        self.ola.fill(0.0);
    }
}
