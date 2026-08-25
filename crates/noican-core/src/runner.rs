//! Adapts a [`Stage`] to the host's sample rate and block size.
//!
//! Every model has its own native rate (16 kHz or 48 kHz) and its own block
//! size (480, 512, or a whole second), while the host audio path delivers
//! whatever the device's buffer size happens to be at a fixed 48 kHz. The
//! runner absorbs both mismatches so that callers — the real-time engine and
//! the offline CLI alike — only ever see "N samples in, N samples out at the
//! host rate".

use crate::error::{Error, Result};
use crate::resample::RationalResampler;
use crate::ring::SampleQueue;
use crate::stage::{Stage, StageSpec};

/// Extra samples of priming beyond the strict minimum, to absorb the
/// ±1-sample rounding each resampler can contribute.
const PRIMING_MARGIN: usize = 8;

/// Runs one stage at the host rate and block size.
pub struct StageRunner {
    stage: Box<dyn Stage>,
    spec: StageSpec,
    host_rate: u32,
    max_host_block: usize,

    to_stage: RationalResampler,
    from_stage: RationalResampler,

    /// Stage-rate samples waiting to fill a block.
    pending_input: SampleQueue,
    /// Host-rate samples ready to be emitted.
    ready_output: SampleQueue,

    stage_input: Box<[f32]>,
    stage_output: Box<[f32]>,
    downsampled: Box<[f32]>,
    upsampled: Box<[f32]>,

    latency_samples: usize,
    underruns: u64,
}

// `Stage` deliberately does not require `Debug`, so that implementations are
// free to hold types (such as inference sessions) that do not provide it.
impl std::fmt::Debug for StageRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StageRunner")
            .field("stage_spec", &self.spec)
            .field("host_rate", &self.host_rate)
            .field("max_host_block", &self.max_host_block)
            .field("latency_samples", &self.latency_samples)
            .field("underruns", &self.underruns)
            .finish_non_exhaustive()
    }
}

impl StageRunner {
    /// Wraps `stage` for use at `host_rate` with host blocks of at most
    /// `max_host_block` samples.
    ///
    /// # Errors
    ///
    /// Returns an error if the rate ratio cannot be represented or if
    /// `max_host_block` is zero.
    pub fn new(stage: Box<dyn Stage>, host_rate: u32, max_host_block: usize) -> Result<Self> {
        if max_host_block == 0 {
            return Err(Error::InvalidConfiguration(
                "max_host_block must be non-zero".to_owned(),
            ));
        }

        let spec = stage.spec();
        if spec.block_size == 0 {
            return Err(Error::InvalidConfiguration(
                "stage block size must be non-zero".to_owned(),
            ));
        }

        let to_stage = RationalResampler::new(host_rate, spec.sample_rate)?;
        let from_stage = RationalResampler::new(spec.sample_rate, host_rate)?;

        let block_in_host = scale_rate(spec.block_size, spec.sample_rate, host_rate);
        let priming = block_in_host + PRIMING_MARGIN;

        // Worst case the queues hold: a whole host block's worth of stage
        // samples on top of an almost-complete block, and the priming plus one
        // block's output.
        let pending_capacity =
            spec.block_size + to_stage.max_output_len(max_host_block) + PRIMING_MARGIN;
        let ready_capacity = priming + block_in_host + max_host_block + PRIMING_MARGIN;

        // Both resampler delays are already expressed at the host rate: the
        // downward converter's in its input rate, the upward one's in its
        // output rate.
        let latency_samples = priming
            + to_stage.group_delay_input_samples()
            + scale_rate(spec.latency_samples, spec.sample_rate, host_rate)
            + from_stage.group_delay_output_samples();

        let mut ready_output = SampleQueue::new(ready_capacity);
        ready_output.push_silence(priming);

        Ok(Self {
            downsampled: vec![0.0; to_stage.max_output_len(max_host_block)].into_boxed_slice(),
            upsampled: vec![0.0; from_stage.max_output_len(spec.block_size)].into_boxed_slice(),
            stage_input: vec![0.0; spec.block_size].into_boxed_slice(),
            stage_output: vec![0.0; spec.block_size].into_boxed_slice(),
            pending_input: SampleQueue::new(pending_capacity),
            ready_output,
            to_stage,
            from_stage,
            stage,
            spec,
            host_rate,
            max_host_block,
            latency_samples,
            underruns: 0,
        })
    }

    /// The wrapped stage's own specification.
    #[must_use]
    pub const fn stage_spec(&self) -> StageSpec {
        self.spec
    }

    /// Host sample rate this runner operates at.
    #[must_use]
    pub const fn host_rate(&self) -> u32 {
        self.host_rate
    }

    /// Largest host block this runner accepts.
    #[must_use]
    pub const fn max_host_block(&self) -> usize {
        self.max_host_block
    }

