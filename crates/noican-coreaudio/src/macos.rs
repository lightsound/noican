//! Audited AUHAL FFI and real-time callbacks.

#![expect(
    unsafe_code,
    reason = "AUHAL and os_workgroup are C APIs; unsafe code is confined to this module and callbacks only touch preallocated buffers and lock-free rings"
)]

use std::{
    ffi::{c_char, c_void},
    mem::size_of,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
};

use noican_core::SwitchingEngine;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::observe::{StreamLevels, tee_into_monitor};
use crate::{CoreAudioError, WORKER_BLOCK_SAMPLES};

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
/// Monitor ring capacity: 100 ms at 48 kHz. Deliberately small — the
/// monitor AUHAL runs on the default output device's own clock and drift
/// is not corrected, so a slow drain pins the ring at its capacity. A
/// small ring caps the preview latency at ~100 ms and turns the drift into
/// an occasional discarded block instead (accepted preview artifact).
const MONITOR_RING_CAPACITY: usize = 4_800;
/// Samples the monitor ring must hold before playback (re)starts after an
/// underrun: 40 ms at 48 kHz. Priming turns scattered single-sample
/// underruns into one bounded silence gap.
const MONITOR_PRIME_SAMPLES: usize = 1_920;

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

const AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
const AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = fourcc(*b"glob");
const AUDIO_OBJECT_PROPERTY_SCOPE_OUTPUT: u32 = fourcc(*b"outp");
const AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;
const AUDIO_HARDWARE_PROPERTY_DEFAULT_OUTPUT_DEVICE: u32 = fourcc(*b"dOut");
const AUDIO_DEVICE_PROPERTY_TRANSPORT_TYPE: u32 = fourcc(*b"tran");
const AUDIO_DEVICE_PROPERTY_DATA_SOURCE: u32 = fourcc(*b"ssrc");
const AUDIO_DEVICE_PROPERTY_DEVICE_UID: u32 = fourcc(*b"uid ");
const AUDIO_DEVICE_TRANSPORT_TYPE_VIRTUAL: u32 = fourcc(*b"virt");
const AUDIO_DEVICE_TRANSPORT_TYPE_BUILT_IN: u32 = fourcc(*b"bltn");
const DATA_SOURCE_INTERNAL_SPEAKER: u32 = fourcc(*b"ispk");

const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

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

#[repr(C)]
struct AudioObjectPropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const AudioObjectPropertyAddress,
        qualifier_data_size: u32,
        qualifier_data: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringGetCString(
        string: *const c_void,
        buffer: *mut c_char,
        buffer_size: isize,
        encoding: u32,
    ) -> u8;
    fn CFRelease(cf: *const c_void);
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

/// Render context of the output-only monitor AUHAL. Owned by the monitor
/// callback while preview runs; reclaimed (with its ring consumer) when
/// preview stops so a later enable can reuse the preallocated ring.
struct MonitorContext {
    output: Consumer<f32>,
    /// Whether enough samples are buffered to play. Cleared on underrun so
    /// playback resumes only after [`MONITOR_PRIME_SAMPLES`] accumulate,
    /// turning clock drift into bounded silence gaps instead of crackle.
    primed: bool,
}

/// Raw pointers of a running monitor AUHAL, stored as integers so the
/// runtime stays `Send`. They are only dereferenced on the control thread
/// that owns the runtime, after callbacks have been stopped.
#[derive(Debug)]
struct MonitorHandle {
    unit: usize,
    context: usize,
}

/// Everything the inference worker owns or shares. Bundled so the worker
/// spawn passes one value instead of a long argument list.
struct WorkerLinks {
    engine: SwitchingEngine,
    input: Consumer<f32>,
    output: Producer<f32>,
    monitor: Producer<f32>,
    monitor_enabled: Arc<AtomicBool>,
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
    /// Tee flag shared with the inference worker: when clear, the worker
    /// skips the monitor branch entirely.
    monitor_enabled: Arc<AtomicBool>,
    /// Consumer half of the preallocated monitor ring, parked here while
    /// preview is off and lent to the monitor callback while it is on.
    monitor_consumer: Option<Consumer<f32>>,
    monitor: Option<MonitorHandle>,
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
    /// resets it to silence on start and on exit.
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
        let callback_property = AudioUnitRenderCallback {
            callback: Some(render_callback),
            context: context.raw().cast(),
        };
        let callback_size = size_u32::<AudioUnitRenderCallback>()?;
        check_status(
            unsafe {
                AudioUnitSetProperty(
                    unit.raw(),
                    AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK,
                    AUDIO_UNIT_SCOPE_INPUT,
                    OUTPUT_BUS,
                    (&raw const callback_property).cast(),
                    callback_size,
                )
            },
            "AudioUnitSetProperty(render callback)",
        )?;
        unit.initialize()?;

