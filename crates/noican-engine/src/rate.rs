//! Streaming sample-rate and frame-size adaptation.

use std::collections::VecDeque;

use audioadapter_buffers::owned::InterleavedOwned;
use rubato::{Fft, FixedSync, Resampler};

use crate::{
    validate_frame_lengths, AudioStage, StageDescriptor, StageError, PIPELINE_FRAME_SAMPLES,
    PIPELINE_SAMPLE_RATE,
};

/// Resample a complete mono clip with delay-compensated FFT conversion.
///
/// This helper is intended for file input and enrollment preparation. Live
/// model adaptation uses [`RateAdapter`] so converter state persists across
/// frames.
///
/// # Errors
///
/// Returns [`StageError::InvalidConfiguration`] for a zero sample rate and
/// [`StageError::Resampling`] if Rubato rejects the conversion.
pub fn resample_clip(
    input: &[f32],
    input_sample_rate: u32,
    output_sample_rate: u32,
) -> Result<Vec<f32>, StageError> {
    if input_sample_rate == 0 || output_sample_rate == 0 {
        return Err(StageError::InvalidConfiguration {
            stage: "file-resampler",
            message: "sample rates must be non-zero".to_owned(),
        });
    }
    if input_sample_rate == output_sample_rate {
        return Ok(input.to_vec());
    }
    let input_rate =
        usize::try_from(input_sample_rate).map_err(|error| StageError::InvalidConfiguration {
            stage: "file-resampler",
            message: error.to_string(),
        })?;
    let output_rate =
        usize::try_from(output_sample_rate).map_err(|error| StageError::InvalidConfiguration {
            stage: "file-resampler",
            message: error.to_string(),
        })?;
    let adapter = InterleavedOwned::new_from(input.to_vec(), 1, input.len())
        .map_err(|error| StageError::Resampling(error.to_string()))?;
    let mut resampler = Fft::<f32>::new(input_rate, output_rate, 1_024, 1, FixedSync::Input)
        .map_err(|error| StageError::Resampling(error.to_string()))?;
    let mut output = resampler
        .process_all(&adapter, input.len(), None)
        .map_err(|error| StageError::Resampling(error.to_string()))?
        .take_data();
    let expected = input
        .len()
        .checked_mul(output_rate)
        .ok_or_else(|| StageError::InvalidConfiguration {
            stage: "file-resampler",
            message: "output length overflow".to_owned(),
        })?
        .div_ceil(input_rate);
    output.resize(expected, 0.0);
    output.truncate(expected);
    Ok(output)
}

/// Adapts a native model stage to noican's fixed 48 kHz, 10 ms contract.
///
/// The adapter retains both resampler state and partial model frames. All
/// allocations happen on the inference thread, never in a Core Audio callback.
pub struct RateAdapter {
    stage: Box<dyn AudioStage>,
    descriptor: StageDescriptor,
    native_descriptor: StageDescriptor,
    downsampler: Option<Fft<f32>>,
    upsampler: Option<Fft<f32>>,
    native_samples_per_quantum: usize,
    upsampler_input_samples: usize,
    startup_watermark: usize,
    started: bool,
    native_input: VecDeque<f32>,
    native_output: VecDeque<f32>,
    pipeline_output: VecDeque<f32>,
    model_input: Vec<f32>,
    model_output: Vec<f32>,
}

struct ResamplerPair {
    down: Option<Fft<f32>>,
    up: Option<Fft<f32>>,
    native_samples_per_quantum: usize,
    conversion_delay: usize,
}