    /// End-to-end delay from input to output, in host-rate samples.
    ///
    /// This is the sum of the two resamplers' group delays, the model's own
    /// algorithmic delay, and the priming needed to guarantee that a host block
    /// always produces a full host block.
    #[must_use]
    pub const fn latency_samples(&self) -> usize {
        self.latency_samples
    }

    /// End-to-end delay in milliseconds.
    #[must_use]
    pub fn latency_ms(&self) -> f32 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "latencies and sample rates are far below f32's exact-integer limit"
        )]
        let ms = (self.latency_samples as f32 / self.host_rate as f32) * 1000.0;
        ms
    }

    /// How many times the output queue ran dry and had to emit silence.
    ///
    /// Should stay at zero; a non-zero value means the priming estimate was too
    /// small for this rate ratio and is worth investigating.
    #[must_use]
    pub const fn underruns(&self) -> u64 {
        self.underruns
    }

    /// Returns the wrapped stage, consuming the runner.
    #[must_use]
    pub fn into_stage(self) -> Box<dyn Stage> {
        self.stage
    }

    /// Resets the stage, both resamplers, and all queues.
    pub fn reset(&mut self) {
        self.stage.reset();
        self.to_stage.reset();
        self.from_stage.reset();
        self.pending_input.clear();
        self.ready_output.clear();
        let priming = scale_rate(self.spec.block_size, self.spec.sample_rate, self.host_rate)
            + PRIMING_MARGIN;
        self.ready_output.push_silence(priming);
        self.underruns = 0;
    }

    /// Processes one host block.
    ///
    /// `input` and `output` must be the same length, which must not exceed
    /// [`Self::max_host_block`]. Real-time safe: no allocation, no locks, no
    /// I/O.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BufferLength`] on a length mismatch, or whatever the
    /// wrapped stage returns.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<()> {
        if input.len() != output.len() {
            return Err(Error::BufferLength {
                expected: input.len(),
                actual: output.len(),
            });
        }
        if input.len() > self.max_host_block {
            return Err(Error::BufferLength {
                expected: self.max_host_block,
                actual: input.len(),
            });
        }

        let produced = self.to_stage.process(input, &mut self.downsampled)?;
        self.pending_input.push(&self.downsampled[..produced]);

        while self.pending_input.len() >= self.spec.block_size {
            self.pending_input.pop(&mut self.stage_input);
            self.stage
                .process(&self.stage_input, &mut self.stage_output)?;
            let host_samples = self
                .from_stage
                .process(&self.stage_output, &mut self.upsampled)?;
            self.ready_output.push(&self.upsampled[..host_samples]);
        }

        let taken = self.ready_output.pop(output);
        if taken < output.len() {
            output[taken..].fill(0.0);
            self.underruns += 1;
        }
        Ok(())
    }
}

