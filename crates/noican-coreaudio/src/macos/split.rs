//! See docs/adr/0003-coreaudio-transport.md.

use std::ffi::c_void;
use std::sync::{
    Arc,
    atomic::{AtomicI32, AtomicU64, Ordering},
};

use noican_core::{DriftServo, InputResampler, SwitchingEngine};
use rtrb::RingBuffer;

use crate::callback::OutputFeed;
use crate::observe::StreamLevels;
use crate::routing::split_render_channels;
use crate::{CoreAudioError, WORKER_BLOCK_SAMPLES};

use super::{
    AUDIO_OUTPUT_UNIT_PROPERTY_SET_INPUT_CALLBACK, AUDIO_UNIT_SCOPE_GLOBAL, AUDIO_UNIT_SCOPE_INPUT,
    AudioBufferList, AudioDeviceId, AudioUnit, AudioUnitRenderActionFlags, AudioUnitRenderCallback,
    AudioUnitSetProperty, AuhalUnit, Capture, ContextGuard, Direction, NO_ERR, OSStatus,
    OUTPUT_BUS, PARAM_ERR, RING_CAPACITY, Runtime, Shared, Transport, WORKER_WAIT_NS, WorkerLinks,
    attach_render_callback, audio_workgroup, check_status, device_output_channels, monitor,
    pcm_format, pcm_format_at, render_samples, set_client_format, set_current_device,
    set_io_enabled, set_max_frames_per_slice, size_u32, spawn_worker, start_output_unit,
    stop_output_unit, stop_worker, stream_format_channels,
};

const NATIVE_CHUNK_SAMPLES: usize = 480;

const OUTPUT_PRIME_SAMPLES: usize = 2_400;

const OUTPUT_STALL_BLOCKS: usize = 300;

pub(super) struct OutputContext {
    feed: OutputFeed,
    pulses: Arc<AtomicU64>,
}

struct SplitWorkerLinks {
    worker: WorkerLinks,
    resampler: InputResampler,
    servo: DriftServo,
    output_pulses: Arc<AtomicU64>,
}

pub(super) fn start(
    input_device: AudioDeviceId,
    output_device: AudioDeviceId,
    capture_rate: u32,
    engine: SwitchingEngine,
    levels: Arc<StreamLevels>,
    monitor_state: Arc<AtomicI32>,
) -> Result<Runtime, CoreAudioError> {
    let resampler = InputResampler::new(capture_rate, NATIVE_CHUNK_SAMPLES)
        .map_err(|error| CoreAudioError::Worker(error.to_string()))?;
    let shared = Shared::new()?;

    let (input_producer, input_consumer) = RingBuffer::new(RING_CAPACITY);
    let (mut output_producer, output_consumer) = RingBuffer::new(RING_CAPACITY);
    for _ in 0..OUTPUT_PRIME_SAMPLES {
        let _ignored = output_producer.push(0.0);
    }

    let mut capture_unit = AuhalUnit::create()?;
    configure_capture_auhal(capture_unit.raw(), input_device, capture_rate)?;
    let capture_context =
        ContextGuard::new(Capture::new(capture_unit.raw(), input_producer, &shared));
    attach_input_callback(capture_unit.raw(), capture_context.raw().cast())?;
    capture_unit.initialize()?;
    let workgroup = audio_workgroup(capture_unit.raw())?;

    let output_pulses = Arc::new(AtomicU64::new(0));
    let (output_unit, output_context, routing) = build_output_half(
        output_device,
        OutputFeed::new(output_consumer, Arc::clone(&shared.underruns)),
        &output_pulses,
    )?;

    let (monitor, tee) = monitor::monitor_pair(monitor_state);
    let links = SplitWorkerLinks {
        worker: WorkerLinks {
            engine,
            input: input_consumer,
            output: output_producer,
            tee,
            levels,
            shared: shared.clone(),
        },
        resampler,
        servo: DriftServo::new(OUTPUT_PRIME_SAMPLES),
        output_pulses,
    };
    let worker = spawn_worker(move || split_processing_loop(links, workgroup))?;

    if let Err(error) = start_output_unit(capture_unit.raw(), "AudioOutputUnitStart(capture)") {
        stop_worker(&shared, worker);
        return Err(error);
    }
    if let Err(error) = start_output_unit(output_unit.raw(), "AudioOutputUnitStart(virtual output)")
    {
        stop_output_unit(capture_unit.raw());
        stop_worker(&shared, worker);
        return Err(error);
    }
    Ok(Runtime {
        transport: Transport::Split {
            capture_unit: capture_unit.into_raw() as usize,
            capture_context: capture_context.into_raw() as usize,
            output_unit: output_unit.into_raw() as usize,
            output_context: output_context.into_raw() as usize,
        },
        shared,
        worker: Some(worker),
        running: true,
        monitor,
        routing,
    })
}

