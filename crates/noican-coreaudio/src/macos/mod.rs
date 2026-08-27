//! Audited AUHAL FFI and real-time callbacks for the main transport.
//!
//! The preview monitor's AUHAL lifecycle lives in the [`monitor`]
//! submodule; shared plumbing (unit creation, property setting, render
//! callback attachment, disposal) is defined here and reused there.

#![expect(
    unsafe_code,
    reason = "AUHAL and os_workgroup are C APIs; unsafe code is confined to this module tree and callbacks only touch preallocated buffers and lock-free rings"
)]

use std::{
    ffi::c_void,
    mem::size_of,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
};

use noican_core::SwitchingEngine;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::monitor::{MonitorTee, fourcc};
use crate::observe::StreamLevels;
use crate::{CoreAudioError, WORKER_BLOCK_SAMPLES};

mod monitor;

use monitor::MonitorControl;
pub use monitor::{check_monitor_device, check_monitor_target};

type OSStatus = i32;
type AudioUnit = *mut c_void;
type AudioComponent = *mut c_void;
type AudioDeviceId = u32;
type AudioUnitRenderActionFlags = u32;

const NO_ERR: OSStatus = 0;
const PARAM_ERR: OSStatus = -50;
const INPUT_BUS: u32 = 1;
const OUTPUT_BUS: u32 = 0;
const RING_CAPACITY: usize = 48_000;

const AUDIO_UNIT_TYPE_OUTPUT: u32 = fourcc(*b"auou");
const AUDIO_UNIT_SUBTYPE_HAL_OUTPUT: u32 = fourcc(*b"ahal");
const AUDIO_UNIT_MANUFACTURER_APPLE: u32 = fourcc(*b"appl");
const AUDIO_FORMAT_LINEAR_PCM: u32 = fourcc(*b"lpcm");

const AUDIO_OUTPUT_UNIT_PROPERTY_CURRENT_DEVICE: u32 = 2_000;
const AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO: u32 = 2_003;
const AUDIO_OUTPUT_UNIT_PROPERTY_OS_WORKGROUP: u32 = 2_015;
const AUDIO_UNIT_PROPERTY_STREAM_FORMAT: u32 = 8;
const AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK: u32 = 23;

const AUDIO_UNIT_SCOPE_GLOBAL: u32 = 0;
const AUDIO_UNIT_SCOPE_INPUT: u32 = 1;
const AUDIO_UNIT_SCOPE_OUTPUT: u32 = 2;

const AUDIO_FORMAT_FLAG_IS_FLOAT: u32 = 1;
const AUDIO_FORMAT_FLAG_IS_PACKED: u32 = 1 << 3;

#[repr(C)]
struct AudioComponentDescription {
    type_id: u32,
    subtype: u32,
    manufacturer: u32,
    flags: u32,
    flags_mask: u32,
}

#[repr(C)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    number_buffers: u32,
    buffers: [AudioBuffer; 1],
}

type RenderCallback = unsafe extern "C" fn(
    *mut c_void,
    *mut AudioUnitRenderActionFlags,
    *const c_void,
    u32,
    u32,
    *mut AudioBufferList,
) -> OSStatus;

#[repr(C)]
struct AudioUnitRenderCallback {
    callback: Option<RenderCallback>,
    context: *mut c_void,
}

#[repr(C)]
struct WorkgroupJoinToken {
    signature: u32,
    opaque: [i8; 36],
}

impl Default for WorkgroupJoinToken {
    fn default() -> Self {
        Self {
            signature: 0,
            opaque: [0; 36],
        }
    }
}

#[link(name = "AudioUnit", kind = "framework")]
unsafe extern "C" {
    fn AudioComponentFindNext(
        component: AudioComponent,
        description: *const AudioComponentDescription,
    ) -> AudioComponent;
    fn AudioComponentInstanceNew(component: AudioComponent, instance: *mut AudioUnit) -> OSStatus;
    fn AudioComponentInstanceDispose(instance: AudioUnit) -> OSStatus;
    fn AudioUnitSetProperty(
        unit: AudioUnit,
        property: u32,
        scope: u32,
        element: u32,
        data: *const c_void,
        data_size: u32,
    ) -> OSStatus;
    fn AudioUnitGetProperty(
        unit: AudioUnit,
        property: u32,
        scope: u32,
        element: u32,
        data: *mut c_void,
        data_size: *mut u32,
    ) -> OSStatus;
    fn AudioUnitInitialize(unit: AudioUnit) -> OSStatus;
    fn AudioUnitUninitialize(unit: AudioUnit) -> OSStatus;
    fn AudioOutputUnitStart(unit: AudioUnit) -> OSStatus;
    fn AudioOutputUnitStop(unit: AudioUnit) -> OSStatus;
    fn AudioUnitRender(
        unit: AudioUnit,
        action_flags: *mut AudioUnitRenderActionFlags,
        timestamp: *const c_void,
        output_bus_number: u32,
        frame_count: u32,
        data: *mut AudioBufferList,
    ) -> OSStatus;
}

