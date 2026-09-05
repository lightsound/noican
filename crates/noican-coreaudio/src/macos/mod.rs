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

use crate::callback::{MAX_CALLBACK_FRAMES, capture_byte_size, render_geometry};
use crate::monitor::{MonitorTee, fourcc};
use crate::observe::{StreamLevels, WorkerBlockStats};
use crate::routing::{VirtualOutputChannels, render_channel_map};
use crate::{CoreAudioError, WORKER_BLOCK_SAMPLES};

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
    /// All-zero description, the landing value for property reads.
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

/// Owns a heap-allocated callback context during setup, reclaiming it
/// on any early error return (safe because the audio unit is never started
/// on those paths, so no callback can observe the pointer).
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

struct CallbackContext {
    unit: AudioUnit,
    input: Producer<f32>,
    /// Preallocated landing buffer for `AudioUnitRender` (mono,
    /// [`MAX_CALLBACK_FRAMES`]). The render buffer AUHAL hands over
    /// carries one channel per virtual-output channel, so it can no
    /// longer double as the mono capture target the way it did while the
    /// render stream was mono too.
    capture: Vec<f32>,
    output: Consumer<f32>,
    faulted: Arc<AtomicBool>,
    samples_ready: Arc<DispatchSemaphore>,
    /// Heartbeat: input frames delivered since start (relaxed atomic add is
    /// real-time safe). A counter that stops advancing while "running"
    /// means the device stopped calling back (unplugged microphone,
    /// coreaudiod restart, post-sleep stall).
    frames: Arc<AtomicU64>,
    /// Diagnostic: render callbacks that delivered no real audio at all
    /// because the output ring was completely dry (relaxed atomic add,
    /// same pattern as `frames`). Underrun means the inference worker
    /// fell behind the device clock — audible as dropouts in recordings
    /// from the virtual microphone while the monitor path masks it
    /// behind its re-priming cushion. Partial zero-fills are not
    /// counted (see the render callback for why they are benign).
    underruns: Arc<AtomicU64>,
    /// Whether any output sample has ever been popped. The output ring
    /// starts empty on this transport (the worker needs one full block
    /// of input before it produces anything), so the ramp-up callbacks
    /// that zero-fill before the first pop succeeds are start-up
    /// latency, not underrun — counting them would flag every model,
    /// including ones that never miss the budget. Callback-thread-only
    /// state, hence no atomic.
    output_primed: bool,
}

/// Everything the inference worker owns or shares. Bundled so the worker
/// spawn passes one value instead of a long argument list.
struct WorkerLinks {
    engine: SwitchingEngine,
    input: Consumer<f32>,
    output: Producer<f32>,
    tee: MonitorTee,
    levels: Arc<StreamLevels>,
    block_stats: Arc<WorkerBlockStats>,
    /// Set by the worker to whether its real-time promotion succeeded
    /// (see [`promote_current_thread_to_realtime`]).
    realtime: Arc<AtomicBool>,
}

/// The AUHAL instance(s) a running transport owns, with their callback
/// contexts. Raw pointers are stored as integers so the runtime stays
/// `Send`; they are only dereferenced on the control thread that owns
/// the runtime, after callbacks have been stopped.
#[derive(Debug)]
enum Transport {
    /// One AUHAL on the private Aggregate Device (48 kHz microphones —
    /// the original, unchanged path).
    Aggregate {
        /// The AUHAL instance.
        unit: usize,
        /// Its [`CallbackContext`].
        context: usize,
    },
    /// Two AUHALs for non-48 kHz microphones (issue #7): an input-only
    /// unit capturing the microphone at its native rate and an
    /// output-only unit feeding the virtual output at 48 kHz, bridged by
    /// the drift-compensating worker (see [`split`]).
    Split {
        /// Input-only AUHAL on the microphone device.
        capture_unit: usize,
        /// Its [`split::CaptureContext`].
        capture_context: usize,
        /// Output-only AUHAL on the virtual output device.
        output_unit: usize,
        /// Its [`split::OutputContext`].
        output_context: usize,
    },
}

impl Transport {
    /// Stops every audio unit (callbacks cease before the worker joins).
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