impl RateAdapter {
    /// Wrap a native stage with the shared pipeline contract.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::InvalidConfiguration`] if the model descriptor
    /// is empty or cannot be represented by the fixed-rate resamplers.
    pub fn new(stage: Box<dyn AudioStage>) -> Result<Self, StageError> {
        let native = stage.descriptor();
        validate_descriptor(native)?;

        let resamplers = build_resamplers(native)?;
        let downsampler = resamplers.down;
        let upsampler = resamplers.up;
        let native_samples_per_quantum = resamplers.native_samples_per_quantum;
        let conversion_delay = resamplers.conversion_delay;
        let upsampler_input_samples = upsampler.as_ref().map_or(1, Resampler::input_frames_next);
        let converted_model_frame = scale_samples(
            native.frame_samples,
            native.sample_rate,
            PIPELINE_SAMPLE_RATE,
        )?;
        let aligned = native_samples_per_quantum % native.frame_samples == 0;
        let startup_watermark = if aligned {
            PIPELINE_FRAME_SAMPLES
        } else {
            PIPELINE_FRAME_SAMPLES
                .checked_add(converted_model_frame)
                .ok_or_else(|| invalid_config(native, "startup watermark overflow"))?
        };
        let startup_quanta = startup_quanta(
            native_samples_per_quantum,
            native.frame_samples,
            upsampler_input_samples,
            upsampler.as_ref().map_or(1, Resampler::output_frames_next),
            upsampler.is_none(),
            startup_watermark,
            native,
        )?;
        let model_delay = scale_samples(
            native.algorithmic_delay_samples,
            native.sample_rate,
            PIPELINE_SAMPLE_RATE,
        )?;
        let buffering_delay = startup_quanta
            .checked_mul(PIPELINE_FRAME_SAMPLES)
            .ok_or_else(|| invalid_config(native, "buffering delay overflow"))?;
        let algorithmic_delay_samples = model_delay
            .checked_add(conversion_delay)
            .and_then(|delay| delay.checked_add(buffering_delay))
            .ok_or_else(|| invalid_config(native, "total delay overflow"))?;
        let tail_samples = native
            .tail_frames
            .checked_mul(native.frame_samples)
            .ok_or_else(|| invalid_config(native, "tail length overflow"))?;
        let converted_tail = scale_samples(tail_samples, native.sample_rate, PIPELINE_SAMPLE_RATE)?;
        let tail_frames = converted_tail.div_ceil(PIPELINE_FRAME_SAMPLES);
        let descriptor = StageDescriptor {
            sample_rate: PIPELINE_SAMPLE_RATE,
            frame_samples: PIPELINE_FRAME_SAMPLES,
            algorithmic_delay_samples,
            tail_frames,
            ..native
        };

        Ok(Self {
            stage,
            descriptor,
            native_descriptor: native,
            downsampler,
            upsampler,
            native_samples_per_quantum,
            upsampler_input_samples,
            startup_watermark,
            started: false,
            native_input: VecDeque::with_capacity(native.frame_samples * 2),
            native_output: VecDeque::with_capacity(native.frame_samples * 2),
            pipeline_output: VecDeque::with_capacity(startup_watermark * 2),
            model_input: vec![0.0; native.frame_samples],
            model_output: vec![0.0; native.frame_samples],
        })
    }

    fn convert_input(&mut self, input: &[f32]) -> Result<Vec<f32>, StageError> {
        let Some(resampler) = &mut self.downsampler else {
            return Ok(input.to_vec());
        };
        let adapter = InterleavedOwned::new_from(input.to_vec(), 1, input.len())
            .map_err(|error| StageError::Resampling(error.to_string()))?;
        let output = resampler
            .process(&adapter, None)
            .map_err(|error| StageError::Resampling(error.to_string()))?
            .take_data();
        if output.len() != self.native_samples_per_quantum {
            return Err(invalid_config(
                self.native_descriptor,
                "downsampler changed its fixed output length",
            ));
        }
        Ok(output)
    }

    fn run_ready_model_frames(&mut self) -> Result<(), StageError> {
        while self.native_input.len() >= self.native_descriptor.frame_samples {
            for slot in &mut self.model_input {
                *slot = self.native_input.pop_front().ok_or_else(|| {
                    invalid_config(self.native_descriptor, "native input queue underflow")
                })?;
            }
            self.stage
                .process_frame(&self.model_input, &mut self.model_output)?;
            self.native_output.extend(self.model_output.iter().copied());
        }
        Ok(())
    }

    fn convert_ready_output(&mut self) -> Result<(), StageError> {
        let Some(resampler) = &mut self.upsampler else {
            self.pipeline_output.extend(self.native_output.drain(..));
            return Ok(());
        };
        while self.native_output.len() >= self.upsampler_input_samples {
            let input: Vec<f32> = self
                .native_output
                .drain(..self.upsampler_input_samples)
                .collect();
            let adapter = InterleavedOwned::new_from(input, 1, self.upsampler_input_samples)
                .map_err(|error| StageError::Resampling(error.to_string()))?;
            let output = resampler
                .process(&adapter, None)
                .map_err(|error| StageError::Resampling(error.to_string()))?
                .take_data();
            self.pipeline_output.extend(output);
        }
        Ok(())
    }
}

impl AudioStage for RateAdapter {
    fn descriptor(&self) -> StageDescriptor {
        self.descriptor
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        validate_frame_lengths(self.descriptor, input, output)?;
        let native = self.convert_input(input)?;
        self.native_input.extend(native);
        self.run_ready_model_frames()?;
        self.convert_ready_output()?;

        if !self.started && self.pipeline_output.len() >= self.startup_watermark {
            self.started = true;
        }
        if !self.started {
            output.fill(0.0);
            return Ok(());
        }
        if self.pipeline_output.len() < output.len() {
            return Err(invalid_config(
                self.native_descriptor,
                "pipeline output queue underflow after startup",
            ));
        }
        for slot in output {
            *slot = self.pipeline_output.pop_front().ok_or_else(|| {
                invalid_config(
                    self.native_descriptor,
                    "pipeline output queue unexpectedly empty",
                )
            })?;
        }
        Ok(())
    }

    fn reset(&mut self) -> Result<(), StageError> {
        self.stage.reset()?;
        if let Some(resampler) = &mut self.downsampler {
            resampler.reset();
        }
        if let Some(resampler) = &mut self.upsampler {
            resampler.reset();
        }
        self.native_input.clear();
        self.native_output.clear();
        self.pipeline_output.clear();
        self.model_input.fill(0.0);
        self.model_output.fill(0.0);
        self.started = false;
        Ok(())
    }
}