#[link(name = "System")]
unsafe extern "C" {
    fn os_workgroup_join(workgroup: *mut c_void, token: *mut WorkgroupJoinToken) -> i32;
    fn os_workgroup_leave(workgroup: *mut c_void, token: *mut WorkgroupJoinToken);
}

const DISPATCH_TIME_NOW: u64 = 0;

#[link(name = "System")]
unsafe extern "C" {
    fn dispatch_semaphore_create(value: isize) -> *mut c_void;
    fn dispatch_semaphore_signal(semaphore: *mut c_void) -> isize;
    fn dispatch_semaphore_wait(semaphore: *mut c_void, timeout: u64) -> isize;
    fn dispatch_time(when: u64, delta: i64) -> u64;
    fn dispatch_release(object: *mut c_void);
}

/// Owned libdispatch semaphore. Wakes the inference worker when the render
/// callback has produced samples, so the worker blocks between device
/// callbacks instead of busy-spinning (`dispatch_semaphore_signal` is the
/// mechanism Apple's audio-workgroup example uses from real-time threads:
/// it never blocks or allocates).
#[derive(Debug)]
struct DispatchSemaphore(*mut c_void);

// The semaphore handle is shared with the render callback and the worker
// thread; libdispatch semaphores are internally thread-safe.
unsafe impl Send for DispatchSemaphore {}
unsafe impl Sync for DispatchSemaphore {}

impl DispatchSemaphore {
    fn new() -> Result<Self, CoreAudioError> {
        let semaphore = unsafe { dispatch_semaphore_create(0) };
        if semaphore.is_null() {
            return Err(CoreAudioError::Worker(
                "dispatch_semaphore_create returned null".to_owned(),
            ));
        }
        Ok(Self(semaphore))
    }

    fn signal(&self) {
        unsafe {
            let _ignored = dispatch_semaphore_signal(self.0);
        }
    }

    fn wait_ns(&self, timeout_ns: i64) {
        unsafe {
            let deadline = dispatch_time(DISPATCH_TIME_NOW, timeout_ns);
            let _timed_out = dispatch_semaphore_wait(self.0, deadline);
        }
    }
}

impl Drop for DispatchSemaphore {
    fn drop(&mut self) {
        unsafe {
            dispatch_release(self.0);
        }
    }
}

/// Owns the AUHAL instance during setup: uninitializes (when reached) and
/// disposes it on any early error return. [`AuhalUnit::into_raw`] defuses
/// the guard once the runtime takes over ownership.
struct AuhalUnit {
    unit: AudioUnit,
    initialized: bool,
}

impl AuhalUnit {
    fn create() -> Result<Self, CoreAudioError> {
        create_auhal().map(|unit| Self {
            unit,
            initialized: false,
        })
    }

    const fn raw(&self) -> AudioUnit {
        self.unit
    }

    fn initialize(&mut self) -> Result<(), CoreAudioError> {
        check_status(
            unsafe { AudioUnitInitialize(self.unit) },
            "AudioUnitInitialize",
        )?;
        self.initialized = true;
        Ok(())
    }

    const fn into_raw(self) -> AudioUnit {
        let unit = self.unit;
        std::mem::forget(self);
        unit
    }
}

impl Drop for AuhalUnit {
    fn drop(&mut self) {
        unsafe {
            if self.initialized {
                let _ignored = AudioUnitUninitialize(self.unit);
            }
            let _ignored = AudioComponentInstanceDispose(self.unit);
        }
    }
}

/// Owns the heap-allocated [`CallbackContext`] during setup, reclaiming it
/// on any early error return (safe because the audio unit is never started
/// on those paths, so no callback can observe the pointer).
struct ContextGuard(*mut CallbackContext);

impl ContextGuard {
    const fn raw(&self) -> *mut CallbackContext {
        self.0
    }

    const fn into_raw(self) -> *mut CallbackContext {
        let context = self.0;
        std::mem::forget(self);
        context
    }
}