    /// Disposes the audio unit(s) and reclaims the callback context(s).
    ///
    /// # Safety
    ///
    /// Callbacks must no longer be running (units stopped, worker
    /// joined), and this must be called at most once.
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
                drop(Box::from_raw(
                    *capture_context as *mut split::CaptureContext,
                ));
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
    shutdown: Arc<AtomicBool>,
    faulted: Arc<AtomicBool>,
    samples_ready: Arc<DispatchSemaphore>,
    frames: Arc<AtomicU64>,
    underruns: Arc<AtomicU64>,
    block_stats: Arc<WorkerBlockStats>,
    /// Whether the inference worker's real-time promotion succeeded at
    /// loop start. False until the worker records it; afterwards it keeps
    /// that start-time result — it is not live scheduling-band membership
    /// (XNU may demote a thread that persistently overruns its declared
    /// computation), and a stopped-but-not-dropped runtime retains the
    /// last start's value.
    worker_realtime: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    running: bool,
    /// Control-plane half of the preview monitor (the worker half is the
    /// [`MonitorTee`] owned by the inference worker).
    monitor: MonitorControl,
    /// Diagnostic: how the render stream was routed into the device (the
    /// aggregate's reported output channel count, the channel map
    /// requested, and the map read back after `AudioUnitInitialize`). Empty on the
    /// split transport, which needs no map. See [`Runtime::routing_description`].
    routing: String,
}

impl Runtime {
    /// Opens an initialized AUHAL on a private Aggregate Device.
    ///
    /// `aggregate_device` must contain the selected physical input and the
    /// `BlackHole` output subdevice with drift compensation configured by the
    /// Swift control plane. `virtual_output` says where that subdevice's
    /// channels sit in the aggregate's output channel list (after any
    /// output channels of the microphone itself); the engine output is
    /// rendered as one client channel per virtual-output channel (dual
    /// mono) and routed there with an explicit one-to-one AUHAL channel
    /// map, see [`crate::routing`].
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
        let samples_ready = Arc::new(DispatchSemaphore::new()?);
        let mut unit = AuhalUnit::create()?;
        let requested_routing = configure_auhal(unit.raw(), aggregate_device, virtual_output)?;

