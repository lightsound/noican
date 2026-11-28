//! See docs/adr/0003-coreaudio-transport.md#coreaudio-module-layout.

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

use crate::CoreAudioError;
use crate::callback::{
    CaptureFeed, MAX_CALLBACK_FRAMES, OutputFeed, capture_byte_size, render_geometry,
};
use crate::monitor::{MonitorTee, fourcc};
use crate::observe::{StreamLevels, WorkerBlockStats};
use crate::pump::{Pump, PumpLinks};
use crate::routing::{VirtualOutputChannels, render_channel_map};

mod monitor;
mod split;

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
const AUDIO_OUTPUT_UNIT_PROPERTY_CHANNEL_MAP: u32 = 2_002;
const AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO: u32 = 2_003;
const AUDIO_OUTPUT_UNIT_PROPERTY_SET_INPUT_CALLBACK: u32 = 2_005;
const AUDIO_OUTPUT_UNIT_PROPERTY_OS_WORKGROUP: u32 = 2_015;
const AUDIO_DEVICE_PROPERTY_BUFFER_FRAME_SIZE: u32 = fourcc(*b"fsiz");
const AUDIO_UNIT_PROPERTY_STREAM_FORMAT: u32 = 8;
const AUDIO_UNIT_PROPERTY_MAXIMUM_FRAMES_PER_SLICE: u32 = 14;
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

impl AudioStreamBasicDescription {
    const EMPTY: Self = Self {
        sample_rate: 0.0,
        format_id: 0,
        format_flags: 0,
        bytes_per_packet: 0,
        frames_per_packet: 0,
        bytes_per_frame: 0,
        channels_per_frame: 0,
        bits_per_channel: 0,
        reserved: 0,
    };
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

#[derive(Debug)]
struct DispatchSemaphore(*mut c_void);

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

struct ContextGuard<T>(*mut T);

impl<T> ContextGuard<T> {
    fn new(context: T) -> Self {
        Self(Box::into_raw(Box::new(context)))
    }

    const fn raw(&self) -> *mut T {
        self.0
    }

    const fn into_raw(self) -> *mut T {
        let context = self.0;
        std::mem::forget(self);
        context
    }
}

impl<T> Drop for ContextGuard<T> {
    fn drop(&mut self) {
        unsafe {
            drop(Box::from_raw(self.0));
        }
    }
}

struct Capture {
    unit: AudioUnit,
    buffer: Vec<f32>,
    feed: CaptureFeed,
    faulted: Arc<AtomicBool>,
    samples_ready: Arc<DispatchSemaphore>,
    frames: Arc<AtomicU64>,
}

impl Capture {
    fn new(unit: AudioUnit, input: Producer<f32>, shared: &Shared) -> Self {
        Self {
            unit,
            buffer: vec![0.0; MAX_CALLBACK_FRAMES],
            feed: CaptureFeed::new(input, Arc::clone(&shared.input_overruns)),
            faulted: Arc::clone(&shared.faulted),
            samples_ready: Arc::clone(&shared.samples_ready),
            frames: Arc::clone(&shared.frames),
        }
    }

    unsafe fn capture(
        &mut self,
        action_flags: *mut AudioUnitRenderActionFlags,
        timestamp: *const c_void,
        frame_count: u32,
    ) -> Result<(), OSStatus> {
        let frames = usize::try_from(frame_count).unwrap_or(0);
        let Some(byte_size) = capture_byte_size(frames, self.buffer.len()) else {
            self.faulted.store(true, Ordering::Release);
            return Err(PARAM_ERR);
        };
        let mut list = AudioBufferList {
            number_buffers: 1,
            buffers: [AudioBuffer {
                number_channels: 1,
                data_byte_size: byte_size,
                data: self.buffer.as_mut_ptr().cast(),
            }],
        };
        let status = unsafe {
            AudioUnitRender(
                self.unit,
                action_flags,
                timestamp,
                INPUT_BUS,
                frame_count,
                &raw mut list,
            )
        };
        if status != NO_ERR {
            self.faulted.store(true, Ordering::Release);
            return Err(NO_ERR);
        }
        self.feed.push(&self.buffer[..frames]);
        self.frames
            .fetch_add(u64::from(frame_count), Ordering::Relaxed);
        self.samples_ready.signal();
        Ok(())
    }
}

struct CallbackContext {
    capture: Capture,
    output: OutputFeed,
}

#[derive(Debug, Clone)]
struct Shared {
    shutdown: Arc<AtomicBool>,
    faulted: Arc<AtomicBool>,
    samples_ready: Arc<DispatchSemaphore>,
    frames: Arc<AtomicU64>,
    underruns: Arc<AtomicU64>,
    input_overruns: Arc<AtomicU64>,
    block_stats: Arc<WorkerBlockStats>,
    worker_realtime: Arc<AtomicBool>,
}

impl Shared {
    fn new() -> Result<Self, CoreAudioError> {
        Ok(Self {
            shutdown: Arc::new(AtomicBool::new(false)),
            faulted: Arc::new(AtomicBool::new(false)),
            samples_ready: Arc::new(DispatchSemaphore::new()?),
            frames: Arc::new(AtomicU64::new(0)),
            underruns: Arc::new(AtomicU64::new(0)),
            input_overruns: Arc::new(AtomicU64::new(0)),
            block_stats: Arc::new(WorkerBlockStats::new()),
            worker_realtime: Arc::new(AtomicBool::new(false)),
        })
    }

    fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.samples_ready.signal();
    }

    fn pump_links(
        &self,
        engine: SwitchingEngine,
        input: Consumer<f32>,
        output: Producer<f32>,
        tee: MonitorTee,
        levels: Arc<StreamLevels>,
    ) -> PumpLinks {
        PumpLinks {
            engine,
            input,
            output,
            tee,
            levels,
            faulted: Arc::clone(&self.faulted),
            input_overruns: Arc::clone(&self.input_overruns),
            block_stats: Arc::clone(&self.block_stats),
        }
    }
}

fn spawn_worker(body: impl FnOnce() + Send + 'static) -> Result<JoinHandle<()>, CoreAudioError> {
    thread::Builder::new()
        .name("noican-inference".to_owned())
        .spawn(body)
        .map_err(|error| CoreAudioError::Worker(error.to_string()))
}

fn stop_worker(shared: &Shared, worker: JoinHandle<()>) {
    shared.request_shutdown();
    let _ignored = worker.join();
}

#[derive(Debug)]
enum Transport {
    Aggregate {
        unit: usize,
        context: usize,
    },
    Split {
        capture_unit: usize,
        capture_context: usize,
        output_unit: usize,
        output_context: usize,
    },
}

impl Transport {
    fn stop_units(&self) {
        match self {
            Self::Aggregate { unit, .. } => stop_output_unit(*unit as AudioUnit),
            Self::Split {
                capture_unit,
                output_unit,
                ..
            } => {
                stop_output_unit(*capture_unit as AudioUnit);
                stop_output_unit(*output_unit as AudioUnit);
            }
        }
    }

    unsafe fn dispose(&self) {
        match self {
            Self::Aggregate { unit, context } => unsafe {
                dispose_unit(*unit as AudioUnit);
                drop(Box::from_raw(*context as *mut CallbackContext));
            },
            Self::Split {
                capture_unit,
                capture_context,
                output_unit,
                output_context,
            } => unsafe {
                dispose_unit(*capture_unit as AudioUnit);
                drop(Box::from_raw(*capture_context as *mut Capture));
                dispose_unit(*output_unit as AudioUnit);
                drop(Box::from_raw(*output_context as *mut split::OutputContext));
            },
        }
    }
}

/// Running AUHAL transport and inference worker.
#[derive(Debug)]
pub struct Runtime {
    transport: Transport,
    shared: Shared,
    worker: Option<JoinHandle<()>>,
    running: bool,
    monitor: MonitorControl,
    routing: String,
}

