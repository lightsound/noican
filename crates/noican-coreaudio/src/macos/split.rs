//! Split transport for non-48 kHz microphones (issue #7): two AUHAL
//! instances bridged by a drift-compensating worker.
//!
//! An Aggregate Device drives all its subdevices at one nominal rate, and
//! the Noican virtual output only supports 44.1/48 kHz, so a telephony-
//! profile microphone (Bluetooth HFP at 8/16/24 kHz) cannot share an
//! aggregate with the 48 kHz virtual output. This module instead opens:
//!
//! - an **input-only AUHAL** on the microphone device with a client
//!   format at its native rate (AUHAL performs no sample-rate conversion
//!   on the input side, so the client rate must equal the device rate);
//! - an **output-only AUHAL** on the virtual output device at the 48 kHz
//!   engine rate (mono engine samples duplicated into every device
//!   channel, like the preview monitor).
//!
//! The two units run on separate device clocks. The inference worker
//! bridges them: it drains the native-rate capture ring, converts to
//! 48 kHz through [`InputResampler`] (integer-factor polyphase plus a
//! micro-ratio drift stage), feeds the unchanged engine in 10 ms blocks,
//! and pushes the result into the output ring. A [`DriftServo`] observes
//! the total samples buffered between the two clock domains once per
//! block and steers the resampler a few hundred ppm to cancel clock
//! drift (docs/tech-research.md §4.2) — the output ring is primed with
//! [`OUTPUT_PRIME_SAMPLES`] of silence, which is both the initial jitter
//! cushion and the servo's occupancy target.
//!
//! Real-time rules (docs/tech-research.md §9) hold as on the aggregate
//! path: the capture callback only calls `AudioUnitRender` into a
//! preallocated buffer, pushes into a lock-free ring, and signals the
//! worker's semaphore; the output callback only pops the output ring
//! (silence on underrun); all resampling runs on the worker with state
//! preallocated at start.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
};
use std::thread;

use noican_core::{DriftServo, InputResampler, SwitchingEngine};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::monitor::MonitorTee;
use crate::observe::StreamLevels;
use crate::{CoreAudioError, WORKER_BLOCK_SAMPLES};

use super::{
    AUDIO_OUTPUT_UNIT_PROPERTY_CURRENT_DEVICE, AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
    AUDIO_OUTPUT_UNIT_PROPERTY_SET_INPUT_CALLBACK, AUDIO_UNIT_PROPERTY_MAXIMUM_FRAMES_PER_SLICE,
    AUDIO_UNIT_PROPERTY_STREAM_FORMAT, AUDIO_UNIT_SCOPE_GLOBAL, AUDIO_UNIT_SCOPE_INPUT,
    AUDIO_UNIT_SCOPE_OUTPUT, AudioBuffer, AudioBufferList, AudioDeviceId, AudioUnit,
    AudioUnitRender, AudioUnitRenderActionFlags, AudioUnitRenderCallback, AudioUnitSetProperty,
    AuhalUnit, ContextGuard, DispatchSemaphore, INPUT_BUS, NO_ERR, OSStatus, OUTPUT_BUS, PARAM_ERR,
    RING_CAPACITY, Runtime, Transport, WORKER_WAIT_NS, WorkgroupGuard, attach_render_callback,
    audio_workgroup, check_status, monitor, pcm_format, pcm_format_at, run_block, set_property,
    size_u32, start_output_unit, stop_output_unit,
};

/// Largest callback the capture unit may deliver, in frames. Set as
/// `kAudioUnitProperty_MaximumFramesPerSlice` and used to size the
/// preallocated render buffer, so the capture callback never allocates.
const MAX_CAPTURE_FRAMES: usize = 4_096;

/// Native-rate samples drained from the capture ring per worker pass
/// (also the resampler's preallocation unit): 480 native samples cover
/// 20–60 ms of telephony-profile audio, comfortably above Bluetooth
/// burst sizes.
const NATIVE_CHUNK_SAMPLES: usize = 480;

/// Silence pushed into the output ring before the units start: the
/// initial cushion absorbing Bluetooth burst jitter, and the drift
/// servo's occupancy target (50 ms at 48 kHz). This is added latency of
/// the split path only; the aggregate path is untouched.
const OUTPUT_PRIME_SAMPLES: usize = 2_400;

/// Render context of the input-only capture AUHAL.
pub(super) struct CaptureContext {
    unit: AudioUnit,
    input: Producer<f32>,
    /// Preallocated landing buffer for `AudioUnitRender` (mono,
    /// [`MAX_CAPTURE_FRAMES`]).
    buffer: Vec<f32>,
    faulted: Arc<AtomicBool>,
    samples_ready: Arc<DispatchSemaphore>,
    /// Heartbeat: capture frames delivered since start (see
    /// [`Runtime::frames_processed`]).
    frames: Arc<AtomicU64>,
}