        let workgroup = audio_workgroup(unit.raw())?;
        let (monitor_producer, monitor_consumer) = RingBuffer::new(MONITOR_RING_CAPACITY);
        let monitor_enabled = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_fault = Arc::clone(&faulted);
        let worker_semaphore = Arc::clone(&samples_ready);
        let links = WorkerLinks {
            engine,
            input: input_consumer,
            output: output_producer,
            monitor: monitor_producer,
            monitor_enabled: Arc::clone(&monitor_enabled),
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

        if let Err(error) = check_status(
            unsafe { AudioOutputUnitStart(unit.raw()) },
            "AudioOutputUnitStart",
        ) {
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
            monitor_enabled,
            monitor_consumer: Some(monitor_consumer),
            monitor: None,
        })
    }

    /// Stops callbacks, leaves the audio workgroup, and disposes AUHAL.
    pub fn stop(&mut self) {
        if !self.running {
            return;
        }
        self.disable_monitor();
        let unit = self.unit as AudioUnit;
        unsafe {
            let _ignored = AudioOutputUnitStop(unit);
        }
        self.shutdown.store(true, Ordering::Release);
        self.samples_ready.signal();
        if let Some(worker) = self.worker.take() {
            let _ignored = worker.join();
        }
        unsafe {
            let _ignored = AudioUnitUninitialize(unit);
            let _ignored = AudioComponentInstanceDispose(unit);
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
    /// directions are idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`CoreAudioError::NotRunning`] after [`Runtime::stop`],
    /// [`CoreAudioError::MonitorLoopbackOutput`] when the default output is
    /// a virtual loopback device (playing there would feed the processed
    /// voice into the meeting twice), and other [`CoreAudioError`] values
    /// when the monitor AUHAL cannot start.
    pub fn set_monitor(&mut self, enabled: bool) -> Result<(), CoreAudioError> {
        if enabled {
            self.enable_monitor()
        } else {
            self.disable_monitor();
            Ok(())
        }
    }

    /// Whether the preview monitor is currently playing.
    #[must_use]
    pub const fn is_monitoring(&self) -> bool {
        self.monitor.is_some()
    }

    fn enable_monitor(&mut self) -> Result<(), CoreAudioError> {
        if self.monitor.is_some() {
            return Ok(());
        }
        if !self.running {
            return Err(CoreAudioError::NotRunning);
        }
        let device = monitor_target_device()?;
        let mut consumer = self.monitor_consumer.take().ok_or_else(|| {
            CoreAudioError::Monitor("the monitor ring consumer is unavailable".to_owned())
        })?;
        // Discard anything left over from a previous preview session so
        // re-enabling never replays stale audio.
        while consumer.pop().is_ok() {}
        match start_monitor_unit(device, consumer) {
            Ok((unit, context)) => {
                self.monitor = Some(MonitorHandle {
                    unit: unit as usize,
                    context: context as usize,
                });
                self.monitor_enabled.store(true, Ordering::Release);
                Ok(())
            }
            Err((error, consumer)) => {
                self.monitor_consumer = Some(consumer);
                Err(error)
            }
        }
    }

    fn disable_monitor(&mut self) {
        // Clear the tee flag first so the worker stops feeding the ring
        // before the monitor AUHAL goes away.
        self.monitor_enabled.store(false, Ordering::Release);
        if let Some(handle) = self.monitor.take() {
            let unit = handle.unit as AudioUnit;
            unsafe {
                let _ignored = AudioOutputUnitStop(unit);
                let _ignored = AudioUnitUninitialize(unit);
                let _ignored = AudioComponentInstanceDispose(unit);
                // Callbacks have stopped; reclaim the ring consumer so a
                // later enable reuses the preallocated ring.
                let context = Box::from_raw(handle.context as *mut MonitorContext);
                self.monitor_consumer = Some(context.output);
            }
        }
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
    let format = AudioStreamBasicDescription {
        sample_rate: 48_000.0,
        format_id: AUDIO_FORMAT_LINEAR_PCM,
        format_flags: AUDIO_FORMAT_FLAG_IS_FLOAT | AUDIO_FORMAT_FLAG_IS_PACKED,
        bytes_per_packet: 4,
        frames_per_packet: 1,
        bytes_per_frame: 4,
        channels_per_frame: 1,
        bits_per_channel: 32,
        reserved: 0,
    };
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

/// Resolves the system default output device, rejecting targets that must
/// not receive the preview: virtual loopbacks (`BlackHole`/Noican — the
/// processed voice would reach the meeting a second time) and the
/// built-in speakers (the voice would feed straight back into the
/// microphone; Phase 0/1 has no echo cancellation).
fn monitor_target_device() -> Result<AudioDeviceId, CoreAudioError> {
    let device = default_output_device()?;
    let uid = device_uid(device);
    let transport = device_u32_property(
        device,
        AUDIO_DEVICE_PROPERTY_TRANSPORT_TYPE,
        AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
    )
    .unwrap_or(0);
    if transport == AUDIO_DEVICE_TRANSPORT_TYPE_VIRTUAL
        || uid.contains("BlackHole")
        || uid.to_lowercase().starts_with("com.lightsound.noican.")
    {
        return Err(CoreAudioError::MonitorLoopbackOutput { uid });
    }
    // Only the built-in output reports its speaker/headphone-jack state
    // through the data source; other transports (Bluetooth, USB, HDMI)
    // cannot be classified reliably and fail open — the feedback guard in
    // the worker is the safety net for those.
    if transport == AUDIO_DEVICE_TRANSPORT_TYPE_BUILT_IN
        && device_u32_property(
            device,
            AUDIO_DEVICE_PROPERTY_DATA_SOURCE,
            AUDIO_OBJECT_PROPERTY_SCOPE_OUTPUT,
        ) == Some(DATA_SOURCE_INTERNAL_SPEAKER)
    {
        return Err(CoreAudioError::MonitorSpeakerOutput);
    }
    Ok(device)
}

fn default_output_device() -> Result<AudioDeviceId, CoreAudioError> {
    let address = AudioObjectPropertyAddress {
        selector: AUDIO_HARDWARE_PROPERTY_DEFAULT_OUTPUT_DEVICE,
        scope: AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
        element: AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
    };
    let mut device: AudioDeviceId = 0;
    let mut size = size_u32::<AudioDeviceId>()?;
    check_status(
        unsafe {
            AudioObjectGetPropertyData(
                AUDIO_OBJECT_SYSTEM_OBJECT,
                &raw const address,
                0,
                ptr::null(),
                &raw mut size,
                (&raw mut device).cast(),
            )
        },
        "AudioObjectGetPropertyData(default output device)",
    )?;
    if device == 0 {
        return Err(CoreAudioError::Monitor(
            "no default output device is configured".to_owned(),
        ));
    }
    Ok(device)
}

/// One `u32` device property (transport type, data source, ...), or
/// `None` when unreadable.
fn device_u32_property(device: AudioDeviceId, selector: u32, scope: u32) -> Option<u32> {
    let address = AudioObjectPropertyAddress {
        selector,
        scope,
        element: AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
    };
    let mut value: u32 = 0;
    let mut size = size_u32::<u32>().ok()?;
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            &raw const address,
            0,
            ptr::null(),
            &raw mut size,
            (&raw mut value).cast(),
        )
    };
    (status == NO_ERR).then_some(value)
}

/// UID of a device, or an empty string when unreadable. Runs on the
/// control thread only (allocates; `CFString` round-trip).
fn device_uid(device: AudioDeviceId) -> String {
    let address = AudioObjectPropertyAddress {
        selector: AUDIO_DEVICE_PROPERTY_DEVICE_UID,
        scope: AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
        element: AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
    };
    let mut cf_string: *const c_void = ptr::null();
    let Ok(mut size) = size_u32::<*const c_void>() else {
        return String::new();
    };
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            &raw const address,
            0,
            ptr::null(),
            &raw mut size,
            (&raw mut cf_string).cast(),
        )
    };
    if status != NO_ERR || cf_string.is_null() {
        return String::new();
    }
    let mut buffer = [0_u8; 256];
    let buffer_len = isize::try_from(buffer.len()).unwrap_or(isize::MAX);
    let converted = unsafe {
        CFStringGetCString(
            cf_string,
            buffer.as_mut_ptr().cast::<c_char>(),
            buffer_len,
            CF_STRING_ENCODING_UTF8,
        )
    };
    unsafe {
        CFRelease(cf_string);
    }
    if converted == 0 {
        return String::new();
    }
    let length = buffer
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..length]).into_owned()
}