impl Drop for ContextGuard {
    fn drop(&mut self) {
        unsafe {
            drop(Box::from_raw(self.0));
        }
    }
}

struct CallbackContext {
    unit: AudioUnit,
    input: Producer<f32>,
    output: Consumer<f32>,
    faulted: Arc<AtomicBool>,
    samples_ready: Arc<DispatchSemaphore>,
    /// Heartbeat: input frames delivered since start (relaxed atomic add is
    /// real-time safe). A counter that stops advancing while "running"
    /// means the device stopped calling back (unplugged microphone,
    /// coreaudiod restart, post-sleep stall).
    frames: Arc<AtomicU64>,
}

/// Everything the inference worker owns or shares. Bundled so the worker
/// spawn passes one value instead of a long argument list.
struct WorkerLinks {
    engine: SwitchingEngine,
    input: Consumer<f32>,
    output: Producer<f32>,
    tee: MonitorTee,
    levels: Arc<StreamLevels>,
}

/// Running AUHAL instance and inference worker.
#[derive(Debug)]
pub struct Runtime {
    unit: usize,
    callback: usize,
    shutdown: Arc<AtomicBool>,
    faulted: Arc<AtomicBool>,
    samples_ready: Arc<DispatchSemaphore>,
    frames: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
    running: bool,
    /// Control-plane half of the preview monitor (the worker half is the
    /// [`MonitorTee`] owned by the inference worker).
    monitor: MonitorControl,
}

impl Runtime {
    /// Opens an initialized AUHAL on a private Aggregate Device.
    ///
    /// `aggregate_device` must contain the selected physical input and the
    /// `BlackHole` output subdevice with drift compensation configured by the
    /// Swift control plane.
    ///
    /// `levels` receives per-block input/output peak meters from the
    /// inference worker for the lifetime of this runtime; the worker
    /// resets it to silence on start and on exit. `monitor_state` is the
    /// shared [`crate::monitor::MonitorState`] cell: monitor toggles move
    /// it between off and playing here, the worker's feedback guard moves
    /// it to tripped, and keeping it caller-owned lets the control plane
    /// poll it without any lock. It reads off whenever this runtime is
    /// down (reset on start and by the stop path's disable).
    ///
    /// # Errors
    ///
    /// Returns [`CoreAudioError`] when AUHAL setup or worker startup fails.
    /// Every error path releases the AUHAL instance, the callback context,
    /// and the worker (RAII guards; nothing leaks on failed starts).
    pub fn start(
        aggregate_device: u32,
        engine: SwitchingEngine,
        levels: Arc<StreamLevels>,
        monitor_state: Arc<AtomicI32>,
    ) -> Result<Self, CoreAudioError> {
        let samples_ready = Arc::new(DispatchSemaphore::new()?);
        let mut unit = AuhalUnit::create()?;
        configure_auhal(unit.raw(), aggregate_device)?;

        let (input_producer, input_consumer) = RingBuffer::new(RING_CAPACITY);
        let (output_producer, output_consumer) = RingBuffer::new(RING_CAPACITY);
        let faulted = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicU64::new(0));
        let context = ContextGuard(Box::into_raw(Box::new(CallbackContext {
            unit: unit.raw(),
            input: input_producer,
            output: output_consumer,
            faulted: Arc::clone(&faulted),
            samples_ready: Arc::clone(&samples_ready),
            frames: Arc::clone(&frames),
        })));
        attach_render_callback(
            unit.raw(),
            render_callback,
            context.raw().cast(),
            "AudioUnitSetProperty(render callback)",
        )?;
        unit.initialize()?;

        let workgroup = audio_workgroup(unit.raw())?;
        let (monitor_control, tee) = monitor::monitor_pair(monitor_state);
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_fault = Arc::clone(&faulted);
        let worker_semaphore = Arc::clone(&samples_ready);
        let links = WorkerLinks {
            engine,
            input: input_consumer,
            output: output_producer,
            tee,
            levels,
        };
        let worker = thread::Builder::new()
            .name("noican-inference".to_owned())
            .spawn(move || {
                processing_loop(
                    links,
                    &worker_shutdown,
                    &worker_fault,
                    &worker_semaphore,
                    workgroup,
                );
            })
            .map_err(|error| CoreAudioError::Worker(error.to_string()))?;