impl Runtime {
    /// See docs/adr/0004-aggregate-output-routing.md#aggregate-transport-contract.
    ///
    /// # Errors
    ///
    /// Returns [`CoreAudioError`] when AUHAL setup or worker startup fails,
    /// including [`CoreAudioError::OutputRouting`] when `virtual_output`
    /// does not match the output channel count the aggregate reports.
    /// Every error path releases the AUHAL instance, the callback context,
    /// and the worker (RAII guards; nothing leaks on failed starts).
    pub fn start(
        aggregate_device: u32,
        virtual_output: VirtualOutputChannels,
        engine: SwitchingEngine,
        levels: Arc<StreamLevels>,
        monitor_state: Arc<AtomicI32>,
    ) -> Result<Self, CoreAudioError> {
        let shared = Shared::new()?;
        let mut unit = AuhalUnit::create()?;
        let requested_routing = configure_auhal(unit.raw(), aggregate_device, virtual_output)?;

        let (input_producer, input_consumer) = RingBuffer::new(RING_CAPACITY);
        let (output_producer, output_consumer) = RingBuffer::new(RING_CAPACITY);
        let context = ContextGuard::new(CallbackContext {
            capture: Capture::new(unit.raw(), input_producer, &shared),
            output: OutputFeed::new(output_consumer, Arc::clone(&shared.underruns)),
        });
        attach_render_callback(
            unit.raw(),
            render_callback,
            context.raw().cast(),
            "AudioUnitSetProperty(render callback)",
        )?;
        unit.initialize()?;
        let routing = format!(
            "{requested_routing}, {}",
            describe_applied_channel_map(unit.raw())
        );

        let workgroup = audio_workgroup(unit.raw())?;
        let (monitor, tee) = monitor::monitor_pair(monitor_state);
        let pump = Pump::new(
            shared.pump_links(engine, input_consumer, output_producer, tee, levels),
            None,
        );
        let worker = spawn_pump(pump, &shared, workgroup)?;

        if let Err(error) = start_output_unit(unit.raw(), "AudioOutputUnitStart") {
            stop_worker(&shared, worker);
            return Err(error);
        }
        Ok(Self {
            transport: Transport::Aggregate {
                unit: unit.into_raw() as usize,
                context: context.into_raw() as usize,
            },
            shared,
            worker: Some(worker),
            running: true,
            monitor,
            routing,
        })
    }

    /// See docs/adr/0003-coreaudio-transport.md#split-transport-contract.
    ///
    /// # Errors
    ///
    /// Returns [`CoreAudioError`] when `capture_rate` lies outside the
    /// resampler's range, or when AUHAL setup or worker startup fails —
    /// including [`CoreAudioError::OutputRouting`] when `output_device`
    /// reports no output channels (the render format is sized from that
    /// count, see [`crate::routing::split_render_channels`]). Every error
    /// path releases both AUHAL instances, the callback contexts, and the
    /// worker.
    pub fn start_native(
        input_device: u32,
        output_device: u32,
        capture_rate: u32,
        engine: SwitchingEngine,
        levels: Arc<StreamLevels>,
        monitor_state: Arc<AtomicI32>,
    ) -> Result<Self, CoreAudioError> {
        split::start(
            input_device,
            output_device,
            capture_rate,
            engine,
            levels,
            monitor_state,
        )
    }

    /// Stops callbacks, leaves the audio workgroup, and disposes the
    /// AUHAL instance(s).
    pub fn stop(&mut self) {
        if !self.running {
            return;
        }
        self.monitor.disable();
        self.transport.stop_units();
        self.shared.request_shutdown();
        if let Some(worker) = self.worker.take() {
            let _ignored = worker.join();
        }
        unsafe {
            self.transport.dispose();
        }
        self.running = false;
    }