/// Render context of the output-only AUHAL on the virtual output.
pub(super) struct OutputContext {
    output: Consumer<f32>,
}

/// Everything the split-transport inference worker owns or shares.
struct SplitWorkerLinks {
    engine: SwitchingEngine,
    input: Consumer<f32>,
    output: Producer<f32>,
    tee: MonitorTee,
    levels: Arc<StreamLevels>,
    resampler: InputResampler,
    servo: DriftServo,
}

/// Builds and starts the split transport (see the module docs and
/// [`Runtime::start_native`]).
pub(super) fn start(
    input_device: AudioDeviceId,
    output_device: AudioDeviceId,
    capture_rate: u32,
    engine: SwitchingEngine,
    levels: Arc<StreamLevels>,
    monitor_state: Arc<AtomicI32>,
) -> Result<Runtime, CoreAudioError> {
    // Validate the rate (and preallocate the conversion state) before
    // touching any audio object.
    let resampler = InputResampler::new(capture_rate, NATIVE_CHUNK_SAMPLES)
        .map_err(|error| CoreAudioError::Worker(error.to_string()))?;
    let samples_ready = Arc::new(DispatchSemaphore::new()?);

    let (input_producer, input_consumer) = RingBuffer::new(RING_CAPACITY);
    let (mut output_producer, output_consumer) = RingBuffer::new(RING_CAPACITY);
    // Prime the output ring at the servo's occupancy target so playback
    // starts with the jitter cushion in place.
    for _ in 0..OUTPUT_PRIME_SAMPLES {
        let _ignored = output_producer.push(0.0);
    }
    let faulted = Arc::new(AtomicBool::new(false));
    let frames = Arc::new(AtomicU64::new(0));

    // Capture half: input-only AUHAL at the microphone's native rate.
    let mut capture_unit = AuhalUnit::create()?;
    configure_capture_auhal(capture_unit.raw(), input_device, capture_rate)?;
    let capture_context = ContextGuard::new(CaptureContext {
        unit: capture_unit.raw(),
        input: input_producer,
        buffer: vec![0.0; MAX_CAPTURE_FRAMES],
        faulted: Arc::clone(&faulted),
        samples_ready: Arc::clone(&samples_ready),
        frames: Arc::clone(&frames),
    });
    attach_input_callback(capture_unit.raw(), capture_context.raw().cast())?;
    capture_unit.initialize()?;
    let workgroup = audio_workgroup(capture_unit.raw())?;

    // Output half: output-only AUHAL on the virtual output at 48 kHz.
    let mut output_unit = AuhalUnit::create()?;
    configure_output_auhal(output_unit.raw(), output_device)?;
    let output_context = ContextGuard::new(OutputContext {
        output: output_consumer,
    });
    attach_render_callback(
        output_unit.raw(),
        output_render_callback,
        output_context.raw().cast(),
        "AudioUnitSetProperty(split output render callback)",
    )?;
    output_unit.initialize()?;

    let (monitor_control, tee) = monitor::monitor_pair(monitor_state);
    let shutdown = Arc::new(AtomicBool::new(false));
    let worker_shutdown = Arc::clone(&shutdown);
    let worker_fault = Arc::clone(&faulted);
    let worker_semaphore = Arc::clone(&samples_ready);
    let links = SplitWorkerLinks {
        engine,
        input: input_consumer,
        output: output_producer,
        tee,
        levels,
        resampler,
        servo: DriftServo::new(OUTPUT_PRIME_SAMPLES),
    };
    let worker = thread::Builder::new()
        .name("noican-inference".to_owned())
        .spawn(move || {
            split_processing_loop(
                links,
                &worker_shutdown,
                &worker_fault,
                &worker_semaphore,
                workgroup,
            );
        })
        .map_err(|error| CoreAudioError::Worker(error.to_string()))?;

    // Start the units, unwinding everything on failure (the RAII guards
    // release the units and contexts; the worker is joined explicitly).
    let stop_worker = |error: CoreAudioError| {
        shutdown.store(true, Ordering::Release);
        samples_ready.signal();
        error
    };
    if let Err(error) = start_output_unit(capture_unit.raw(), "AudioOutputUnitStart(capture)") {
        let error = stop_worker(error);
        let _ignored = worker.join();
        return Err(error);
    }
    if let Err(error) = start_output_unit(output_unit.raw(), "AudioOutputUnitStart(virtual output)")
    {
        stop_output_unit(capture_unit.raw());
        let error = stop_worker(error);
        let _ignored = worker.join();
        return Err(error);
    }
    Ok(Runtime {
        transport: Transport::Split {
            capture_unit: capture_unit.into_raw() as usize,
            capture_context: capture_context.into_raw() as usize,
            output_unit: output_unit.into_raw() as usize,
            output_context: output_context.into_raw() as usize,
        },
        shutdown,
        faulted,
        samples_ready,
        frames,
        worker: Some(worker),
        running: true,
        monitor: monitor_control,
    })
}

