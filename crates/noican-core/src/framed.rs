//! Adapter turning a fixed-frame, fixed-rate [`FrameProcessor`] into a
//! [`Stage`] operating at [`ENGINE_SAMPLE_RATE`] with arbitrary block sizes.
//!
//! This is where per-model sample-rate and frame-size differences are
//! absorbed (docs/tech-research.md §12): the engine always exchanges 48 kHz
//! blocks; the adapter resamples (integer factor), accumulates full model
//! frames, and re-emits at the engine rate with a fixed, reported latency.

use std::collections::VecDeque;

use crate::error::StageError;
use crate::resample::{Decimator, Interpolator};
use crate::stage::{ENGINE_SAMPLE_RATE, FrameProcessor, Stage};

/// Wraps a [`FrameProcessor`] as a [`Stage`].
#[derive(Debug)]
pub struct FramedStage<P> {
    processor: P,
    /// Engine-rate samples consumed per processing round.
    round_len: usize,
    resampler: Option<(Decimator, Interpolator)>,
    in_fifo: VecDeque<f32>,
    out_fifo: VecDeque<f32>,
    round_in: Vec<f32>,
    frame_in: Vec<f32>,
    frame_out: Vec<f32>,
    round_out: Vec<f32>,
    latency: usize,
}

impl<P: FrameProcessor> FramedStage<P> {
    /// Wraps `processor`. `max_block_len` is the largest block the engine
    /// will pass to [`Stage::process_block`] (used only to pre-size buffers;
    /// larger blocks still work at the cost of a reallocation).
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Unsupported`] when the processor's sample rate
    /// does not divide [`ENGINE_SAMPLE_RATE`].
    pub fn new(processor: P, max_block_len: usize) -> Result<Self, StageError> {
        let native = processor.sample_rate();
        if native == 0 || !ENGINE_SAMPLE_RATE.is_multiple_of(native) {
            return Err(StageError::Unsupported(format!(
                "sample rate {native} Hz does not divide the {ENGINE_SAMPLE_RATE} Hz engine rate"
            )));
        }
        let factor = (ENGINE_SAMPLE_RATE / native) as usize;
        let frame_len = processor.frame_len();
        let round_len = frame_len * factor;
        let resampler = (factor > 1).then(|| {
            (
                Decimator::new(factor, round_len),
                Interpolator::new(factor, frame_len),
            )
        });
        // Latency: one full round of buffering, plus both filters' group
        // delays when resampling. Priming the output FIFO with `round_len`
        // zeros guarantees process_block can always pop a full block.
        let filter_delay = resampler.as_ref().map_or(0, |(d, i)| {
            d.delay_input_samples() + i.delay_output_samples()
        });
        // Include the model's own algorithmic delay, scaled to engine rate.
        let latency = round_len + filter_delay + processor.output_delay() * factor;
        let mut out_fifo = VecDeque::with_capacity(round_len * 2 + max_block_len);
        out_fifo.extend(std::iter::repeat_n(0.0, round_len));
        Ok(Self {
            processor,
            round_len,
            resampler,
            in_fifo: VecDeque::with_capacity(round_len + max_block_len),
            out_fifo,
            round_in: Vec::with_capacity(round_len),
            frame_in: vec![0.0; frame_len],
            frame_out: vec![0.0; frame_len],
            round_out: Vec::with_capacity(round_len),
            latency,
        })
    }

    /// Access the wrapped processor.
    pub const fn processor(&self) -> &P {
        &self.processor
    }

    /// Mutable access to the wrapped processor (e.g. to update enrollment).
    pub const fn processor_mut(&mut self) -> &mut P {
        &mut self.processor
    }

    fn run_rounds(&mut self) -> Result<(), StageError> {
        while self.in_fifo.len() >= self.round_len {
            self.round_in.clear();
            for _ in 0..self.round_len {
                // Guarded by the length check above.
                if let Some(s) = self.in_fifo.pop_front() {
                    self.round_in.push(s);
                }
            }
            if let Some((decim, interp)) = &mut self.resampler {
                self.frame_in.clear();
                decim.process(&self.round_in, &mut self.frame_in);
                self.frame_out.resize(self.processor.frame_len(), 0.0);
                self.processor
                    .process_frame(&self.frame_in, &mut self.frame_out)?;
                self.round_out.clear();
                interp.process(&self.frame_out, &mut self.round_out);
                self.out_fifo.extend(self.round_out.iter().copied());
            } else {
                self.processor
                    .process_frame(&self.round_in, &mut self.frame_out)?;
                self.out_fifo.extend(self.frame_out.iter().copied());
            }
        }
        Ok(())
    }
}

impl<P: FrameProcessor> Stage for FramedStage<P> {
    fn id(&self) -> &str {
        self.processor.id()
    }

    fn process_block(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        if input.len() != output.len() {
            return Err(StageError::BufferLen {
                expected: input.len(),
                got: output.len(),
            });
        }
        self.in_fifo.extend(input.iter().copied());
        self.run_rounds()?;
        debug_assert!(self.out_fifo.len() >= output.len());
        for sample in output.iter_mut() {
            *sample = self.out_fifo.pop_front().unwrap_or(0.0);
        }
        Ok(())
    }

    fn latency_samples(&self) -> usize {
        self.latency
    }

    fn reset(&mut self) {
        self.processor.reset();
        if let Some((decim, interp)) = &mut self.resampler {
            decim.reset();
            interp.reset();
        }
        self.in_fifo.clear();
        self.out_fifo.clear();
        self.out_fifo
            .extend(std::iter::repeat_n(0.0, self.round_len));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Identity frame processor at an arbitrary rate/frame size.
    #[derive(Debug)]
    struct Identity {
        rate: u32,
        frame: usize,
    }

    impl FrameProcessor for Identity {
        fn id(&self) -> &'static str {
            "identity"
        }
        fn sample_rate(&self) -> u32 {
            self.rate
        }
        fn frame_len(&self) -> usize {
            self.frame
        }
        fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
            output.copy_from_slice(input);
            Ok(())
        }
        fn reset(&mut self) {}
    }

    fn sine(rate: u32, freq: f32, len: usize) -> Vec<f32> {
        #[allow(clippy::cast_precision_loss, reason = "test signal indices are small")]
        (0..len)
            .map(|n| (2.0 * std::f32::consts::PI * freq * n as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    fn run_blocks(stage: &mut dyn Stage, input: &[f32], block: usize) -> Vec<f32> {
        let mut out = vec![0.0; input.len()];
        for (i, o) in input.chunks(block).zip(out.chunks_mut(block)) {
            stage.process_block(i, o).expect("processing failed");
        }
        out
    }

    /// A native-rate identity model must reproduce the input exactly,
    /// delayed by one frame.
    #[test]
    fn native_rate_identity_is_pure_delay() {
        let mut stage = FramedStage::new(
            Identity {
                rate: 48_000,
                frame: 480,
            },
            128,
        )
        .expect("valid config");
        assert_eq!(stage.latency_samples(), 480);
        let input = sine(48_000, 440.0, 4800);
        let out = run_blocks(&mut stage, &input, 128);
        for n in 480..input.len() {
            assert!((out[n] - input[n - 480]).abs() < 1e-6, "mismatch at {n}");
        }
        assert!(out[..480].iter().all(|s| *s == 0.0));
    }

    /// A 16 kHz identity model must reproduce a mid-band sine (within
    /// resampling fidelity) delayed by the reported latency.
    #[test]
    fn resampled_identity_matches_reported_latency() {
        let mut stage = FramedStage::new(
            Identity {
                rate: 16_000,
                frame: 160,
            },
            128,
        )
        .expect("valid config");
        let delay = stage.latency_samples();
        let input = sine(48_000, 1000.0, 19_200);
        let out = run_blocks(&mut stage, &input, 128);
        let mut err = 0.0_f64;
        let mut sig = 0.0_f64;
        for n in delay + 500..input.len() {
            let e = f64::from(out[n]) - f64::from(input[n - delay]);
            err += e * e;
            sig += f64::from(input[n - delay]).powi(2);
        }
        let snr_db = 10.0 * (sig / err).log10();
        assert!(snr_db > 50.0, "SNR too low: {snr_db} dB");
    }

    /// Odd block sizes (not divisible by frame or factor) must still work.
    #[test]
    fn irregular_block_sizes_work() {
        let mut stage = FramedStage::new(
            Identity {
                rate: 16_000,
                frame: 160,
            },
            97,
        )
        .expect("valid config");
        let input = sine(48_000, 350.0, 9700);
        let out = run_blocks(&mut stage, &input, 97);
        assert_eq!(out.len(), input.len());
        assert!(out.iter().all(|s| s.is_finite()));
    }
}