    /// Whether AUHAL has started and has not been stopped.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.running
    }

    /// Whether a callback or inference call has failed or the worker has exited.
    #[must_use]
    pub fn is_faulted(&self) -> bool {
        self.shared.faulted.load(Ordering::Acquire)
            || self.worker.as_ref().is_some_and(JoinHandle::is_finished)
    }

    /// See docs/adr/0003-coreaudio-transport.md#heartbeat-frame-counter.
    #[must_use]
    pub fn frames_processed(&self) -> u64 {
        self.shared.frames.load(Ordering::Relaxed)
    }

    /// See docs/adr/0002-engine-diagnostics.md#diagnostic-counters.
    #[must_use]
    pub fn output_underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }

    /// See docs/adr/0002-engine-diagnostics.md#diagnostic-counters.
    #[must_use]
    pub fn input_overruns(&self) -> u64 {
        self.shared.input_overruns.load(Ordering::Relaxed)
    }

    /// Diagnostic: engine blocks the inference worker has processed
    /// since start (or the last [`Runtime::reset_debug_stats`]).
    #[must_use]
    pub fn worker_blocks(&self) -> u64 {
        self.shared.block_stats.blocks()
    }

    /// Diagnostic: worker blocks that exceeded the 10 ms budget
    /// ([`crate::observe::BLOCK_BUDGET_NS`]) since start (or the last
    /// [`Runtime::reset_debug_stats`]).
    #[must_use]
    pub fn worker_blocks_over_budget(&self) -> u64 {
        self.shared.block_stats.over_budget()
    }

    /// Diagnostic: the longest single worker block since start (or the
    /// last [`Runtime::reset_debug_stats`]), in nanoseconds.
    #[must_use]
    pub fn worker_block_max_ns(&self) -> u64 {
        self.shared.block_stats.max_ns()
    }

    /// See docs/adr/0001-realtime-audio-rules.md#worker-realtime-promotion.
    #[must_use]
    pub fn worker_realtime(&self) -> bool {
        self.shared.worker_realtime.load(Ordering::Acquire)
    }

    /// See docs/adr/0002-engine-diagnostics.md#diagnostic-counters.
    pub fn reset_debug_stats(&self) {
        self.shared.underruns.store(0, Ordering::Relaxed);
        self.shared.input_overruns.store(0, Ordering::Relaxed);
        self.shared.block_stats.reset();
    }

    /// See docs/adr/0005-preview-monitor.md#preview-monitor.
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

    /// See docs/adr/0005-preview-monitor.md#preview-monitor.
    #[must_use]
    pub fn monitor_device(&self) -> Option<u32> {
        self.monitor.active_device()
    }

    /// See docs/adr/0003-coreaudio-transport.md#auhal-channel-map.
    #[must_use]
    pub fn routing_description(&self) -> &str {
        &self.routing
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

fn configure_auhal(
    unit: AudioUnit,
    device: AudioDeviceId,
    virtual_output: VirtualOutputChannels,
) -> Result<String, CoreAudioError> {
    set_io_enabled(unit, Direction::Capture, true, "enable AUHAL input")?;
    set_io_enabled(unit, Direction::Render, true, "enable AUHAL output")?;
    set_current_device(unit, device, "select Aggregate Device")?;
    set_client_format(
        unit,
        Direction::Capture,
        &pcm_format(1),
        "set AUHAL capture format",
    )?;
    set_client_format(
        unit,
        Direction::Render,
        &pcm_format(virtual_output.count()),
        "set AUHAL render format",
    )?;
    set_max_frames_per_slice(unit, "set AUHAL frame bound")?;
    set_render_channel_map(unit, virtual_output)
}

fn set_max_frames_per_slice(
    unit: AudioUnit,
    operation: &'static str,
) -> Result<(), CoreAudioError> {
    let max_frames = u32::try_from(MAX_CALLBACK_FRAMES)
        .map_err(|error| CoreAudioError::Worker(format!("frame bound overflow: {error}")))?;
    set_property(
        unit,
        AUDIO_UNIT_PROPERTY_MAXIMUM_FRAMES_PER_SLICE,
        AUDIO_UNIT_SCOPE_GLOBAL,
        OUTPUT_BUS,
        &max_frames,
        operation,
    )
}

fn buffer_frames(unit: AudioUnit) -> Option<usize> {
    let mut frames = 0_u32;
    let mut size = size_u32::<u32>().ok()?;
    let status = unsafe {
        AudioUnitGetProperty(
            unit,
            AUDIO_DEVICE_PROPERTY_BUFFER_FRAME_SIZE,
            AUDIO_UNIT_SCOPE_GLOBAL,
            OUTPUT_BUS,
            (&raw mut frames).cast(),
            &raw mut size,
        )
    };
    (status == NO_ERR)
        .then(|| usize::try_from(frames).ok())
        .flatten()
}

#[derive(Debug, Clone, Copy)]
enum Direction {
    Capture,
    Render,
}

impl Direction {
    const fn element(self) -> u32 {
        match self {
            Self::Capture => INPUT_BUS,
            Self::Render => OUTPUT_BUS,
        }
    }

    const fn device_scope(self) -> u32 {
        match self {
            Self::Capture => AUDIO_UNIT_SCOPE_INPUT,
            Self::Render => AUDIO_UNIT_SCOPE_OUTPUT,
        }
    }

    const fn client_scope(self) -> u32 {
        match self {
            Self::Capture => AUDIO_UNIT_SCOPE_OUTPUT,
            Self::Render => AUDIO_UNIT_SCOPE_INPUT,
        }
    }
}

fn set_io_enabled(
    unit: AudioUnit,
    direction: Direction,
    enabled: bool,
    operation: &'static str,
) -> Result<(), CoreAudioError> {
    set_property(
        unit,
        AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
        direction.device_scope(),
        direction.element(),
        &u32::from(enabled),
        operation,
    )
}

fn set_current_device(
    unit: AudioUnit,
    device: AudioDeviceId,
    operation: &'static str,
) -> Result<(), CoreAudioError> {
    set_property(
        unit,
        AUDIO_OUTPUT_UNIT_PROPERTY_CURRENT_DEVICE,
        AUDIO_UNIT_SCOPE_GLOBAL,
        OUTPUT_BUS,
        &device,
        operation,
    )
}

fn set_client_format(
    unit: AudioUnit,
    direction: Direction,
    format: &AudioStreamBasicDescription,
    operation: &'static str,
) -> Result<(), CoreAudioError> {
    set_property(
        unit,
        AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
        direction.client_scope(),
        direction.element(),
        format,
        operation,
    )
}

fn set_render_channel_map(
    unit: AudioUnit,
    virtual_output: VirtualOutputChannels,
) -> Result<String, CoreAudioError> {
    let device_channels = device_output_channels(unit, "read aggregate device output format")?;
    let map = render_channel_map(device_channels, virtual_output)?;
    let byte_size = channel_map_bytes(map.len())?;
    check_status(
        unsafe {
            AudioUnitSetProperty(
                unit,
                AUDIO_OUTPUT_UNIT_PROPERTY_CHANNEL_MAP,
                AUDIO_UNIT_SCOPE_OUTPUT,
                OUTPUT_BUS,
                map.as_ptr().cast(),
                byte_size,
            )
        },
        "set AUHAL render channel map",
    )?;
    Ok(format!(
        "aggregate output channels {device_channels}, virtual output at channels {}..{}, \
         channel map requested {map:?}",
        virtual_output.first(),
        virtual_output.end()
    ))
}

fn describe_applied_channel_map(unit: AudioUnit) -> String {
    let Ok(device_channels) = device_output_channels(unit, "read aggregate device output format")
    else {
        return "channel map read back: device format unreadable".to_owned();
    };
    let Ok(len) = usize::try_from(device_channels) else {
        return "channel map read back: channel count overflow".to_owned();
    };
    let Ok(byte_size) = channel_map_bytes(len) else {
        return "channel map read back: size overflow".to_owned();
    };
    let mut applied = vec![0_i32; len];
    let mut applied_size = byte_size;
    let status = unsafe {
        AudioUnitGetProperty(
            unit,
            AUDIO_OUTPUT_UNIT_PROPERTY_CHANNEL_MAP,
            AUDIO_UNIT_SCOPE_OUTPUT,
            OUTPUT_BUS,
            applied.as_mut_ptr().cast(),
            &raw mut applied_size,
        )
    };
    if status != NO_ERR {
        return format!("channel map read back after initialize: unreadable (OSStatus {status})");
    }
    let applied_len = usize::try_from(applied_size)
        .unwrap_or(0)
        .saturating_div(size_of::<i32>())
        .min(applied.len());
    format!(
        "channel map read back after initialize {:?} (device output channels then \
         {device_channels})",
        &applied[..applied_len]
    )
}

fn channel_map_bytes(entries: usize) -> Result<u32, CoreAudioError> {
    entries
        .checked_mul(size_of::<i32>())
        .and_then(|bytes| u32::try_from(bytes).ok())
        .ok_or_else(|| CoreAudioError::OutputRouting("channel map size overflow".to_owned()))
}

fn device_output_channels(unit: AudioUnit, operation: &'static str) -> Result<u32, CoreAudioError> {
    stream_format_channels(unit, AUDIO_UNIT_SCOPE_OUTPUT, operation)
}

fn stream_format_channels(
    unit: AudioUnit,
    scope: u32,
    operation: &'static str,
) -> Result<u32, CoreAudioError> {
    let mut format = AudioStreamBasicDescription::EMPTY;
    let mut size = size_u32::<AudioStreamBasicDescription>()?;
    check_status(
        unsafe {
            AudioUnitGetProperty(
                unit,
                AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
                scope,
                OUTPUT_BUS,
                (&raw mut format).cast(),
                &raw mut size,
            )
        },
        operation,
    )?;
    Ok(format.channels_per_frame)
}

const fn pcm_format(channels: u32) -> AudioStreamBasicDescription {
    pcm_format_at(48_000.0, channels)
}

const fn pcm_format_at(sample_rate: f64, channels: u32) -> AudioStreamBasicDescription {
    AudioStreamBasicDescription {
        sample_rate,
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

const WORKER_WAIT_NS: i64 = 2_000_000;

#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

#[repr(C)]
struct ThreadTimeConstraintPolicy {
    period: u32,
    computation: u32,
    constraint: u32,
    preemptible: u32,
}

const THREAD_TIME_CONSTRAINT_POLICY: u32 = 2;
const THREAD_TIME_CONSTRAINT_POLICY_COUNT: u32 = 4;
const KERN_SUCCESS: i32 = 0;

#[link(name = "System")]
unsafe extern "C" {
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    fn pthread_self() -> *mut c_void;
    fn pthread_mach_thread_np(thread: *mut c_void) -> u32;
    fn thread_policy_set(
        thread: u32,
        flavor: u32,
        policy_info: *const ThreadTimeConstraintPolicy,
        count: u32,
    ) -> i32;
}

const WORKER_PERIOD_NS: u64 = 10_000_000;
const WORKER_COMPUTATION_NS: u64 = 5_000_000;
const WORKER_CONSTRAINT_NS: u64 = 10_000_000;

fn promote_current_thread_to_realtime() -> bool {
    let mut timebase = MachTimebaseInfo { numer: 0, denom: 0 };
    if unsafe { mach_timebase_info(&raw mut timebase) } != KERN_SUCCESS
        || timebase.numer == 0
        || timebase.denom == 0
    {
        return false;
    }
    let ticks = |ns: u64| -> u32 {
        let ticks = ns.saturating_mul(u64::from(timebase.denom)) / u64::from(timebase.numer);
        u32::try_from(ticks).unwrap_or(u32::MAX)
    };
    let policy = ThreadTimeConstraintPolicy {
        period: ticks(WORKER_PERIOD_NS),
        computation: ticks(WORKER_COMPUTATION_NS),
        constraint: ticks(WORKER_CONSTRAINT_NS),
        preemptible: 1,
    };
    let thread = unsafe { pthread_mach_thread_np(pthread_self()) };
    let status = unsafe {
        thread_policy_set(
            thread,
            THREAD_TIME_CONSTRAINT_POLICY,
            &raw const policy,
            THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        )
    };
    status == KERN_SUCCESS
}

struct WorkgroupGuard {
    workgroup: *mut c_void,
    token: WorkgroupJoinToken,
    joined: bool,
}

impl WorkgroupGuard {
    fn join(workgroup: usize) -> Self {
        let workgroup = workgroup as *mut c_void;
        let mut token = WorkgroupJoinToken::default();
        let joined = unsafe { os_workgroup_join(workgroup, &raw mut token) } == 0;
        Self {
            workgroup,
            token,
            joined,
        }
    }

    const fn joined(&self) -> bool {
        self.joined
    }
}

impl Drop for WorkgroupGuard {
    fn drop(&mut self) {
        if self.joined {
            unsafe {
                os_workgroup_leave(self.workgroup, &raw mut self.token);
            }
        }
    }
}

fn spawn_pump(
    mut pump: Pump,
    shared: &Shared,
    workgroup: usize,
) -> Result<JoinHandle<()>, CoreAudioError> {
    let shared = shared.clone();
    spawn_worker(move || {
        shared
            .worker_realtime
            .store(promote_current_thread_to_realtime(), Ordering::Release);
        let membership = WorkgroupGuard::join(workgroup);
        if !membership.joined() {
            shared.faulted.store(true, Ordering::Release);
        }
        while !shared.shutdown.load(Ordering::Acquire) {
            if !pump.step() {
                shared.samples_ready.wait_ns(WORKER_WAIT_NS);
            }
        }
        drop(membership);
    })
}

unsafe fn render_samples<'a>(
    data: *mut AudioBufferList,
    frame_count: u32,
) -> Option<(&'a mut [f32], usize)> {
    let buffer_list = unsafe { &mut *data };
    if buffer_list.number_buffers == 0 {
        return None;
    }
    let buffer = &mut buffer_list.buffers[0];
    if buffer.data.is_null() {
        return None;
    }
    let geometry = render_geometry(buffer.data_byte_size, buffer.number_channels, frame_count);
    let samples =
        unsafe { std::slice::from_raw_parts_mut(buffer.data.cast::<f32>(), geometry.samples()) };
    Some((samples, geometry.channels))
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
    let Some((samples, channels)) = (unsafe { render_samples(data, frame_count) }) else {
        context.capture.faulted.store(true, Ordering::Release);
        return PARAM_ERR;
    };
    if let Err(status) = unsafe {
        context
            .capture
            .capture(action_flags, timestamp, frame_count)
    } {
        samples.fill(0.0);
        return status;
    }
    context.output.render(samples, channels);
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