/// Input-only AUHAL on the microphone: output disabled, capture client
/// format at the device's native rate (AUHAL performs no sample-rate
/// conversion on the input side, so the client rate must match).
fn configure_capture_auhal(
    unit: AudioUnit,
    device: AudioDeviceId,
    capture_rate: u32,
) -> Result<(), CoreAudioError> {
    let enabled = 1_u32;
    let disabled = 0_u32;
    set_property(
        unit,
        AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
        AUDIO_UNIT_SCOPE_INPUT,
        INPUT_BUS,
        &enabled,
        "enable capture AUHAL input",
    )?;
    set_property(
        unit,
        AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
        AUDIO_UNIT_SCOPE_OUTPUT,
        OUTPUT_BUS,
        &disabled,
        "disable capture AUHAL output",
    )?;
    set_property(
        unit,
        AUDIO_OUTPUT_UNIT_PROPERTY_CURRENT_DEVICE,
        AUDIO_UNIT_SCOPE_GLOBAL,
        OUTPUT_BUS,
        &device,
        "select capture device",
    )?;
    let format = pcm_format_at(f64::from(capture_rate), 1);
    set_property(
        unit,
        AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
        AUDIO_UNIT_SCOPE_OUTPUT,
        INPUT_BUS,
        &format,
        "set capture client format",
    )?;
    // Bound the callback size so the preallocated render buffer always
    // suffices (the callback never allocates).
    let max_frames = u32::try_from(MAX_CAPTURE_FRAMES)
        .map_err(|error| CoreAudioError::Worker(format!("frame bound overflow: {error}")))?;
    set_property(
        unit,
        AUDIO_UNIT_PROPERTY_MAXIMUM_FRAMES_PER_SLICE,
        AUDIO_UNIT_SCOPE_GLOBAL,
        OUTPUT_BUS,
        &max_frames,
        "set capture frame bound",
    )
}

/// Output-only AUHAL on the virtual output: input disabled, mono 48 kHz
/// engine samples rendered as interleaved stereo (the device is the
/// 48 kHz Noican/`BlackHole` loopback, so no device-side conversion is
/// involved).
fn configure_output_auhal(unit: AudioUnit, device: AudioDeviceId) -> Result<(), CoreAudioError> {
    let enabled = 1_u32;
    let disabled = 0_u32;
    set_property(
        unit,
        AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
        AUDIO_UNIT_SCOPE_INPUT,
        INPUT_BUS,
        &disabled,
        "disable split output AUHAL input",
    )?;
    set_property(
        unit,
        AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
        AUDIO_UNIT_SCOPE_OUTPUT,
        OUTPUT_BUS,
        &enabled,
        "enable split output AUHAL output",
    )?;
    set_property(
        unit,
        AUDIO_OUTPUT_UNIT_PROPERTY_CURRENT_DEVICE,
        AUDIO_UNIT_SCOPE_GLOBAL,
        OUTPUT_BUS,
        &device,
        "select virtual output device",
    )?;
    let format = pcm_format(2);
    set_property(
        unit,
        AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
        AUDIO_UNIT_SCOPE_INPUT,
        OUTPUT_BUS,
        &format,
        "set split output render format",
    )
}

/// Registers `context` as the capture unit's input callback (the
/// input-only AUHAL notification slot, distinct from the render-callback
/// slot used by output units).
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