fn build_output_half(
    output_device: AudioDeviceId,
    feed: OutputFeed,
    output_pulses: &Arc<AtomicU64>,
) -> Result<(AuhalUnit, ContextGuard<OutputContext>, String), CoreAudioError> {
    let mut output_unit = AuhalUnit::create()?;
    let channels = configure_output_auhal(output_unit.raw(), output_device)?;
    let output_context = ContextGuard::new(OutputContext {
        feed,
        pulses: Arc::clone(output_pulses),
    });
    attach_render_callback(
        output_unit.raw(),
        output_render_callback,
        output_context.raw().cast(),
        "AudioUnitSetProperty(split output render callback)",
    )?;
    output_unit.initialize()?;
    let routing = format!(
        "virtual output channels {channels}, render format requested {channels} ch, {}",
        describe_applied_render_format(output_unit.raw())
    );
    Ok((output_unit, output_context, routing))
}

fn describe_applied_render_format(unit: AudioUnit) -> String {
    match stream_format_channels(
        unit,
        AUDIO_UNIT_SCOPE_INPUT,
        "read split output render format",
    ) {
        Ok(channels) => format!("render format read back after initialize {channels} ch"),
        Err(error) => format!("render format read back after initialize: unreadable ({error})"),
    }
}

fn configure_capture_auhal(
    unit: AudioUnit,
    device: AudioDeviceId,
    capture_rate: u32,
) -> Result<(), CoreAudioError> {
    set_io_enabled(unit, Direction::Capture, true, "enable capture AUHAL input")?;
    set_io_enabled(
        unit,
        Direction::Render,
        false,
        "disable capture AUHAL output",
    )?;
    set_current_device(unit, device, "select capture device")?;
    set_client_format(
        unit,
        Direction::Capture,
        &pcm_format_at(f64::from(capture_rate), 1),
        "set capture client format",
    )?;
    set_max_frames_per_slice(unit, "set capture frame bound")
}

fn configure_output_auhal(unit: AudioUnit, device: AudioDeviceId) -> Result<u32, CoreAudioError> {
    set_io_enabled(
        unit,
        Direction::Capture,
        false,
        "disable split output AUHAL input",
    )?;
    set_io_enabled(
        unit,
        Direction::Render,
        true,
        "enable split output AUHAL output",
    )?;
    set_current_device(unit, device, "select virtual output device")?;
    let device_channels = device_output_channels(unit, "read virtual output device format")?;
    let channels = split_render_channels(device_channels)?;
    set_client_format(
        unit,
        Direction::Render,
        &pcm_format(channels),
        "set split output render format",
    )?;
    Ok(channels)
}

fn attach_input_callback(unit: AudioUnit, context: *mut c_void) -> Result<(), CoreAudioError> {
    let property = AudioUnitRenderCallback {
        callback: Some(capture_input_callback),
        context,
    };
    check_status(
        unsafe {
            AudioUnitSetProperty(
                unit,
                AUDIO_OUTPUT_UNIT_PROPERTY_SET_INPUT_CALLBACK,
                AUDIO_UNIT_SCOPE_GLOBAL,
                OUTPUT_BUS,
                (&raw const property).cast(),
                size_u32::<AudioUnitRenderCallback>()?,
            )
        },
        "AudioUnitSetProperty(capture input callback)",
    )
}