/// Creates, configures, and starts the output-only monitor AUHAL on
/// `device`, handing `consumer` to its render callback. On failure the
/// consumer is returned so the preallocated ring survives for a retry;
/// the partially configured unit is released by its RAII guard.
fn start_monitor_unit(
    device: AudioDeviceId,
    consumer: Consumer<f32>,
) -> Result<(AudioUnit, *mut MonitorContext), (CoreAudioError, Consumer<f32>)> {
    let mut unit = match AuhalUnit::create() {
        Ok(unit) => unit,
        Err(error) => return Err((error, consumer)),
    };
    if let Err(error) = configure_monitor_auhal(unit.raw(), device) {
        return Err((error, consumer));
    }
    let context = Box::into_raw(Box::new(MonitorContext {
        output: consumer,
        primed: false,
    }));
    let callback_property = AudioUnitRenderCallback {
        callback: Some(monitor_render_callback),
        context: context.cast(),
    };
    let attach: Result<(), CoreAudioError> = (|| {
        let callback_size = size_u32::<AudioUnitRenderCallback>()?;
        check_status(
            unsafe {
                AudioUnitSetProperty(
                    unit.raw(),
                    AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK,
                    AUDIO_UNIT_SCOPE_INPUT,
                    OUTPUT_BUS,
                    (&raw const callback_property).cast(),
                    callback_size,
                )
            },
            "AudioUnitSetProperty(monitor render callback)",
        )?;
        unit.initialize()?;
        check_status(
            unsafe { AudioOutputUnitStart(unit.raw()) },
            "AudioOutputUnitStart(monitor)",
        )
    })();
    match attach {
        Ok(()) => Ok((unit.into_raw(), context)),
        Err(error) => {
            // The unit never started, so no callback observed the context;
            // reclaim it to recover the ring consumer.
            let context = unsafe { Box::from_raw(context) };
            Err((error, context.output))
        }
    }
}