/// Converts a sample count from `from_rate` to `to_rate`, rounding to nearest.
///
/// Integer arithmetic throughout, so the value the runner reports as its
/// latency is exactly the value a caller can subtract to realign audio.
fn scale_rate(samples: usize, from_rate: u32, to_rate: u32) -> usize {
    if from_rate == to_rate {
        return samples;
    }
    let Ok(samples) = u64::try_from(samples) else {
        return usize::MAX;
    };
    let numerator = samples
        .saturating_mul(u64::from(to_rate))
        .saturating_add(u64::from(from_rate) / 2);
    usize::try_from(numerator / u64::from(from_rate)).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::StageRunner;
    use crate::error::Result;
    use crate::stage::{Passthrough, Stage, StageSpec};

    /// A stage that multiplies by two, so we can see it ran.
    #[derive(Debug)]
    struct Gain {
        spec: StageSpec,
        factor: f32,
        resets: usize,
    }

    impl Stage for Gain {
        fn spec(&self) -> StageSpec {
            self.spec
        }

        fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<()> {
            for (dst, &src) in output.iter_mut().zip(input) {
                *dst = src * self.factor;
            }
            Ok(())
        }

        fn reset(&mut self) {
            self.resets += 1;
        }
    }

    fn sine(rate: f32, freq: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| {
                #[expect(clippy::cast_precision_loss, reason = "test fixture")]
                let t = i as f32 / rate;
                0.5 * (2.0 * core::f32::consts::PI * freq * t).sin()
            })
            .collect()
    }

    fn run(runner: &mut StageRunner, input: &[f32], block: usize) -> Vec<f32> {
        let mut output = Vec::with_capacity(input.len());
        let mut scratch = vec![0.0; block];
        for chunk in input.chunks(block) {
            runner.process(chunk, &mut scratch[..chunk.len()]).unwrap();
            output.extend_from_slice(&scratch[..chunk.len()]);
        }
        output
    }

    #[test]
    fn same_rate_passthrough_is_delayed_by_the_reported_latency() {
        let stage = Box::new(Passthrough::new(48_000, 480));
        let mut runner = StageRunner::new(stage, 48_000, 512).unwrap();
        let delay = runner.latency_samples();

        let input = sine(48_000.0, 440.0, 48_000);
        let output = run(&mut runner, &input, 128);

        assert_eq!(runner.underruns(), 0);
        for i in 0..24_000 {
            let expected = input[i];
            let actual = output[i + delay];
            assert!(
                (expected - actual).abs() < 1e-6,
                "sample {i}: expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn resampled_stage_is_delayed_by_the_reported_latency() {
        let stage = Box::new(Passthrough::new(16_000, 160));
        let mut runner = StageRunner::new(stage, 48_000, 512).unwrap();
        let delay = runner.latency_samples();

        let input = sine(48_000.0, 440.0, 96_000);
        let output = run(&mut runner, &input, 128);

        assert_eq!(runner.underruns(), 0);
        // A trip through 16 kHz is not bit-exact; check the residual instead.
        let compared = 24_000;
        #[expect(clippy::cast_precision_loss, reason = "test fixture")]
        let error = (0..compared)
            .map(|i| (output[i + delay] - input[i]).powi(2))
            .sum::<f32>()
            / compared as f32;
        assert!(error < 1e-5, "mean squared error = {error}");
    }

    #[test]
    fn the_stage_actually_runs() {
        let stage = Box::new(Gain {
            spec: StageSpec::streaming(48_000, 480),
            factor: 2.0,
            resets: 0,
        });
        let mut runner = StageRunner::new(stage, 48_000, 480).unwrap();
        let delay = runner.latency_samples();
        let input = vec![0.25; 48_000];
        let output = run(&mut runner, &input, 480);
        assert!((output[delay + 1_000] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn block_stage_with_a_large_block_still_matches_block_counts() {
        // A one-second block at 16 kHz driven by 128-sample host blocks.
        let stage = Box::new(Passthrough::new(16_000, 16_000));
        let mut runner = StageRunner::new(stage, 48_000, 128).unwrap();
        let output = run(&mut runner, &vec![0.1; 240_000], 128);
        assert_eq!(output.len(), 240_000);
        assert_eq!(runner.underruns(), 0);
    }

    #[test]
    fn varying_host_block_sizes_never_underrun() {
        let stage = Box::new(Passthrough::new(16_000, 512));
        let mut runner = StageRunner::new(stage, 48_000, 1_024).unwrap();
        let input = sine(48_000.0, 1_000.0, 96_000);
        let mut scratch = vec![0.0; 1_024];
        let mut offset = 0;
        let mut sizes = [64usize, 1_024, 128, 7, 511, 256].into_iter().cycle();
        while offset < input.len() {
            let size = sizes.next().unwrap().min(input.len() - offset);
            runner
                .process(&input[offset..offset + size], &mut scratch[..size])
                .unwrap();
            offset += size;
        }
        assert_eq!(runner.underruns(), 0);
    }

    #[test]
    fn reset_restores_the_priming() {
        let stage = Box::new(Passthrough::new(48_000, 480));
        let mut runner = StageRunner::new(stage, 48_000, 480).unwrap();
        run(&mut runner, &vec![1.0; 4_800], 480);
        runner.reset();
        let output = run(&mut runner, &vec![0.0; 4_800], 480);
        assert!(output.iter().all(|s| s.abs() < 1e-9));
        assert_eq!(runner.underruns(), 0);
    }

    #[test]
    fn rejects_bad_arguments() {
        let stage = Box::new(Passthrough::new(48_000, 480));
        assert!(StageRunner::new(stage, 48_000, 0).is_err());

        let stage = Box::new(Passthrough::new(48_000, 0));
        assert!(StageRunner::new(stage, 48_000, 128).is_err());

        let stage = Box::new(Passthrough::new(48_000, 480));
        let mut runner = StageRunner::new(stage, 48_000, 128).unwrap();
        assert!(runner.process(&[0.0; 8], &mut [0.0; 9]).is_err());
        assert!(runner.process(&[0.0; 256], &mut [0.0; 256]).is_err());
    }

    #[test]
    fn exposes_its_configuration() {
        let stage = Box::new(Passthrough::new(16_000, 160));
        let runner = StageRunner::new(stage, 48_000, 128).unwrap();
        assert_eq!(runner.host_rate(), 48_000);
        assert_eq!(runner.max_host_block(), 128);
        assert_eq!(runner.stage_spec().sample_rate, 16_000);
        assert!(runner.latency_ms() > 0.0);
        let stage = runner.into_stage();
        assert_eq!(stage.spec().block_size, 160);
    }
}