fn split_processing_loop(links: SplitWorkerLinks, workgroup: usize) {
    let SplitWorkerLinks {
        mut worker,
        mut resampler,
        mut servo,
        output_pulses,
    } = links;
    let membership = worker.enter(workgroup);
    let output_capacity = worker.output.buffer().capacity();
    let mut native_chunk = [0.0_f32; NATIVE_CHUNK_SAMPLES];
    let max_converted = resampler.max_output_len(NATIVE_CHUNK_SAMPLES);
    let mut converted: Vec<f32> = Vec::with_capacity(max_converted);
    let mut pending: std::collections::VecDeque<f32> =
        std::collections::VecDeque::with_capacity(max_converted + WORKER_BLOCK_SAMPLES);
    let mut input_block = [0.0_f32; WORKER_BLOCK_SAMPLES];
    let mut output_block = [0.0_f32; WORKER_BLOCK_SAMPLES];
    let mut last_output_pulses = 0_u64;
    let mut output_idle_blocks = 0_usize;
    while !worker.shared.shutdown.load(Ordering::Acquire) {
        let mut popped = 0;
        while popped < NATIVE_CHUNK_SAMPLES {
            match worker.input.pop() {
                Ok(sample) => {
                    native_chunk[popped] = sample;
                    popped += 1;
                }
                Err(_empty) => break,
            }
        }
        if popped == 0 {
            worker.shared.samples_ready.wait_ns(WORKER_WAIT_NS);
            continue;
        }
        converted.clear();
        resampler.process(&native_chunk[..popped], &mut converted);
        pending.extend(converted.iter().copied());
        while pending.len() >= WORKER_BLOCK_SAMPLES {
            for slot in &mut input_block {
                *slot = pending.pop_front().unwrap_or(0.0);
            }
            worker.run_block(&input_block, &mut output_block);
            let buffered = (output_capacity - worker.output.slots())
                + pending.len()
                + resampler.to_engine_samples(worker.input.slots());
            resampler.set_drift_ppm(servo.update(buffered));
            let pulses = output_pulses.load(Ordering::Relaxed);
            if pulses == last_output_pulses {
                output_idle_blocks = output_idle_blocks.saturating_add(1);
                if output_idle_blocks >= OUTPUT_STALL_BLOCKS {
                    worker.shared.faulted.store(true, Ordering::Release);
                }
            } else {
                last_output_pulses = pulses;
                output_idle_blocks = 0;
            }
        }
    }
    worker.exit(membership);
}

unsafe extern "C" fn capture_input_callback(
    context: *mut c_void,
    action_flags: *mut AudioUnitRenderActionFlags,
    timestamp: *const c_void,
    _bus: u32,
    frame_count: u32,
    _data: *mut AudioBufferList,
) -> OSStatus {
    if context.is_null() {
        return PARAM_ERR;
    }
    if frame_count == 0 {
        return NO_ERR;
    }
    let capture = unsafe { &mut *context.cast::<Capture>() };
    match unsafe { capture.capture(action_flags, timestamp, frame_count) } {
        Ok(()) => NO_ERR,
        Err(status) => status,
    }
}

unsafe extern "C" fn output_render_callback(
    context: *mut c_void,
    _action_flags: *mut AudioUnitRenderActionFlags,
    _timestamp: *const c_void,
    _bus: u32,
    frame_count: u32,
    data: *mut AudioBufferList,
) -> OSStatus {
    if context.is_null() || data.is_null() {
        return PARAM_ERR;
    }
    let context = unsafe { &mut *context.cast::<OutputContext>() };
    let Some((samples, channels)) = (unsafe { render_samples(data, frame_count) }) else {
        return PARAM_ERR;
    };
    context.feed.render(samples, channels);
    context.pulses.fetch_add(1, Ordering::Relaxed);
    NO_ERR
}