        if let Err(error) = start_output_unit(unit.raw(), "AudioOutputUnitStart") {
            shutdown.store(true, Ordering::Release);
            samples_ready.signal();
            let _ignored = worker.join();
            // `unit` and `context` guards clean up on drop.
            return Err(error);
        }
        Ok(Self {
            unit: unit.into_raw() as usize,
            callback: context.into_raw() as usize,
            shutdown,
            faulted,
            samples_ready,
            frames,
            worker: Some(worker),
            running: true,
            monitor: monitor_control,
        })
    }

    /// Stops callbacks, leaves the audio workgroup, and disposes AUHAL.
    pub fn stop(&mut self) {
        if !self.running {
            return;
        }
        self.monitor.disable();
        let unit = self.unit as AudioUnit;
        stop_output_unit(unit);
        self.shutdown.store(true, Ordering::Release);
        self.samples_ready.signal();
        if let Some(worker) = self.worker.take() {
            let _ignored = worker.join();
        }
        dispose_unit(unit);
        unsafe {
            drop(Box::from_raw(self.callback as *mut CallbackContext));
        }
        self.running = false;
    }

    /// Whether AUHAL has started and has not been stopped.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.running
    }

    /// Whether a callback or inference call has failed.
    #[must_use]
    pub fn is_faulted(&self) -> bool {
        self.faulted.load(Ordering::Acquire)
    }

    /// Total input frames the render callback has delivered since start.
    /// A value that stops advancing while running means the device stopped
    /// calling back; the control plane uses it as a heartbeat.
    #[must_use]
    pub fn frames_processed(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    /// Enables or disables the preview self-monitor: the worker tees its
    /// processed output into the monitor ring and a second, output-only
    /// AUHAL plays it on the system default output device.
    ///
    /// The monitor target is resolved at enable time; it does not follow a
    /// later default-output change (re-enable preview to pick it up).
    /// Monitor failures never affect the meeting-facing path, and both
    /// directions are idempotent. Enabling clears a pending feedback trip
    /// and re-arms a still-running monitor.
    ///
    /// Enabling can take a while (`AudioOutputUnitStart` on a sleeping
    /// output device); callers that also poll lock-guarded status getters
    /// should gate concurrent control calls (the menu app uses its busy
    /// flag for this).
    ///
    /// # Errors
    ///
    /// Returns [`CoreAudioError::NotRunning`] after [`Runtime::stop`], a
    /// refusal from [`crate::monitor::classify_monitor_target`] when the
    /// default output must not receive the preview (loopback, aggregate,
    /// or built-in speakers), and other [`CoreAudioError`] values when the
    /// monitor AUHAL cannot start.
    pub fn set_monitor(&mut self, enabled: bool) -> Result<(), CoreAudioError> {
        if enabled {
            if !self.running {
                return Err(CoreAudioError::NotRunning);
            }
            self.monitor.enable()
        } else {
            self.monitor.disable();
            Ok(())
        }
    }

    /// Device the running preview monitor plays on (resolved and vetted
    /// at enable time), or `None` while the monitor is down. The control
    /// plane re-vets this device with [`check_monitor_device`] while the
    /// preview plays: the safety decision made at enable time can be
    /// invalidated later (headphone jack unplugged → the same built-in
    /// device flips to the internal speakers) without the monitor
    /// noticing.
    #[must_use]
    pub fn monitor_device(&self) -> Option<u32> {
        self.monitor.active_device()
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop();
    }
}

fn create_auhal() -> Result<AudioUnit, CoreAudioError> {
    let description = AudioComponentDescription {
        type_id: AUDIO_UNIT_TYPE_OUTPUT,
        subtype: AUDIO_UNIT_SUBTYPE_HAL_OUTPUT,
        manufacturer: AUDIO_UNIT_MANUFACTURER_APPLE,
        flags: 0,
        flags_mask: 0,
    };
    let component = unsafe { AudioComponentFindNext(ptr::null_mut(), &raw const description) };
    if component.is_null() {
        return Err(CoreAudioError::MissingAuHal);
    }
    let mut unit = ptr::null_mut();
    check_status(
        unsafe { AudioComponentInstanceNew(component, &raw mut unit) },
        "AudioComponentInstanceNew(AUHAL)",
    )?;
    Ok(unit)
}

