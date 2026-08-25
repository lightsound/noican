//! Audited AUHAL FFI and real-time callbacks.

#![allow(
    unsafe_code,
    reason = "AUHAL and os_workgroup are C APIs; unsafe code is confined to this module and callbacks only touch preallocated buffers and lock-free rings"
)]

use std::{
    ffi::c_void,
    mem::size_of,
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};

use noican_engine::{SwitchingEngine, PIPELINE_FRAME_SAMPLES};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::CoreAudioError;

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
    component_type: u32,
    component_subtype: u32,
    component_manufacturer: u32,
    component_flags: u32,
    component_flags_mask: u32,
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

struct CallbackContext {
    unit: AudioUnit,
    input: Producer<f32>,
    output: Consumer<f32>,
    faulted: Arc<AtomicBool>,
}

/// Running AUHAL instance and inference worker.
pub struct Runtime {
    unit: usize,
    callback: usize,
    shutdown: Arc<AtomicBool>,
    faulted: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    running: bool,
}

impl Runtime {
    /// Open an initialized AUHAL on a private Aggregate Device.
    ///
    /// `aggregate_device` must contain the selected physical input and the
    /// BlackHole output subdevice with drift compensation configured by the
    /// Swift control plane.
    ///
    /// # Errors
    ///
    /// Returns [`CoreAudioError`] when AUHAL setup or worker startup fails.
    pub fn start(aggregate_device: u32, engine: SwitchingEngine) -> Result<Self, CoreAudioError> {
        let unit = create_auhal()?;
        if let Err(error) = configure_auhal(unit, aggregate_device) {
            unsafe {
                let _ignored = AudioComponentInstanceDispose(unit);
            }
            return Err(error);
        }

        let (input_producer, input_consumer) = RingBuffer::new(RING_CAPACITY);
        let (output_producer, output_consumer) = RingBuffer::new(RING_CAPACITY);
        let faulted = Arc::new(AtomicBool::new(false));
        let callback = Box::new(CallbackContext {
            unit,
            input: input_producer,
            output: output_consumer,
            faulted: Arc::clone(&faulted),
        });
        let callback = Box::into_raw(callback);
        let callback_property = AudioUnitRenderCallback {
            callback: Some(render_callback),
            context: callback.cast(),
        };
        let callback_size = size_u32::<AudioUnitRenderCallback>()?;
        check_status(
            unsafe {
                AudioUnitSetProperty(
                    unit,
                    AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK,
                    AUDIO_UNIT_SCOPE_INPUT,
                    OUTPUT_BUS,
                    (&raw const callback_property).cast(),
                    callback_size,
                )
            },
            "AudioUnitSetProperty(render callback)",
        )?;
        check_status(unsafe { AudioUnitInitialize(unit) }, "AudioUnitInitialize")?;

        let workgroup = audio_workgroup(unit)?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_fault = Arc::clone(&faulted);
        let worker = thread::Builder::new()
            .name("noican-inference".to_owned())
            .spawn(move || {
                processing_loop(
                    engine,
                    input_consumer,
                    output_producer,
                    worker_shutdown,
                    worker_fault,
                    workgroup,
                );
            })
            .map_err(|error| CoreAudioError::Worker(error.to_string()))?;

        if let Err(error) = check_status(
            unsafe { AudioOutputUnitStart(unit) },
            "AudioOutputUnitStart",
        ) {
            shutdown.store(true, Ordering::Release);
            let _ignored = worker.join();
            unsafe {
                let _ignored = AudioUnitUninitialize(unit);
                let _ignored = AudioComponentInstanceDispose(unit);
                drop(Box::from_raw(callback));
            }
            return Err(error);
        }
        Ok(Self {
            unit: unit as usize,
            callback: callback as usize,
            shutdown,
            faulted,
            worker: Some(worker),
            running: true,
        })
    }

    /// Stop callbacks, leave the audio workgroup, and dispose AUHAL.
    pub fn stop(&mut self) {
        if !self.running {
            return;
        }
        let unit = self.unit as AudioUnit;
        unsafe {
            let _ignored = AudioOutputUnitStop(unit);
        }
        self.shutdown.store(true, Ordering::Release);
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
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop();
    }
}

fn create_auhal() -> Result<AudioUnit, CoreAudioError> {
    let description = AudioComponentDescription {
        component_type: AUDIO_UNIT_TYPE_OUTPUT,
        component_subtype: AUDIO_UNIT_SUBTYPE_HAL_OUTPUT,
        component_manufacturer: AUDIO_UNIT_MANUFACTURER_APPLE,
        component_flags: 0,
        component_flags_mask: 0,
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
                (value as *const T).cast(),
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

fn processing_loop(
    mut engine: SwitchingEngine,
    mut input: Consumer<f32>,
    mut output: Producer<f32>,
    shutdown: Arc<AtomicBool>,
    faulted: Arc<AtomicBool>,
    workgroup: usize,
) {
    let workgroup = workgroup as *mut c_void;
    let mut token = WorkgroupJoinToken::default();
    let joined = unsafe { os_workgroup_join(workgroup, &raw mut token) } == 0;
    if !joined {
        faulted.store(true, Ordering::Release);
    }
    let mut input_frame = [0.0_f32; PIPELINE_FRAME_SAMPLES];
    let mut output_frame = [0.0_f32; PIPELINE_FRAME_SAMPLES];
    let mut position = 0;
    while !shutdown.load(Ordering::Acquire) {
        match input.pop() {
            Ok(sample) => {
                input_frame[position] = sample;
                position += 1;
                if position == PIPELINE_FRAME_SAMPLES {
                    if engine
                        .process_frame(&input_frame, &mut output_frame)
                        .is_err()
                    {
                        output_frame.fill(0.0);
                        faulted.store(true, Ordering::Release);
                    }
                    for sample in output_frame {
                        let _ignored = output.push(sample);
                    }
                    position = 0;
                }
            }
            Err(_empty) => thread::yield_now(),
        }
    }
    if joined {
        unsafe {
            os_workgroup_leave(workgroup, &raw mut token);
        }
    }
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
    for sample in samples {
        *sample = context.output.pop().unwrap_or(0.0);
    }
    NO_ERR
}

fn check_status(status: OSStatus, operation: &'static str) -> Result<(), CoreAudioError> {
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
