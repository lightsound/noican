//! See docs/adr/0005-preview-monitor.md#monitor-module-split.

use std::ffi::{c_char, c_void};
use std::ptr;
use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

use rtrb::{Consumer, RingBuffer};

use crate::CoreAudioError;
use crate::callback::fill_dual_mono;
use crate::monitor::{
    MONITOR_PRIME_SAMPLES, MONITOR_RING_CAPACITY, MonitorState, MonitorTee,
    classify_monitor_target, fourcc,
};

use super::{
    AudioBufferList, AudioDeviceId, AudioUnit, AudioUnitRenderActionFlags, AuhalUnit, Direction,
    NO_ERR, OSStatus, PARAM_ERR, attach_render_callback, check_status, dispose_unit, pcm_format,
    render_samples, set_client_format, set_current_device, set_io_enabled, size_u32,
    start_output_unit, stop_output_unit,
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

struct MonitorContext {
    output: Consumer<f32>,
    primed: bool,
}

#[derive(Debug)]
struct MonitorHandle {
    unit: usize,
    context: usize,
    device: AudioDeviceId,
}

#[derive(Debug)]
pub(super) struct MonitorControl {
    state: Arc<AtomicI32>,
    consumer: Option<Consumer<f32>>,
    handle: Option<MonitorHandle>,
}

pub(super) fn monitor_pair(state: Arc<AtomicI32>) -> (MonitorControl, MonitorTee) {
    state.store(MonitorState::Off.as_raw(), Ordering::Release);
    let (producer, consumer) = RingBuffer::new(MONITOR_RING_CAPACITY);
    let tee = MonitorTee::new(producer, Arc::clone(&state));
    (
        MonitorControl {
            state,
            consumer: Some(consumer),
            handle: None,
        },
        tee,
    )
}

impl MonitorControl {
    pub(super) fn enable(&mut self) -> Result<(), CoreAudioError> {
        if self.handle.is_some() {
            self.state
                .store(MonitorState::Playing.as_raw(), Ordering::Release);
            return Ok(());
        }
        let device = monitor_target_device()?;
        let mut consumer = self.consumer.take().ok_or_else(|| {
            CoreAudioError::Monitor("the monitor ring consumer is unavailable".to_owned())
        })?;
        while consumer.pop().is_ok() {}
        match start_monitor_unit(device, consumer) {
            Ok((unit, context)) => {
                self.handle = Some(MonitorHandle {
                    unit: unit as usize,
                    context: context as usize,
                    device,
                });
                self.state
                    .store(MonitorState::Playing.as_raw(), Ordering::Release);
                Ok(())
            }
            Err((error, consumer)) => {
                self.consumer = Some(consumer);
                Err(error)
            }
        }
    }

    pub(super) fn disable(&mut self) {
        self.state
            .store(MonitorState::Off.as_raw(), Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let unit = handle.unit as AudioUnit;
            stop_output_unit(unit);
            dispose_unit(unit);
            unsafe {
                let context = Box::from_raw(handle.context as *mut MonitorContext);
                self.consumer = Some(context.output);
            }
        }
    }

    pub(super) fn active_device(&self) -> Option<AudioDeviceId> {
        self.handle.as_ref().map(|handle| handle.device)
    }
}

/// See docs/adr/0005-preview-monitor.md#monitor-target-vetting.
///
/// # Errors
///
/// Returns exactly the refusals enabling would produce: the
/// [`classify_monitor_target`] matrix (loopback, aggregate, built-in
/// speakers) or [`CoreAudioError::Monitor`] when no default output is
/// configured.
pub fn check_monitor_target() -> Result<(), CoreAudioError> {
    monitor_target_device().map(|_device| ())
}

/// See docs/adr/0005-preview-monitor.md#monitor-target-vetting.
///
/// # Errors
///
/// Returns the [`classify_monitor_target`] refusals (loopback, aggregate,
/// built-in speakers).
pub fn check_monitor_device(device: AudioDeviceId) -> Result<(), CoreAudioError> {
    classify_device(device)
}

fn monitor_target_device() -> Result<AudioDeviceId, CoreAudioError> {
    let device = default_output_device()?;
    classify_device(device)?;
    Ok(device)
}

fn classify_device(device: AudioDeviceId) -> Result<(), CoreAudioError> {
    let transport = device_u32_property(
        device,
        AUDIO_DEVICE_PROPERTY_TRANSPORT_TYPE,
        AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
    )
    .unwrap_or(0);
    let data_source = device_u32_property(
        device,
        AUDIO_DEVICE_PROPERTY_DATA_SOURCE,
        AUDIO_OBJECT_PROPERTY_SCOPE_OUTPUT,
    );
    classify_monitor_target(transport, &device_uid(device), data_source)
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
            let context = unsafe { Box::from_raw(context) };
            Err((error, context.output))
        }
    }
}

fn configure_monitor_auhal(unit: AudioUnit, device: AudioDeviceId) -> Result<(), CoreAudioError> {
    set_io_enabled(
        unit,
        Direction::Capture,
        false,
        "disable monitor AUHAL input",
    )?;
    set_io_enabled(unit, Direction::Render, true, "enable monitor AUHAL output")?;
    set_current_device(unit, device, "select monitor output device")?;
    set_client_format(
        unit,
        Direction::Render,
        &pcm_format(2),
        "set monitor render format",
    )
}

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
    let Some((samples, channels)) = (unsafe { render_samples(data, frame_count) }) else {
        return PARAM_ERR;
    };
    if !context.primed && context.output.slots() >= MONITOR_PRIME_SAMPLES {
        context.primed = true;
    }
    let MonitorContext { output, primed } = context;
    fill_dual_mono(samples, channels, || {
        if !*primed {
            return None;
        }
        match output.pop() {
            Ok(sample) => Some(sample),
            Err(_underrun) => {
                *primed = false;
                None
            }
        }
    });
    NO_ERR
}
