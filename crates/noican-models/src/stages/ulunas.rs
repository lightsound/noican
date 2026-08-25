//! UL-UNAS 16 kHz streaming stage.
//!
//! The graph consumes one complex STFT frame (`mix [1, 257, 1, 2]`) and
//! returns the enhanced complex frame plus three opaque cache blobs. The
//! STFT/iSTFT is computed outside the graph (verified against the repo's
//! `ulunas_stream.py`, which reproduces the shipped enhanced sample to
//! 1.3e-4): `n_fft` = 512, hop = 256, periodic Hann analysis and synthesis
//! windows, overlap-add divided by the overlapped squared-window sum.

use std::borrow::Cow;
use std::path::Path;

use noican_core::{FrameProcessor, StageError};
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;

use crate::dsp::{FftPair, periodic_hann_window};
use crate::onnx::{StateBank, inference_error, load_streaming_session};

const SAMPLE_RATE: u32 = 16_000;
const N_FFT: usize = 512;
const HOP: usize = 256;

/// UL-UNAS streaming frame processor.
#[derive(Debug)]
pub struct UlunasStage {
    id: String,
    session: Session,
    window: Vec<f32>,
    /// Steady-state OLA denominator per hop sample:
    /// `w²[i] + w²[i + hop]`.
    wsum: Vec<f32>,
    fft: FftPair,
    analysis: Vec<f32>,
    ola: Vec<f32>,
    spec: Vec<f32>,
    frame: Vec<f32>,
    states: StateBank,
}

impl UlunasStage {
    /// Loads the UL-UNAS streaming ONNX at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] when the file cannot be loaded or
    /// does not expose the expected tensors.
    pub fn new(id: &str, path: &Path) -> Result<Self, StageError> {
        let session = load_streaming_session(path)?;
        let states = StateBank::from_pairs(
            &session,
            &[
                ("conv_cache", "conv_cache_out"),
                ("tfa_cache", "tfa_cache_out"),
                ("inter_cache", "inter_cache_out"),
            ],
        )?;
        let window = periodic_hann_window(N_FFT);
        let wsum = (0..HOP)
            .map(|i| {
                let a = window[i];
                let b = window[i + HOP];
                a.mul_add(a, b * b).max(1e-8)
            })
            .collect();
        let fft = FftPair::new(N_FFT);
        let bins = fft.bins();
        Ok(Self {
            id: id.to_owned(),
            session,
            window,
            wsum,
            analysis: vec![0.0; N_FFT],
            ola: vec![0.0; N_FFT],
            spec: vec![0.0; 2 * bins],
            frame: vec![0.0; N_FFT],
            states,
            fft,
        })
    }
}

impl FrameProcessor for UlunasStage {
    fn id(&self) -> &str {
        &self.id
    }

    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    fn frame_len(&self) -> usize {
        HOP
    }

    fn output_delay(&self) -> usize {
        N_FFT - HOP
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        // Slide the analysis window and append the new hop.
        self.analysis.copy_within(HOP.., 0);
        self.analysis[N_FFT - HOP..].copy_from_slice(input);

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
        let mix = Tensor::from_array(([1, bins, 1, 2], spec.clone()))
            .map_err(|e| inference_error("building mix tensor", &e))?;
        let mut inputs: Vec<(Cow<'static, str>, SessionInputValue<'static>)> =
            vec![(Cow::Borrowed("mix"), mix.into())];
        self.states.append_inputs(&mut inputs)?;
        let outputs = self
            .session
            .run(inputs)
            .map_err(|e| inference_error("UL-UNAS inference", &e))?;

        let (_, enhanced) = outputs
            .get("enh")
            .ok_or_else(|| StageError::Inference("enh missing".to_owned()))?
            .try_extract_tensor::<f32>()
            .map_err(|e| inference_error("extracting enh", &e))?;
        if enhanced.len() != spec.len() {
            return Err(StageError::BufferLen {
                expected: spec.len(),
                got: enhanced.len(),
            });
        }
        spec.copy_from_slice(enhanced);
        self.states.update_from_outputs(&outputs)?;

        // Inverse FFT + synthesis window + normalized overlap-add.
        self.fft.inverse_interleaved(&spec, &mut self.frame)?;
        self.spec = spec;
        for (acc, (y, w)) in self.ola.iter_mut().zip(self.frame.iter().zip(&self.window)) {
            *acc += y * w;
        }
        for (out, (acc, wsum)) in output.iter_mut().zip(self.ola.iter().zip(&self.wsum)) {
            *out = acc / wsum;
        }
        self.ola.copy_within(HOP.., 0);
        self.ola[N_FFT - HOP..].fill(0.0);
        Ok(())
    }

    fn reset(&mut self) {
        self.states.reset();
        self.analysis.fill(0.0);
        self.ola.fill(0.0);
    }
}