        let (input_producer, input_consumer) = RingBuffer::new(RING_CAPACITY);
        let (output_producer, output_consumer) = RingBuffer::new(RING_CAPACITY);
        let faulted = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicU64::new(0));
        let underruns = Arc::new(AtomicU64::new(0));
        let block_stats = Arc::new(WorkerBlockStats::new());
        let worker_realtime = Arc::new(AtomicBool::new(false));
        let context = ContextGuard::new(CallbackContext {
            unit: unit.raw(),
            input: input_producer,
            capture: vec![0.0; MAX_CALLBACK_FRAMES],
            output: output_consumer,
            faulted: Arc::clone(&faulted),
            samples_ready: Arc::clone(&samples_ready),
            frames: Arc::clone(&frames),
            underruns: Arc::clone(&underruns),
            output_primed: false,
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
            block_stats: Arc::clone(&block_stats),
            realtime: Arc::clone(&worker_realtime),
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
            transport: Transport::Aggregate {
                unit: unit.into_raw() as usize,
                context: context.into_raw() as usize,
            },
            shutdown,
            faulted,
            samples_ready,
            frames,
            underruns,
            block_stats,
            worker_realtime,
            worker: Some(worker),
            running: true,
            monitor: monitor_control,
            routing,
        })
    }

    /// Opens the split transport for a microphone that cannot run at the
    /// 48 kHz engine rate (issue #7): an input-only AUHAL captures
    /// `input_device` at `capture_rate` Hz (its native rate — any rate
    /// from 8 to 192 kHz, see [`noican_core::capture`]), and an output-only
    /// AUHAL feeds `output_device` (the `BlackHole`/Noican virtual
    /// output) at 48 kHz. The inference worker bridges the two clock
    /// domains, converting to the engine rate with a drift-compensating
    /// resampler steered by ring occupancy
    /// ([`noican_core::capture`]).
    ///
    /// No Aggregate Device is involved: an aggregate drives all
    /// subdevices at one rate, so it cannot hold a 16 kHz microphone and
    /// the 48 kHz virtual output at the same time.
    ///
    /// `levels` and `monitor_state` behave exactly as in
    /// [`Runtime::start`].
    ///
    /// # Errors
    ///
    /// Returns [`CoreAudioError`] when `capture_rate` lies outside the
    /// resampler's range, or when AUHAL setup or worker startup fails.
    /// Every error path releases both AUHAL instances, the callback
    /// contexts, and the worker.
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
        self.shutdown.store(true, Ordering::Release);
        self.samples_ready.signal();
        if let Some(worker) = self.worker.take() {
            let _ignored = worker.join();
        }
        // Units are stopped and the worker joined, and `running` guards
        // against a second call.
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

    /// Diagnostic: output callbacks that were fully starved — zero
    /// real samples popped — after the ring first produced audio (the
    /// unprimed ramp-up on the aggregate path is start-up latency, not
    /// underrun, and partial shortfalls are benign block-quantization
    /// jitter). A count that keeps growing means the inference worker
    /// misses its 10 ms block budget — audible as dropouts in
    /// recordings from the virtual microphone. Resettable via
    /// [`Runtime::reset_debug_stats`] for per-model attribution.
    #[must_use]
    pub fn output_underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Diagnostic: engine blocks the inference worker has processed
    /// since start (or the last [`Runtime::reset_debug_stats`]).
    #[must_use]
    pub fn worker_blocks(&self) -> u64 {
        self.block_stats.blocks()
    }

    /// Diagnostic: worker blocks that exceeded the 10 ms budget
    /// ([`crate::observe::BLOCK_BUDGET_NS`]) since start (or the last
    /// [`Runtime::reset_debug_stats`]).
    #[must_use]
    pub fn worker_blocks_over_budget(&self) -> u64 {
        self.block_stats.over_budget()
    }

    /// Diagnostic: the longest single worker block since start (or the
    /// last [`Runtime::reset_debug_stats`]), in nanoseconds.
    #[must_use]
    pub fn worker_block_max_ns(&self) -> u64 {
        self.block_stats.max_ns()
    }

    /// Diagnostic: whether the inference worker's mach time-constraint
    /// (real-time) promotion succeeded at loop start (see
    /// [`promote_current_thread_to_realtime`]). False means the worker
    /// runs at default priority, so budget misses on hardware may be
    /// scheduling, not model cost. This reports the start-time result,
    /// not live membership: XNU may later demote a persistently
    /// overrunning thread without this flag changing.
    #[must_use]
    pub fn worker_realtime(&self) -> bool {
        self.worker_realtime.load(Ordering::Acquire)
    }

    /// Zeroes the diagnostic counters (underruns and worker block
    /// statistics) so a model switch can be measured in isolation.
    /// Plain relaxed stores — safe while the callbacks and the worker
    /// run.
    pub fn reset_debug_stats(&self) {
        self.underruns.store(0, Ordering::Relaxed);
        self.block_stats.reset();
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

    /// Diagnostic: how the render stream is routed into the device on the
    /// aggregate transport — the output channel count AUHAL reports for
    /// the aggregate, the channel map requested, and the map read back
    /// after `AudioUnitInitialize` (the point where AUHAL reconciles its
    /// configuration with the device; whether it may alter a map there is
    /// not documented, which is why the read-back is recorded rather
    /// than assumed). Empty on the split transport. Meant for the
    /// one-time start log on hardware, where the map's effect cannot
    /// otherwise be observed.
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

/// Configures the aggregate AUHAL: both directions enabled, a mono 48 kHz
/// capture client format, a 48 kHz render client format with one channel
/// per virtual-output channel (the callback duplicates the engine sample
/// into each), the frame bound that sizes the callback's capture landing
/// buffer, and the one-to-one output channel map that places the render
/// stream on `virtual_output`'s channels. Returns the routing
/// description for [`Runtime::routing_description`].
fn configure_auhal(
    unit: AudioUnit,
    device: AudioDeviceId,
    virtual_output: VirtualOutputChannels,
) -> Result<String, CoreAudioError> {
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
    let capture_format = pcm_format(1);
    set_property(
        unit,
        AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
        AUDIO_UNIT_SCOPE_OUTPUT,
        INPUT_BUS,
        &capture_format,
        "set AUHAL capture format",
    )?;
    // Dual mono: as many client channels as the virtual output has, so a
    // one-to-one channel map can feed every one of them (crate::routing).
    // A 1-channel virtual output yields the mono format of old.
    let render_format = pcm_format(virtual_output.count());
    set_property(
        unit,
        AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
        AUDIO_UNIT_SCOPE_INPUT,
        OUTPUT_BUS,
        &render_format,
        "set AUHAL render format",
    )?;
    set_max_frames_per_slice(unit, "set AUHAL frame bound")?;
    set_render_channel_map(unit, virtual_output)
}

/// Bounds the unit's callback size to [`MAX_CALLBACK_FRAMES`] so the
/// preallocated capture landing buffer always suffices (the callback
/// never allocates; see [`crate::callback`]).
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

/// Places the render stream (one client channel per virtual-output
/// channel) on the virtual output's channels inside the aggregate device
/// (see [`crate::routing`] for the bug and the decision).
///
/// The recipe follows Apple's AUHAL notes ("Channel Maps", reproduced
/// verbatim in `PortAudio`'s `src/hostapi/coreaudio/notes.txt`): for
/// output, the map is an `SInt32` array sized by the device's channel
/// count — "Get the Format of the AUHAL's output Element == 0" — with
/// every entry `-1` except `map[deviceOutputChannel] = clientChannel`,
/// each client channel named once.
/// That device-side format is read from `kAudioUnitScope_Output`,
/// element 0 (the only place it lives; TN2091 notes it is never
/// writable, so the client format set on the input scope cannot leak
/// into the read). The map is set on that same scope and element:
/// the property is documented for the input and output scopes, and
/// `PortAudio`'s Core Audio host API (`pa_mac_core.c`) — the shipping
/// implementation this was checked against — sets the output-element
/// map on the output scope, after the stream formats and before
/// `AudioUnitInitialize`, which is the order used here. AUHAL's default
/// map (identity: client `i` → device `i`) is what sent the engine
/// output into a headphone-equipped microphone's own outputs.
///
/// The capture direction keeps AUHAL's default map (client channel 0 ←
/// device input channel 0 = the microphone, which is the first
/// subdevice); it is deliberately not touched here.
fn set_render_channel_map(
    unit: AudioUnit,
    virtual_output: VirtualOutputChannels,
) -> Result<String, CoreAudioError> {
    let device_channels = device_output_channels(unit)?;
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

/// Reads the output channel map back for the start-time diagnostics.
/// Called after `AudioUnitInitialize`, the point where AUHAL reconciles
/// its configuration with the device: before it, a read would only echo
/// the array the setter just accepted. On hardware the map's effect is
/// otherwise invisible, so this — together with the requested map — is
/// the first thing to look at when the virtual microphone stays silent.
/// Never fails the start: an unreadable map is reported as such.
fn describe_applied_channel_map(unit: AudioUnit) -> String {
    let Ok(device_channels) = device_output_channels(unit) else {
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

/// Byte size of a channel map with `entries` `i32` slots.
fn channel_map_bytes(entries: usize) -> Result<u32, CoreAudioError> {
    entries
        .checked_mul(size_of::<i32>())
        .and_then(|bytes| u32::try_from(bytes).ok())
        .ok_or_else(|| CoreAudioError::OutputRouting("channel map size overflow".to_owned()))
}

/// Output channel count of the device behind the AUHAL, from the
/// hardware-side stream format of the output element (readable once the
/// current device is set, before initialization).
fn device_output_channels(unit: AudioUnit) -> Result<u32, CoreAudioError> {
    let mut format = AudioStreamBasicDescription::EMPTY;
    let mut size = size_u32::<AudioStreamBasicDescription>()?;
    check_status(
        unsafe {
            AudioUnitGetProperty(
                unit,
                AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
                AUDIO_UNIT_SCOPE_OUTPUT,
                OUTPUT_BUS,
                (&raw mut format).cast(),
                &raw mut size,
            )
        },
        "read aggregate device output format",
    )?;
    Ok(format.channels_per_frame)
}

/// Packed-float PCM at the 48 kHz engine rate with `channels` interleaved
/// channels (shared by the main capture/render and monitor formats).
const fn pcm_format(channels: u32) -> AudioStreamBasicDescription {
    pcm_format_at(48_000.0, channels)
}

/// Packed-float PCM at an arbitrary rate (the split transport captures
/// at the microphone's native rate).
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

#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

/// `thread_time_constraint_policy` (`mach/thread_policy.h`): all times
/// in mach absolute-time ticks; `preemptible` is a `boolean_t`.
#[repr(C)]
struct ThreadTimeConstraintPolicy {
    period: u32,
    computation: u32,
    constraint: u32,
    preemptible: u32,
}

const THREAD_TIME_CONSTRAINT_POLICY: u32 = 2;
/// `THREAD_TIME_CONSTRAINT_POLICY_COUNT`: four 32-bit fields.
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

/// The worker's declared real-time cadence: one engine block every 10 ms.
const WORKER_PERIOD_NS: u64 = 10_000_000;
/// Guaranteed computation per period, sized on the heaviest model's
/// typical block: p50 ≈ 4.2 ms on a modest 4-core x86-64 host
/// (`noican-models/examples/block_bench.rs`, 2026-09-02). This is the
/// CPU slice the scheduler reserves, not a ceiling — with `preemptible`
/// set the thread may keep running past it up to `constraint` — so p50
/// (plus margin) rather than that host's p95 of 6.2 ms is the basis:
/// tail blocks still complete within the period, Apple Silicon
/// performance cores are faster than the measuring host, and a larger
/// reservation would only raise the admission cost of the RT band.
/// Persistent overruns of the declared computation risk demotion by
/// XNU; what the 2026-09-04 on-device run supports is the absence of
/// steady-state underruns (block-time figures only reach the log
/// alongside underrun growth, so the device logged none for clean
/// models) — the sub-5 ms steady-state figure itself is the
/// `block_bench` measurement above.
const WORKER_COMPUTATION_NS: u64 = 5_000_000;
/// Deadline within each period: the whole period, matching the audio
/// cadence the output ring absorbs.
const WORKER_CONSTRAINT_NS: u64 = 10_000_000;

/// Promotes the calling thread to mach time-constraint ("real-time")
/// scheduling, the setup Apple's audio-workgroup guidance requires for
/// self-created inference threads *before* they join the device's
/// workgroup: joining alone conveys the deadline but does not lift the
/// thread out of its default priority, so the scheduler may still park it on
/// efficiency cores or preempt it for tens of milliseconds — observed on
/// hardware (2026-09-02) as chronic 10 ms-budget misses for
/// FastEnhancer-L (max 80.5 ms) and one-shot 40 ms stalls even on light
/// models, on a machine where the same models measure well inside the
/// budget when scheduled.
///
/// Returns whether the promotion succeeded; a failure is survivable
/// (audio still flows at default priority) and is surfaced through
/// [`Runtime::worker_realtime`] so hardware runs can interpret their
/// underrun numbers.
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

/// Membership of the device's audio `os_workgroup` for the lifetime of a
/// worker loop; leaves the workgroup on drop. A failed join is reported
/// through [`WorkgroupGuard::joined`] so the loop can flag the fault.
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

/// Runs the engine on one block and fans the result out to the meters,
/// the preview tee, and the output ring — the per-block step shared by
/// the aggregate and split worker loops. An engine failure emits silence
/// and flags the fault; output-ring overrun drops samples rather than
/// blocking.
fn run_block(
    engine: &mut SwitchingEngine,
    input_block: &[f32],
    output_block: &mut [f32],
    levels: &StreamLevels,
    tee: &mut MonitorTee,
    output: &mut Producer<f32>,
    faulted: &AtomicBool,
) {
    if engine.process_block(input_block, output_block).is_err() {
        output_block.fill(0.0);
        faulted.store(true, Ordering::Release);
    }
    levels.update(input_block, output_block);
    // Preview branch: the tee only copies into its preallocated monitor
    // ring (skipped entirely while disarmed) and disarms itself on
    // sustained feedback; it never delays the main path below.
    let _teed = tee.feed(output_block);
    for sample in output_block.iter().copied() {
        let _ignored = output.push(sample);
    }
}

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
        block_stats,
        realtime,
    } = links;
    levels.reset();
    block_stats.reset();
    // Real-time promotion must precede the workgroup join (see
    // promote_current_thread_to_realtime); failure degrades, not faults.
    // The stored value is the promotion result at loop start; XNU may
    // later demote a persistently overrunning thread, which this flag
    // does not track.
    realtime.store(promote_current_thread_to_realtime(), Ordering::Release);
    let membership = WorkgroupGuard::join(workgroup);
    if !membership.joined() {
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
                    // Timing the block on the worker (not the callback):
                    // `Instant::now` is a non-blocking clock read, fine
                    // on the inference thread that runs whole models.
                    let started = std::time::Instant::now();
                    run_block(
                        &mut engine,
                        &input_block,
                        &mut output_block,
                        &levels,
                        &mut tee,
                        &mut output,
                        faulted,
                    );
                    block_stats.record(saturating_elapsed_ns(started));
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
    drop(membership);
    // Meters and stats read 0 whenever no worker is running.
    levels.reset();
    block_stats.reset();
}

/// Nanoseconds since `started`, saturated into a `u64` (585 years —
/// unreachable in practice, but the stats must never panic).
fn saturating_elapsed_ns(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Render callback of the aggregate AUHAL: captures the microphone frames
/// of this cycle into the preallocated mono landing buffer, pushes them to
/// the input ring, and fills the (one channel per virtual-output channel)
/// render buffer from the output ring, duplicating each engine sample
/// into every channel of its frame — the same dual-mono shape the split
/// transport's output callback and the preview monitor produce.
/// Real-time rules (docs/tech-research.md §9): no allocation, no locks;
/// a callback larger than the frame bound faults instead of allocating;
/// ring overrun drops samples; underrun renders silence.
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
    let frames = usize::try_from(frame_count).unwrap_or(0);
    let Some(capture_bytes) = capture_byte_size(frames, context.capture.len()) else {
        // Cannot happen while kAudioUnitProperty_MaximumFramesPerSlice
        // holds; never allocate on the audio thread to compensate.
        context.faulted.store(true, Ordering::Release);
        return PARAM_ERR;
    };
    let mut capture_list = AudioBufferList {
        number_buffers: 1,
        buffers: [AudioBuffer {
            number_channels: 1,
            data_byte_size: capture_bytes,
            data: context.capture.as_mut_ptr().cast(),
        }],
    };
    let status = unsafe {
        AudioUnitRender(
            context.unit,
            action_flags,
            timestamp,
            INPUT_BUS,
            frame_count,
            &raw mut capture_list,
        )
    };
    let geometry = render_geometry(buffer.data_byte_size, buffer.number_channels, frame_count);
    let samples =
        unsafe { std::slice::from_raw_parts_mut(buffer.data.cast::<f32>(), geometry.samples()) };
    if status != NO_ERR {
        samples.fill(0.0);
        context.faulted.store(true, Ordering::Release);
        return NO_ERR;
    }
    for sample in &context.capture[..frames] {
        let _ignored = context.input.push(*sample);
    }
    context
        .frames
        .fetch_add(u64::from(frame_count), Ordering::Relaxed);
    // Wake the inference worker (never blocks; see DispatchSemaphore).
    context.samples_ready.signal();
    let was_primed = context.output_primed;
    let mut popped_any = false;
    for frame in samples.chunks_exact_mut(geometry.channels) {
        let value = match context.output.pop() {
            Ok(value) => {
                popped_any = true;
                value
            }
            Err(_empty) => 0.0,
        };
        for channel in frame {
            *channel = value;
        }
    }
    if popped_any {
        context.output_primed = true;
    } else if was_primed {
        // Underrun diagnostic: counted only when a callback delivered no
        // real audio at all after the stream primed. Partial shortfalls
        // are excluded on purpose — the worker writes 480-sample blocks
        // into (typically) smaller I/O periods, so a few benign partial
        // zero-fills occur on every model while the ring's phase cushion
        // forms; a fully starved callback, by contrast, means the worker
        // fell a whole I/O period behind. Relaxed add — real-time safe,
        // same pattern as the frames heartbeat.
        context.underruns.fetch_add(1, Ordering::Relaxed);
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