fn validate_descriptor(descriptor: StageDescriptor) -> Result<(), StageError> {
    if descriptor.sample_rate == 0 {
        return Err(invalid_config(descriptor, "sample rate must be non-zero"));
    }
    if descriptor.frame_samples == 0 {
        return Err(invalid_config(descriptor, "frame size must be non-zero"));
    }
    Ok(())
}

fn build_resamplers(descriptor: StageDescriptor) -> Result<ResamplerPair, StageError> {
    if descriptor.sample_rate == PIPELINE_SAMPLE_RATE {
        return Ok(ResamplerPair {
            down: None,
            up: None,
            native_samples_per_quantum: PIPELINE_FRAME_SAMPLES,
            conversion_delay: 0,
        });
    }
    let down = Fft::<f32>::new(
        usize::try_from(PIPELINE_SAMPLE_RATE)
            .map_err(|error| invalid_config(descriptor, &error.to_string()))?,
        usize::try_from(descriptor.sample_rate)
            .map_err(|error| invalid_config(descriptor, &error.to_string()))?,
        PIPELINE_FRAME_SAMPLES,
        1,
        FixedSync::Both,
    )
    .map_err(|error| StageError::Resampling(error.to_string()))?;
    if down.input_frames_next() != PIPELINE_FRAME_SAMPLES {
        return Err(invalid_config(
            descriptor,
            "downsampler cannot preserve the shared input quantum",
        ));
    }
    let native_samples = down.output_frames_next();
    let down_delay = scale_samples(
        down.output_delay(),
        descriptor.sample_rate,
        PIPELINE_SAMPLE_RATE,
    )?;
    let up = Fft::<f32>::new(
        usize::try_from(descriptor.sample_rate)
            .map_err(|error| invalid_config(descriptor, &error.to_string()))?,
        usize::try_from(PIPELINE_SAMPLE_RATE)
            .map_err(|error| invalid_config(descriptor, &error.to_string()))?,
        native_samples,
        1,
        FixedSync::Both,
    )
    .map_err(|error| StageError::Resampling(error.to_string()))?;
    if up.input_frames_next() != native_samples || up.output_frames_next() != PIPELINE_FRAME_SAMPLES
    {
        return Err(invalid_config(
            descriptor,
            "upsampler cannot preserve the shared output quantum",
        ));
    }
    let conversion_delay = down_delay
        .checked_add(up.output_delay())
        .ok_or_else(|| invalid_config(descriptor, "resampler delay overflow"))?;
    Ok(ResamplerPair {
        down: Some(down),
        up: Some(up),
        native_samples_per_quantum: native_samples,
        conversion_delay,
    })
}

fn startup_quanta(
    native_per_quantum: usize,
    model_frame: usize,
    upsampler_input: usize,
    upsampler_output: usize,
    identity_output: bool,
    watermark: usize,
    descriptor: StageDescriptor,
) -> Result<usize, StageError> {
    const SEARCH_LIMIT: usize = 16_384;
    for quanta in 1..=SEARCH_LIMIT {
        let native_received = quanta
            .checked_mul(native_per_quantum)
            .ok_or_else(|| invalid_config(descriptor, "startup input overflow"))?;
        let model_calls = native_received / model_frame;
        let native_produced = model_calls
            .checked_mul(model_frame)
            .ok_or_else(|| invalid_config(descriptor, "startup output overflow"))?;
        let pipeline_produced = if identity_output {
            native_produced
        } else {
            (native_produced / upsampler_input)
                .checked_mul(upsampler_output)
                .ok_or_else(|| invalid_config(descriptor, "converted startup overflow"))?
        };
        if pipeline_produced >= watermark {
            return Ok(quanta - 1);
        }
    }
    Err(invalid_config(
        descriptor,
        "stage frame sizes never reach the startup watermark",
    ))
}

fn scale_samples(samples: usize, from_rate: u32, to_rate: u32) -> Result<usize, StageError> {
    let numerator = samples
        .checked_mul(
            usize::try_from(to_rate)
                .map_err(|error| invalid_config_placeholder(&error.to_string()))?,
        )
        .ok_or_else(|| invalid_config_placeholder("sample-rate conversion overflow"))?;
    let denominator = usize::try_from(from_rate)
        .map_err(|error| invalid_config_placeholder(&error.to_string()))?;
    Ok(numerator.div_ceil(denominator))
}

fn invalid_config(descriptor: StageDescriptor, message: &str) -> StageError {
    StageError::InvalidConfiguration {
        stage: descriptor.id,
        message: message.to_owned(),
    }
}

fn invalid_config_placeholder(message: &str) -> StageError {
    StageError::InvalidConfiguration {
        stage: "rate-adapter",
        message: message.to_owned(),
    }
}
