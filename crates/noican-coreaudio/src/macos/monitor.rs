//! Control-plane half of the preview monitor: resolving and vetting the
//! playback target, and owning the output-only monitor AUHAL's lifecycle.
//!
//! The policy (which targets are refused) and the worker half (the
//! [`MonitorTee`]) live in the platform-independent [`crate::monitor`]
//! module so they stay unit-tested on every CI target; this module only
//! contains what needs Core Audio.

use std::ffi::{c_char, c_void};
use std::mem::size_of;
use std::ptr;
use std::sync::{
    Arc,
    atomic::{AtomicI32, AtomicU64, Ordering},
};

use rtrb::{Consumer, RingBuffer};

use crate::CoreAudioError;
use crate::monitor::{
    MONITOR_PRIME_SAMPLES, MONITOR_RING_CAPACITY, MonitorState, MonitorTee, classify_monitor_flip,
    classify_monitor_target, fourcc,
};

use super::{
    AUDIO_OUTPUT_UNIT_PROPERTY_CURRENT_DEVICE, AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
    AUDIO_UNIT_PROPERTY_STREAM_FORMAT, AUDIO_UNIT_SCOPE_GLOBAL, AUDIO_UNIT_SCOPE_INPUT,
    AUDIO_UNIT_SCOPE_OUTPUT, AudioBufferList, AudioDeviceId, AudioUnit, AudioUnitRenderActionFlags,
    AuhalUnit, INPUT_BUS, NO_ERR, OSStatus, OUTPUT_BUS, PARAM_ERR, attach_render_callback,
    check_status, dispose_unit, pcm_format, set_property, size_u32, start_output_unit,
    stop_output_unit,
};

const AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
const AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = fourcc(*b"glob");
const AUDIO_OBJECT_PROPERTY_SCOPE_OUTPUT: u32 = fourcc(*b"outp");
const AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;
const AUDIO_HARDWARE_PROPERTY_DEFAULT_OUTPUT_DEVICE: u32 = fourcc(*b"dOut");
const AUDIO_DEVICE_PROPERTY_TRANSPORT_TYPE: u32 = fourcc(*b"tran");
const AUDIO_DEVICE_PROPERTY_DATA_SOURCE: u32 = fourcc(*b"ssrc");
const AUDIO_DEVICE_PROPERTY_DEVICE_UID: u32 = fourcc(*b"uid ");

const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

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
    /// Device the AUHAL was opened on (the vetted default output at
    /// enable time). Exposed to the control plane so it can watch *this*
    /// device for losing its safety — the default output may move on
    /// while the monitor stays here.
    device: AudioDeviceId,
    /// The device's output data source at enable time: the user's
    /// vetted choice, against which later reads are compared by the
    /// flip policy ([`classify_monitor_flip`] — the headphone jack
    /// flipping to the internal speakers must stop the preview, while
    /// deliberately chosen speakers keep playing).
    data_source: Option<u32>,
}

/// Control-plane owner of the preview monitor. Invariants held here:
/// exactly one of `consumer` (parked while off) and `handle` (lent to the
/// running monitor AUHAL) is populated, and a feedback trip disarms the
/// tee (the shared [`MonitorState`] cell reads `Tripped`) while leaving
/// the AUHAL up until the next toggle.
#[derive(Debug)]
pub(super) struct MonitorControl {
    /// The shared [`MonitorState`] cell: this control half moves it
    /// between `Off` and `Playing`; the worker's [`MonitorTee`] gates on
    /// it and moves it to `Tripped`. Also read lock-free by the FFI
    /// layer, which owns the `Arc` (`noican_engine_monitor_state`).
    state: Arc<AtomicI32>,
    /// Monitor session generation, bumped by every [`MonitorControl::enable`]:
    /// the worker's echo canceller ([`crate::aec::SelfMonitorAec`])
    /// resets when it observes a bump, because a toggle or a new output
    /// device invalidates the echo path it learned.
    generation: Arc<AtomicU64>,
    consumer: Option<Consumer<f32>>,
    handle: Option<MonitorHandle>,
}

/// Creates the two halves of the preview monitor around one preallocated
/// ring — the control half for [`super::Runtime`] and the worker half
/// for the inference thread — plus the session-generation cell the
/// worker's echo canceller watches. Resets any stale value left in
/// `state` to [`MonitorState::Off`].
pub(super) fn monitor_pair(state: Arc<AtomicI32>) -> (MonitorControl, MonitorTee, Arc<AtomicU64>) {
    state.store(MonitorState::Off.as_raw(), Ordering::Release);
    let generation = Arc::new(AtomicU64::new(0));
    let (producer, consumer) = RingBuffer::new(MONITOR_RING_CAPACITY);
    let tee = MonitorTee::new(producer, Arc::clone(&state));
    (
        MonitorControl {
            state,
            generation: Arc::clone(&generation),
            consumer: Some(consumer),
            handle: None,
        },
        tee,
        generation,
    )
}