/// Inference worker of the split transport: drains native-rate capture
/// samples, converts them to 48 kHz with the drift-compensated
/// resampler, and runs the unchanged engine in 10 ms blocks. All state
/// is preallocated; the loop never locks (docs/tech-research.md §9).
fn split_processing_loop(
    links: SplitWorkerLinks,
    shutdown: &Arc<AtomicBool>,
    faulted: &Arc<AtomicBool>,
    samples_ready: &DispatchSemaphore,
    workgroup: usize,
) {
    let SplitWorkerLinks {
        mut engine,
        mut input,
        mut output,
        mut tee,
        levels,
        mut resampler,
        mut servo,
    } = links;
    levels.reset();
    let membership = WorkgroupGuard::join(workgroup);
    if !membership.joined() {
        faulted.store(true, Ordering::Release);
    }
    let factor = resampler.factor();
    let output_capacity = output.buffer().capacity();
    let mut native_chunk = [0.0_f32; NATIVE_CHUNK_SAMPLES];
    // Converted samples per pass: at most chunk × factor, slightly
    // modulated by the drift correction.
    let mut converted: Vec<f32> = Vec::with_capacity(NATIVE_CHUNK_SAMPLES * 6 + 16);
    // Engine-rate FIFO between conversion output and 10 ms blocks.
    let mut pending: std::collections::VecDeque<f32> =
        std::collections::VecDeque::with_capacity(NATIVE_CHUNK_SAMPLES * 6 + WORKER_BLOCK_SAMPLES);
    let mut input_block = [0.0_f32; WORKER_BLOCK_SAMPLES];
    let mut output_block = [0.0_f32; WORKER_BLOCK_SAMPLES];
    while !shutdown.load(Ordering::Acquire) {
        let mut popped = 0;
        while popped < NATIVE_CHUNK_SAMPLES {
            match input.pop() {
                Ok(sample) => {
                    native_chunk[popped] = sample;
                    popped += 1;
                }
                Err(_empty) => break,
            }
        }
        if popped == 0 {
            // Block until the capture callback signals more input (or
            // the timeout elapses, so shutdown is always noticed).
            samples_ready.wait_ns(WORKER_WAIT_NS);
            continue;
        }
        converted.clear();
        resampler.process(&native_chunk[..popped], &mut converted);
        pending.extend(converted.iter().copied());
        while pending.len() >= WORKER_BLOCK_SAMPLES {
            for slot in &mut input_block {
                *slot = pending.pop_front().unwrap_or(0.0);
            }
            run_block(
                &mut engine,
                &input_block,
                &mut output_block,
                &levels,
                &mut tee,
                &mut output,
                faulted,
            );
            // Drift servo: total samples buffered between the capture
            // clock (producer) and the output clock (consumer), in
            // engine-rate samples. A trend here *is* clock drift.
            let buffered = (output_capacity - output.slots())
                + pending.len()
                + input.slots().saturating_mul(factor);
            resampler.set_drift_ppm(servo.update(buffered));
        }
    }
    drop(membership);
    // Meters read 0 whenever no worker is running (engine stopped).
    levels.reset();
}

/// Input callback of the capture AUHAL: renders the microphone's frames
/// into the preallocated buffer and pushes them to the capture ring.
/// Real-time rules (docs/tech-research.md §9): no allocation, no locks;
/// ring overrun drops samples; a render failure flags the fault and
/// returns cleanly.
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
    let context = unsafe { &mut *context.cast::<CaptureContext>() };
    let frames = usize::try_from(frame_count).unwrap_or(0);
    if frames == 0 {
        return NO_ERR;
    }
    if frames > context.buffer.len() {
        // Cannot happen while kAudioUnitProperty_MaximumFramesPerSlice
        // holds; never allocate on the audio thread to compensate.
        context.faulted.store(true, Ordering::Release);
        return PARAM_ERR;
    }
    let byte_size = u32::try_from(frames * size_of::<f32>()).unwrap_or(0);
    let mut list = AudioBufferList {
        number_buffers: 1,
        buffers: [AudioBuffer {
            number_channels: 1,
            data_byte_size: byte_size,
            data: context.buffer.as_mut_ptr().cast(),
        }],
    };
    let status = unsafe {
        AudioUnitRender(
            context.unit,
            action_flags,
            timestamp,
            INPUT_BUS,
            frame_count,
            &raw mut list,
        )
    };
    if status != NO_ERR {
        context.faulted.store(true, Ordering::Release);
        return NO_ERR;
    }
    for sample in &context.buffer[..frames] {
        let _ignored = context.input.push(*sample);
    }
    context
        .frames
        .fetch_add(u64::from(frame_count), Ordering::Relaxed);
    // Wake the inference worker (never blocks; see DispatchSemaphore).
    context.samples_ready.signal();
    NO_ERR
}

/// Render callback of the output-only AUHAL on the virtual output: pops
/// engine-rate samples from the output ring into the device buffer,
/// duplicating the mono engine signal into every device channel.
/// Underrun renders silence — it never blocks (docs/tech-research.md
/// §9); systematic underrun is prevented by the drift servo, not here.
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
    let buffer_list = unsafe { &mut *data };
    if buffer_list.number_buffers == 0 {
        return PARAM_ERR;
    }
    let buffer = &mut buffer_list.buffers[0];
    if buffer.data.is_null() {
        return PARAM_ERR;
    }
    let available = usize::try_from(buffer.data_byte_size)
        .unwrap_or(0)
        .saturating_div(size_of::<f32>());
    let channels = usize::try_from(buffer.number_channels).unwrap_or(0).max(1);
    let frames = usize::try_from(frame_count)
        .unwrap_or(0)
        .min(available.saturating_div(channels));
    let samples =
        unsafe { std::slice::from_raw_parts_mut(buffer.data.cast::<f32>(), frames * channels) };
    for frame in samples.chunks_exact_mut(channels) {
        let value = context.output.pop().unwrap_or(0.0);
        for channel in frame {
            *channel = value;
        }
    }
    NO_ERR
}