fn configure_auhal(unit: AudioUnit, device: AudioDeviceId) -> Result<(), CoreAudioError> {
    let enabled = 1_u32;
    set_property(
        unit,
        AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
        AUDIO_UNIT_SCOPE_INPUT,
        INPUT_BUS,
        &enabled,
        "enable AUHAL input",
    )?;
    set_property(
        unit,
        AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
        AUDIO_UNIT_SCOPE_OUTPUT,
        OUTPUT_BUS,
        &enabled,
        "enable AUHAL output",
    )?;
    set_property(
        unit,
        AUDIO_OUTPUT_UNIT_PROPERTY_CURRENT_DEVICE,
        AUDIO_UNIT_SCOPE_GLOBAL,
        OUTPUT_BUS,
        &device,
        "select Aggregate Device",
    )?;
    let format = pcm_format(1);
    set_property(
        unit,
        AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
        AUDIO_UNIT_SCOPE_OUTPUT,
        INPUT_BUS,
        &format,
        "set AUHAL capture format",
    )?;
    set_property(
        unit,
        AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
        AUDIO_UNIT_SCOPE_INPUT,
        OUTPUT_BUS,
        &format,
        "set AUHAL render format",
    )
}

/// Packed-float PCM at the 48 kHz engine rate with `channels` interleaved
/// channels (shared by the main capture/render and monitor formats).
const fn pcm_format(channels: u32) -> AudioStreamBasicDescription {
    AudioStreamBasicDescription {
        sample_rate: 48_000.0,
        format_id: AUDIO_FORMAT_LINEAR_PCM,
        format_flags: AUDIO_FORMAT_FLAG_IS_FLOAT | AUDIO_FORMAT_FLAG_IS_PACKED,
        bytes_per_packet: 4 * channels,
        frames_per_packet: 1,
        bytes_per_frame: 4 * channels,
        channels_per_frame: channels,
        bits_per_channel: 32,
        reserved: 0,
    }
}

/// Registers `callback`/`context` as the render provider on an output
/// unit's input scope (the AUHAL output-element callback slot).
fn attach_render_callback(
    unit: AudioUnit,
    callback: RenderCallback,
    context: *mut c_void,
    operation: &'static str,
) -> Result<(), CoreAudioError> {
    let property = AudioUnitRenderCallback {
        callback: Some(callback),
        context,
    };
    check_status(
        unsafe {
            AudioUnitSetProperty(
                unit,
                AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK,
                AUDIO_UNIT_SCOPE_INPUT,
                OUTPUT_BUS,
                (&raw const property).cast(),
                size_u32::<AudioUnitRenderCallback>()?,
            )
        },
        operation,
    )
}

fn start_output_unit(unit: AudioUnit, operation: &'static str) -> Result<(), CoreAudioError> {
    check_status(unsafe { AudioOutputUnitStart(unit) }, operation)
}

fn stop_output_unit(unit: AudioUnit) {
    unsafe {
        let _ignored = AudioOutputUnitStop(unit);
    }
}

/// Uninitializes and disposes a stopped audio unit. Callbacks must no
/// longer be running (the unit was stopped, and for the main transport
/// the worker joined).
fn dispose_unit(unit: AudioUnit) {
    unsafe {
        let _ignored = AudioUnitUninitialize(unit);
        let _ignored = AudioComponentInstanceDispose(unit);
    }
}

fn set_property<T>(
    unit: AudioUnit,
    property: u32,
    scope: u32,
    element: u32,
    value: &T,
    operation: &'static str,
) -> Result<(), CoreAudioError> {
    check_status(
        unsafe {
            AudioUnitSetProperty(
                unit,
                property,
                scope,
                element,
                ptr::from_ref(value).cast(),
                size_u32::<T>()?,
            )
        },
        operation,
    )
}

fn audio_workgroup(unit: AudioUnit) -> Result<usize, CoreAudioError> {
    let mut workgroup: *mut c_void = ptr::null_mut();
    let mut size = size_u32::<*mut c_void>()?;
    check_status(
        unsafe {
            AudioUnitGetProperty(
                unit,
                AUDIO_OUTPUT_UNIT_PROPERTY_OS_WORKGROUP,
                AUDIO_UNIT_SCOPE_GLOBAL,
                OUTPUT_BUS,
                (&raw mut workgroup).cast(),
                &raw mut size,
            )
        },
        "AudioUnitGetProperty(OSWorkgroup)",
    )?;
    if workgroup.is_null() {
        return Err(CoreAudioError::Worker(
            "AUHAL returned a null audio workgroup".to_owned(),
        ));
    }
    Ok(workgroup as usize)
}

/// Longest the worker sleeps waiting for the render callback's semaphore
/// signal before re-checking the shutdown flag (a fraction of the ~5.3 ms
/// device period at 256 frames / 48 kHz).
const WORKER_WAIT_NS: i64 = 2_000_000;