/// Output-only AUHAL on the monitor device: input disabled, mono 48 kHz
/// engine samples rendered as interleaved stereo (AUHAL's converter
/// handles the device's own rate and format).
fn configure_monitor_auhal(unit: AudioUnit, device: AudioDeviceId) -> Result<(), CoreAudioError> {
    let disabled = 0_u32;
    let enabled = 1_u32;
    set_property(
        unit,
        AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
        AUDIO_UNIT_SCOPE_INPUT,
        INPUT_BUS,
        &disabled,
        "disable monitor AUHAL input",
    )?;
    set_property(
        unit,
        AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
        AUDIO_UNIT_SCOPE_OUTPUT,
        OUTPUT_BUS,
        &enabled,
        "enable monitor AUHAL output",
    )?;
    set_property(
        unit,
        AUDIO_OUTPUT_UNIT_PROPERTY_CURRENT_DEVICE,
        AUDIO_UNIT_SCOPE_GLOBAL,
        OUTPUT_BUS,
        &device,
        "select monitor output device",
    )?;
    let format = AudioStreamBasicDescription {
        sample_rate: 48_000.0,
        format_id: AUDIO_FORMAT_LINEAR_PCM,
        format_flags: AUDIO_FORMAT_FLAG_IS_FLOAT | AUDIO_FORMAT_FLAG_IS_PACKED,
        bytes_per_packet: 8,
        frames_per_packet: 1,
        bytes_per_frame: 8,
        channels_per_frame: 2,
        bits_per_channel: 32,
        reserved: 0,
    };
    set_property(
        unit,
        AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
        AUDIO_UNIT_SCOPE_INPUT,
        OUTPUT_BUS,
        &format,
        "set monitor render format",
    )
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
        mut monitor,
        monitor_enabled,
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
                    // Preview branch first: it only copies into the
                    // preallocated monitor ring (skipped entirely while
                    // preview is off) and never delays the main path.
                    tee_into_monitor(&monitor_enabled, &mut monitor, &output_block);
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

/// Render callback of the output-only monitor AUHAL. Real-time rules
/// (docs/tech-research.md §9): it only moves `f32` samples from the
/// preallocated monitor ring into the device buffer — no allocation, no
/// locks, no syscalls. Ring underrun renders silence and re-arms priming;
/// it never blocks and never touches the meeting-facing path.
unsafe extern "C" fn monitor_render_callback(
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
    let context = unsafe { &mut *context.cast::<MonitorContext>() };
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
    if !context.primed && context.output.slots() >= MONITOR_PRIME_SAMPLES {
        context.primed = true;
    }
    for frame in samples.chunks_exact_mut(channels) {
        let value = if context.primed {
            match context.output.pop() {
                Ok(sample) => sample,
                Err(_underrun) => {
                    context.primed = false;
                    0.0
                }
            }
        } else {
            0.0
        };
        // The engine is mono; duplicate into every device channel.
        for channel in frame {
            *channel = value;
        }
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

const fn fourcc(bytes: [u8; 4]) -> u32 {
    u32::from_be_bytes(bytes)
}
