//! See docs/adr/0003-coreaudio-transport.md#split-transport-architecture.

use std::ffi::c_void;
use std::sync::{
    Arc,
    atomic::{AtomicI32, AtomicU64, Ordering},
};

use noican_core::{DriftServo, InputResampler, SwitchingEngine};
use rtrb::RingBuffer;

use crate::callback::OutputFeed;
use crate::observe::StreamLevels;
use crate::pump::{ClockBridge, NATIVE_CHUNK_SAMPLES, Pump};
use crate::routing::split_render_channels;
use crate::{CoreAudioError, cushion};

use super::{
    AUDIO_OUTPUT_UNIT_PROPERTY_SET_INPUT_CALLBACK, AUDIO_UNIT_SCOPE_GLOBAL, AUDIO_UNIT_SCOPE_INPUT,
    AudioBufferList, AudioDeviceId, AudioUnit, AudioUnitRenderActionFlags, AudioUnitRenderCallback,
    AudioUnitSetProperty, AuhalUnit, Capture, ContextGuard, Direction, NO_ERR, OSStatus,
    OUTPUT_BUS, PARAM_ERR, RING_CAPACITY, Runtime, Shared, Transport, attach_render_callback,
    audio_workgroup, buffer_frames, check_status, device_output_channels, monitor, pcm_format,
    pcm_format_at, render_samples, set_client_format, set_current_device, set_io_enabled,
    set_max_frames_per_slice, size_u32, spawn_pump, start_output_unit, stop_output_unit,
    stop_worker, stream_format_channels,
};

pub(super) struct OutputContext {
    feed: OutputFeed,
    pulses: Arc<AtomicU64>,
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
    let requested = std::env::var_os(cushion::VARIABLE);
    let requested = requested.as_deref().map(std::ffi::OsStr::to_string_lossy);
    let cushion = cushion::requested(requested.as_deref()).map_err(CoreAudioError::Worker)?;
    let shared = Shared::new()?;

    let (input_producer, input_consumer) = RingBuffer::new(RING_CAPACITY);
    let (mut output_producer, output_consumer) = RingBuffer::new(RING_CAPACITY);
    for _ in 0..cushion {
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
        OutputFeed::primed_ring(output_consumer, Arc::clone(&shared.underruns)),
        &output_pulses,
    )?;
    let routing = format!(
        "{routing}, {}",
        cushion::describe(
            cushion,
            capture_rate,
            buffer_frames(capture_unit.raw()),
            buffer_frames(output_unit.raw()),
        )
    );

    let (monitor, tee) = monitor::monitor_pair(monitor_state);
    let pump = Pump::new(
        shared.pump_links(engine, input_consumer, output_producer, tee, levels),
        Some(ClockBridge::new(
            resampler,
            DriftServo::new(cushion),
            output_pulses,
        )),
    );
    let worker = spawn_pump(pump, &shared, workgroup)?;

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