fn processing_loop(
    links: WorkerLinks,
    shutdown: &Arc<AtomicBool>,
    faulted: &Arc<AtomicBool>,
    samples_ready: &DispatchSemaphore,
    workgroup: usize,
) {
    let WorkerLinks {
        mut engine,
        mut input,
        mut output,
        mut tee,
        levels,
    } = links;
    levels.reset();
    let workgroup = workgroup as *mut c_void;
    let mut token = WorkgroupJoinToken::default();
    let joined = unsafe { os_workgroup_join(workgroup, &raw mut token) } == 0;
    if !joined {
        faulted.store(true, Ordering::Release);
    }
    let mut input_block = [0.0_f32; WORKER_BLOCK_SAMPLES];
    let mut output_block = [0.0_f32; WORKER_BLOCK_SAMPLES];
    let mut position = 0;
    while !shutdown.load(Ordering::Acquire) {
        match input.pop() {
            Ok(sample) => {
                input_block[position] = sample;
                position += 1;
                if position == WORKER_BLOCK_SAMPLES {
                    if engine
                        .process_block(&input_block, &mut output_block)
                        .is_err()
                    {
                        output_block.fill(0.0);
                        faulted.store(true, Ordering::Release);
                    }
                    levels.update(&input_block, &output_block);
                    // Preview branch: the tee only copies into its
                    // preallocated monitor ring (skipped entirely while
                    // disarmed) and disarms itself on sustained feedback;
                    // it never delays the main path below.
                    let _teed = tee.feed(&output_block);
                    for sample in output_block {
                        let _ignored = output.push(sample);
                    }
                    position = 0;
                }
            }
            // Block until the render callback signals more input (or the
            // timeout elapses, so shutdown is always noticed). Busy-spinning
            // here would burn a core for the whole session and distort the
            // os_workgroup's power/deadline balancing.
            Err(_empty) => samples_ready.wait_ns(WORKER_WAIT_NS),
        }
    }
    if joined {
        unsafe {
            os_workgroup_leave(workgroup, &raw mut token);
        }
    }
    // Meters read 0 whenever no worker is running (engine stopped).
    levels.reset();
}

unsafe extern "C" fn render_callback(
    context: *mut c_void,
    action_flags: *mut AudioUnitRenderActionFlags,
    timestamp: *const c_void,
    _bus: u32,
    frame_count: u32,
    data: *mut AudioBufferList,
) -> OSStatus {
    if context.is_null() || data.is_null() {
        return PARAM_ERR;
    }
    let context = unsafe { &mut *context.cast::<CallbackContext>() };
    let status = unsafe {
        AudioUnitRender(
            context.unit,
            action_flags,
            timestamp,
            INPUT_BUS,
            frame_count,
            data,
        )
    };
    let buffer_list = unsafe { &mut *data };
    if buffer_list.number_buffers == 0 {
        context.faulted.store(true, Ordering::Release);
        return PARAM_ERR;
    }
    let buffer = &mut buffer_list.buffers[0];
    if buffer.data.is_null() {
        context.faulted.store(true, Ordering::Release);
        return PARAM_ERR;
    }
    let available = usize::try_from(buffer.data_byte_size)
        .unwrap_or(0)
        .saturating_div(size_of::<f32>());
    let requested = usize::try_from(frame_count).unwrap_or(0);
    let sample_count = available.min(requested);
    let samples =
        unsafe { std::slice::from_raw_parts_mut(buffer.data.cast::<f32>(), sample_count) };
    if status != NO_ERR {
        samples.fill(0.0);
        context.faulted.store(true, Ordering::Release);
        return NO_ERR;
    }
    for sample in samples.iter().copied() {
        let _ignored = context.input.push(sample);
    }
    context
        .frames
        .fetch_add(u64::from(frame_count), Ordering::Relaxed);
    // Wake the inference worker (never blocks; see DispatchSemaphore).
    context.samples_ready.signal();
    for sample in samples {
        *sample = context.output.pop().unwrap_or(0.0);
    }
    NO_ERR
}

const fn check_status(status: OSStatus, operation: &'static str) -> Result<(), CoreAudioError> {
    if status == NO_ERR {
        Ok(())
    } else {
        Err(CoreAudioError::Status { operation, status })
    }
}

fn size_u32<T>() -> Result<u32, CoreAudioError> {
    u32::try_from(size_of::<T>())
        .map_err(|error| CoreAudioError::Worker(format!("property size overflow: {error}")))
}
