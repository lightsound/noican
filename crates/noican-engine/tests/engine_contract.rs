use noican_engine::{
    process_clip, validate_frame_lengths, AudioStage, DelayCompensation, EnrollmentRequirement,
    RateAdapter, StageDescriptor, StageError, StageKind, SwitchingEngine, PIPELINE_FRAME_SAMPLES,
    PIPELINE_SAMPLE_RATE,
};

struct PassthroughStage {
    descriptor: StageDescriptor,
}

impl AudioStage for PassthroughStage {
    fn descriptor(&self) -> StageDescriptor {
        self.descriptor
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        validate_frame_lengths(self.descriptor, input, output)?;
        output.copy_from_slice(input);
        Ok(())
    }

    fn reset(&mut self) -> Result<(), StageError> {
        Ok(())
    }
}

struct ConstantStage {
    descriptor: StageDescriptor,
    value: f32,
}

impl AudioStage for ConstantStage {
    fn descriptor(&self) -> StageDescriptor {
        self.descriptor
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        validate_frame_lengths(self.descriptor, input, output)?;
        output.fill(self.value);
        Ok(())
    }

    fn reset(&mut self) -> Result<(), StageError> {
        Ok(())
    }
}

const fn descriptor(id: &'static str, sample_rate: u32, frame_samples: usize) -> StageDescriptor {
    StageDescriptor {
        id,
        display_name: id,
        kind: StageKind::NoiseSuppression,
        sample_rate,
        frame_samples,
        algorithmic_delay_samples: 0,
        tail_frames: 0,
        enrollment: EnrollmentRequirement::None,
    }
}

#[test]
fn offline_path_preserves_aligned_passthrough() -> Result<(), StageError> {
    let native = PassthroughStage {
        descriptor: descriptor("passthrough", PIPELINE_SAMPLE_RATE, PIPELINE_FRAME_SAMPLES),
    };
    let mut adapted = RateAdapter::new(Box::new(native))?;
    let input: Vec<f32> = (0_u16..1_000)
        .map(|sample| f32::from(sample) / 1_000.0)
        .collect();
    let output = process_clip(&mut adapted, &input, DelayCompensation::Remove)?;
    assert_eq!(output, input);
    Ok(())
}

#[test]
fn frame_adapter_buffers_non_divisible_model_frames() -> Result<(), StageError> {
    let native = PassthroughStage {
        descriptor: descriptor("wide-frame", PIPELINE_SAMPLE_RATE, 512),
    };
    let mut adapted = RateAdapter::new(Box::new(native))?;
    let input = [0.5_f32; PIPELINE_FRAME_SAMPLES];
    let mut first = [0.0_f32; PIPELINE_FRAME_SAMPLES];
    let mut second = [0.0_f32; PIPELINE_FRAME_SAMPLES];
    let mut third = [0.0_f32; PIPELINE_FRAME_SAMPLES];
    adapted.process_frame(&input, &mut first)?;
    adapted.process_frame(&input, &mut second)?;
    adapted.process_frame(&input, &mut third)?;
    assert!(first.iter().all(|sample| *sample == 0.0));
    assert!(second.iter().all(|sample| *sample == 0.0));
    assert!(third.iter().any(|sample| *sample != 0.0));
    Ok(())
}

#[test]
fn rate_adapter_absorbs_sixteen_kilohertz_stage() -> Result<(), StageError> {
    let native = PassthroughStage {
        descriptor: descriptor("sixteen-k", 16_000, 160),
    };
    let mut adapted = RateAdapter::new(Box::new(native))?;
    assert_eq!(adapted.descriptor().sample_rate, PIPELINE_SAMPLE_RATE);
    assert_eq!(adapted.descriptor().frame_samples, PIPELINE_FRAME_SAMPLES);
    let input = [0.25_f32; PIPELINE_FRAME_SAMPLES];
    let mut output = [0.0_f32; PIPELINE_FRAME_SAMPLES];
    for _ in 0..4 {
        adapted.process_frame(&input, &mut output)?;
    }
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| sample.abs() > 0.01));
    Ok(())
}

#[test]
fn published_stage_switches_through_short_mute() -> Result<(), StageError> {
    let initial = ConstantStage {
        descriptor: descriptor("one", PIPELINE_SAMPLE_RATE, PIPELINE_FRAME_SAMPLES),
        value: 1.0,
    };
    let replacement = ConstantStage {
        descriptor: descriptor("minus-one", PIPELINE_SAMPLE_RATE, PIPELINE_FRAME_SAMPLES),
        value: -1.0,
    };
    let (publisher, mut engine) = SwitchingEngine::new(Box::new(initial), 240)?;
    let input = [0.0_f32; PIPELINE_FRAME_SAMPLES];
    let mut output = [0.0_f32; PIPELINE_FRAME_SAMPLES];

    engine.process_frame(&input, &mut output)?;
    assert!(output.iter().all(|sample| *sample == 1.0));
    assert!(publisher.publish(Box::new(replacement))?.is_none());

    engine.process_frame(&input, &mut output)?;
    assert_eq!(output[0], 1.0);
    assert_eq!(output[240], 0.0);
    assert_eq!(engine.active_descriptor().id, "minus-one");

    engine.process_frame(&input, &mut output)?;
    assert_eq!(output[0], 0.0);
    assert_eq!(output[240], -1.0);

    engine.process_frame(&input, &mut output)?;
    assert!(output.iter().all(|sample| *sample == -1.0));
    assert_eq!(engine.active_generation(), 1);
    Ok(())
}
