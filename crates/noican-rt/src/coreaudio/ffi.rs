//! Hand-written FFI declarations for the small Core Audio / Core
//! Foundation surface this crate needs.
//!
//! Written by hand (instead of `coreaudio-sys`/bindgen) so the crate
//! type-checks on any host with `cargo check --target aarch64-apple-darwin`
//! — no macOS SDK headers required at check time. Layouts and constants
//! follow `CoreAudio/AudioHardware.h` and `CoreAudioTypes.h` (stable ABI
//! since Mac OS X 10.6).

#![allow(
    non_snake_case,
    non_upper_case_globals,
    reason = "names mirror the Core Audio C API"
)]
#![allow(
    missing_debug_implementations,
    reason = "raw FFI mirror types; Debug is meaningless for opaque handles"
)]

use std::ffi::c_void;

pub type OSStatus = i32;
pub type AudioObjectID = u32;
pub type AudioDeviceID = AudioObjectID;
pub type AudioDeviceIOProcID = *mut c_void;
pub type Boolean = u8;

/// Builds a `FourCC` constant (`'dev#'` style) as used by Core Audio.
#[must_use]
pub const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

pub const kAudioObjectSystemObject: AudioObjectID = 1;

pub const kAudioObjectPropertyScopeGlobal: u32 = fourcc(b"glob");
pub const kAudioObjectPropertyScopeInput: u32 = fourcc(b"inpt");
pub const kAudioObjectPropertyScopeOutput: u32 = fourcc(b"outp");
pub const kAudioObjectPropertyElementMain: u32 = 0;

pub const kAudioHardwarePropertyDevices: u32 = fourcc(b"dev#");
pub const kAudioHardwarePropertyDefaultInputDevice: u32 = fourcc(b"dIn ");
pub const kAudioObjectPropertyName: u32 = fourcc(b"lnam");
pub const kAudioDevicePropertyDeviceUID: u32 = fourcc(b"uid ");
pub const kAudioDevicePropertyStreams: u32 = fourcc(b"stm#");
pub const kAudioDevicePropertyStreamConfiguration: u32 = fourcc(b"slay");
pub const kAudioDevicePropertyNominalSampleRate: u32 = fourcc(b"nsrt");
pub const kAudioDevicePropertyBufferFrameSize: u32 = fourcc(b"fsiz");

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AudioObjectPropertyAddress {
    pub mSelector: u32,
    pub mScope: u32,
    pub mElement: u32,
}

#[repr(C)]
pub struct AudioBuffer {
    pub mNumberChannels: u32,
    pub mDataByteSize: u32,
    pub mData: *mut c_void,
}

/// Variable-length in C; only ever accessed through raw-pointer arithmetic.
#[repr(C)]
pub struct AudioBufferList {
    pub mNumberBuffers: u32,
    pub mBuffers: [AudioBuffer; 1],
}

/// Opaque, ABI-size-correct mirror of `AudioTimeStamp` (64 bytes). The
/// engine never reads its fields.
#[repr(C)]
pub struct AudioTimeStamp {
    _opaque: [u8; 64],
}

pub type AudioDeviceIOProc = unsafe extern "C-unwind" fn(
    device: AudioObjectID,
    now: *const AudioTimeStamp,
    input_data: *const AudioBufferList,
    input_time: *const AudioTimeStamp,
    output_data: *mut AudioBufferList,
    output_time: *const AudioTimeStamp,
    client_data: *mut c_void,
) -> OSStatus;

// Core Foundation minimal surface.
pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFArrayRef = *const c_void;
pub type CFNumberRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub type CFIndex = isize;

pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;
/// `kCFNumberSInt32Type`.
pub const kCFNumberSInt32Type: CFIndex = 3;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub static kCFTypeDictionaryKeyCallBacks: [u8; 0];
    pub static kCFTypeDictionaryValueCallBacks: [u8; 0];
    pub static kCFTypeArrayCallBacks: [u8; 0];

    pub fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const u8,
        encoding: u32,
    ) -> CFStringRef;
    pub fn CFStringGetCString(
        the_string: CFStringRef,
        buffer: *mut u8,
        buffer_size: CFIndex,
        encoding: u32,
    ) -> Boolean;
    pub fn CFDictionaryCreate(
        alloc: CFAllocatorRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        num_values: CFIndex,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
    pub fn CFArrayCreate(
        alloc: CFAllocatorRef,
        values: *const CFTypeRef,
        num_values: CFIndex,
        callbacks: *const c_void,
    ) -> CFArrayRef;
    pub fn CFNumberCreate(
        alloc: CFAllocatorRef,
        the_type: CFIndex,
        value_ptr: *const c_void,
    ) -> CFNumberRef;
    pub fn CFRelease(cf: CFTypeRef);
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    pub fn AudioObjectGetPropertyDataSize(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        qualifier_data_size: u32,
        qualifier_data: *const c_void,
        out_data_size: *mut u32,
    ) -> OSStatus;
    pub fn AudioObjectGetPropertyData(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        qualifier_data_size: u32,
        qualifier_data: *const c_void,
        io_data_size: *mut u32,
        out_data: *mut c_void,
    ) -> OSStatus;
    pub fn AudioObjectSetPropertyData(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        qualifier_data_size: u32,
        qualifier_data: *const c_void,
        data_size: u32,
        data: *const c_void,
    ) -> OSStatus;
    pub fn AudioHardwareCreateAggregateDevice(
        description: CFDictionaryRef,
        out_device: *mut AudioObjectID,
    ) -> OSStatus;
    pub fn AudioHardwareDestroyAggregateDevice(device: AudioObjectID) -> OSStatus;
    pub fn AudioDeviceCreateIOProcID(
        device: AudioObjectID,
        proc_: AudioDeviceIOProc,
        client_data: *mut c_void,
        out_proc_id: *mut AudioDeviceIOProcID,
    ) -> OSStatus;
    pub fn AudioDeviceDestroyIOProcID(
        device: AudioObjectID,
        proc_id: AudioDeviceIOProcID,
    ) -> OSStatus;
    pub fn AudioDeviceStart(device: AudioObjectID, proc_id: AudioDeviceIOProcID) -> OSStatus;
    pub fn AudioDeviceStop(device: AudioObjectID, proc_id: AudioDeviceIOProcID) -> OSStatus;
}