impl MonitorControl {
    /// Starts (or re-arms) the monitor on the vetted system default
    /// output.
    ///
    /// # Errors
    ///
    /// Returns a refusal from [`classify_monitor_target`] for unsafe
    /// targets, and other [`CoreAudioError`] values when the monitor
    /// AUHAL cannot start; the ring consumer is reclaimed on every error
    /// path so a later enable can retry.
    pub(super) fn enable(&mut self) -> Result<(), CoreAudioError> {
        if self.handle.is_some() {
            // The monitor AUHAL is still up (a feedback trip only disarms
            // the tee); re-arm it — which also clears the trip, since
            // `Playing` overwrites `Tripped` in the one shared cell. The
            // generation bump lands before the state store, so a worker
            // that observes `Playing` also observes the new session and
            // resets its echo canceller.
            self.generation.fetch_add(1, Ordering::Release);
            self.state
                .store(MonitorState::Playing.as_raw(), Ordering::Release);
            return Ok(());
        }
        let device = monitor_target_device()?;
        let mut consumer = self.consumer.take().ok_or_else(|| {
            CoreAudioError::Monitor("the monitor ring consumer is unavailable".to_owned())
        })?;
        // Discard anything left over from a previous preview session so
        // re-enabling never replays stale audio.
        while consumer.pop().is_ok() {}
        match start_monitor_unit(device, consumer) {
            Ok((unit, context)) => {
                self.handle = Some(MonitorHandle {
                    unit: unit as usize,
                    context: context as usize,
                    device,
                    data_source: device_data_source(device),
                });
                // Generation before state, as in the re-arm branch: a
                // worker observing `Playing` must observe the new
                // session and reset its echo canceller.
                self.generation.fetch_add(1, Ordering::Release);
                self.state
                    .store(MonitorState::Playing.as_raw(), Ordering::Release);
                Ok(())
            }
            Err((error, consumer)) => {
                // The AUHAL never came up, so the cell already reads
                // `Off`; park the consumer for a retry.
                self.consumer = Some(consumer);
                Err(error)
            }
        }
    }

    /// Stops the monitor AUHAL (if up) and parks the ring consumer for
    /// the next enable. Idempotent.
    pub(super) fn disable(&mut self) {
        // Move the cell to `Off` first (also clearing a pending trip) so
        // the worker stops feeding the ring before the monitor AUHAL
        // goes away.
        self.state
            .store(MonitorState::Off.as_raw(), Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let unit = handle.unit as AudioUnit;
            stop_output_unit(unit);
            dispose_unit(unit);
            unsafe {
                // Callbacks have stopped; reclaim the ring consumer so a
                // later enable reuses the preallocated ring.
                let context = Box::from_raw(handle.context as *mut MonitorContext);
                self.consumer = Some(context.output);
            }
        }
    }

    /// Device the running monitor AUHAL plays on (resolved and vetted at
    /// enable time), or `None` while the monitor is down.
    pub(super) fn active_device(&self) -> Option<AudioDeviceId> {
        self.handle.as_ref().map(|handle| handle.device)
    }

    /// Re-vets the device the running monitor plays on and returns why
    /// it must no longer receive the preview, or `None` while it stays
    /// safe (or no monitor is up).
    ///
    /// Two policies apply: the enable-time classification
    /// ([`classify_monitor_target`] — belt and braces, its inputs are
    /// static device properties) and the flip policy
    /// ([`classify_monitor_flip`]) against the data source recorded at
    /// enable time, which is what catches the headphone jack flipping
    /// to the internal speakers without any notification. A vanished
    /// device reads as unclassifiable and stays quiet here; device loss
    /// is visible in the device list and is the caller's check.
    pub(super) fn unsafe_reason(&self) -> Option<CoreAudioError> {
        let handle = self.handle.as_ref()?;
        if let Err(error) = classify_device(handle.device) {
            return Some(error);
        }
        classify_monitor_flip(handle.data_source, device_data_source(handle.device)).err()
    }
}

/// Checks whether the current system default output may receive the
/// preview, without creating or changing any audio object.
///
/// A pure inspection (a few Core Audio property reads against the HAL's
/// in-process cache), cheap enough for the UI to call before every
/// enable and whenever the default output changes — so an unsafe target
/// can be surfaced *before* the user tries to preview into it.
///
/// # Errors
///
/// Returns exactly the refusals enabling would produce: the
/// [`classify_monitor_target`] matrix (loopback, aggregate) or
/// [`CoreAudioError::Monitor`] when no default output is configured.
pub fn check_monitor_target() -> Result<(), CoreAudioError> {
    monitor_target_device().map(|_device| ())
}

/// Resolves the system default output device and applies
/// [`classify_monitor_target`] to refuse loopbacks and aggregates.
fn monitor_target_device() -> Result<AudioDeviceId, CoreAudioError> {
    let device = default_output_device()?;
    classify_device(device)?;
    Ok(device)
}

/// Reads `device`'s transport type and UID, and applies the
/// [`classify_monitor_target`] policy to them.
fn classify_device(device: AudioDeviceId) -> Result<(), CoreAudioError> {
    let transport = device_u32_property(
        device,
        AUDIO_DEVICE_PROPERTY_TRANSPORT_TYPE,
        AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
    )
    .unwrap_or(0);
    classify_monitor_target(transport, &device_uid(device))
}

/// Reads `device`'s output data source (`'ispk'`, `'hdpn'`, ...), or
/// `None` when unreadable — the flip policy's inputs.
fn device_data_source(device: AudioDeviceId) -> Option<u32> {
    device_u32_property(
        device,
        AUDIO_DEVICE_PROPERTY_DATA_SOURCE,
        AUDIO_OBJECT_PROPERTY_SCOPE_OUTPUT,
    )
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
    let attach: Result<(), CoreAudioError> = (|| {
        attach_render_callback(
            unit.raw(),
            monitor_render_callback,
            context.cast(),
            "AudioUnitSetProperty(monitor render callback)",
        )?;
        unit.initialize()?;
        start_output_unit(unit.raw(), "AudioOutputUnitStart(monitor)")
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
    let format = pcm_format(2);
    set_property(
        unit,
        AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
        AUDIO_UNIT_SCOPE_INPUT,
        OUTPUT_BUS,
        &format,
        "set monitor render format",
    )
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
