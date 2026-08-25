//! Hand-written declarations for the Core Audio HAL.
//!
//! Written out rather than generated because the surface we need is small,
//! stable, and entirely C: about a dozen functions, three plain-old-data
//! structs, and a pile of four-character-code constants. A generator would add
//! a build-time dependency on the macOS SDK and a layer of indirection for no
//! benefit at this size.
//!
//! Every constant below is the four-character code Apple's headers define, in
//! the byte order `AudioObjectPropertySelector` uses.

use core::ffi::c_void;

/// Result code returned by every Core Audio call. Zero means success.
pub type OsStatus = i32;

/// Identifier of an object in the HAL — a device, a stream, or the system.
pub type AudioObjectId = u32;

/// Token identifying a registered I/O callback.
pub type AudioDeviceIoProcId = *mut c_void;

/// Builds a four-character code the way Apple's headers do.
const fn fourcc(code: [u8; 4]) -> u32 {
    u32::from_be_bytes(code)
}

/// The system object, the root of every HAL query.
pub const SYSTEM_OBJECT: AudioObjectId = 1;

/// `kAudioObjectPropertyElementMain`.
pub const ELEMENT_MAIN: u32 = 0;

/// `kAudioObjectPropertyScopeGlobal`.
pub const SCOPE_GLOBAL: u32 = fourcc(*b"glob");
/// `kAudioObjectPropertyScopeInput`.
pub const SCOPE_INPUT: u32 = fourcc(*b"inpt");
/// `kAudioObjectPropertyScopeOutput`.
pub const SCOPE_OUTPUT: u32 = fourcc(*b"outp");

/// `kAudioHardwarePropertyDevices`.
pub const HARDWARE_DEVICES: u32 = fourcc(*b"dev#");
/// `kAudioHardwarePropertyDefaultInputDevice`.
pub const HARDWARE_DEFAULT_INPUT: u32 = fourcc(*b"dIn ");
/// `kAudioHardwarePropertyTranslateUIDToDevice`.
pub const HARDWARE_TRANSLATE_UID: u32 = fourcc(*b"uidd");

/// `kAudioDevicePropertyDeviceUID`.
pub const DEVICE_UID: u32 = fourcc(*b"uid ");
/// `kAudioObjectPropertyName`.
pub const OBJECT_NAME: u32 = fourcc(*b"lnam");
/// `kAudioDevicePropertyStreamConfiguration`.
pub const DEVICE_STREAM_CONFIGURATION: u32 = fourcc(*b"slay");
/// `kAudioDevicePropertyNominalSampleRate`.
pub const DEVICE_NOMINAL_SAMPLE_RATE: u32 = fourcc(*b"nsrt");
/// `kAudioDevicePropertyBufferFrameSize`.
pub const DEVICE_BUFFER_FRAME_SIZE: u32 = fourcc(*b"fsiz");
/// `kAudioDevicePropertyDeviceIsAlive`.
pub const DEVICE_IS_ALIVE: u32 = fourcc(*b"livn");
/// `kAudioDevicePropertyTransportType`.
pub const DEVICE_TRANSPORT_TYPE: u32 = fourcc(*b"tran");

/// `kAudioDeviceTransportTypeVirtual`.
pub const TRANSPORT_TYPE_VIRTUAL: u32 = fourcc(*b"virt");
/// `kAudioDeviceTransportTypeAggregate`.
pub const TRANSPORT_TYPE_AGGREGATE: u32 = fourcc(*b"grup");

/// Keys of the dictionary passed to `AudioHardwareCreateAggregateDevice`.
///
/// These are the string values behind Apple's `kAudioAggregateDevice*Key` and
/// `kAudioSubDevice*Key` macros.
pub mod aggregate_keys {
    /// `kAudioAggregateDeviceUIDKey`.
    pub const UID: &str = "uid";
    /// `kAudioAggregateDeviceNameKey`.
    pub const NAME: &str = "name";
    /// `kAudioAggregateDeviceSubDeviceListKey`.
    pub const SUB_DEVICE_LIST: &str = "subdevices";
    /// `kAudioAggregateDeviceMasterSubDeviceKey`, which names the clock source.
    pub const MAIN_SUB_DEVICE: &str = "master";
    /// `kAudioAggregateDeviceIsPrivateKey`: keeps the device out of the
    /// system's device list, so it never appears in Sound Settings.
    pub const IS_PRIVATE: &str = "private";
    /// `kAudioAggregateDeviceIsStackedKey`.
    pub const IS_STACKED: &str = "stacked";
    /// `kAudioSubDeviceUIDKey`.
    pub const SUB_DEVICE_UID: &str = "uid";
    /// `kAudioSubDeviceDriftCompensationKey`.
    pub const DRIFT_COMPENSATION: &str = "drift";
    /// `kAudioSubDeviceDriftCompensationQualityKey`.
    pub const DRIFT_COMPENSATION_QUALITY: &str = "drift quality";
}

/// `kAudioSubDeviceDriftCompensationMaxQuality`.
pub const DRIFT_COMPENSATION_MAX_QUALITY: i32 = 0x7F;

/// Which property of which object, in which scope.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioObjectPropertyAddress {
    /// The property, as a four-character code.
    pub selector: u32,
    /// Global, input, or output.
    pub scope: u32,
    /// Channel, or [`ELEMENT_MAIN`] for the device as a whole.
    pub element: u32,
}

impl AudioObjectPropertyAddress {
    /// A global-scope address for the whole object.
    #[must_use]
    pub const fn global(selector: u32) -> Self {
        Self {
            selector,
            scope: SCOPE_GLOBAL,
            element: ELEMENT_MAIN,
        }
    }

    /// An address scoped to a device's inputs or outputs.
    #[must_use]
    pub const fn scoped(selector: u32, scope: u32) -> Self {
        Self {
            selector,
            scope,
            element: ELEMENT_MAIN,
        }
    }
}

/// One buffer of audio handed to or expected from an I/O callback.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioBuffer {
    /// Channels interleaved in `data`.
    pub number_channels: u32,
    /// Size of `data`, in bytes.
    pub data_byte_size: u32,
    /// The samples, or null when the device provided none.
    pub data: *mut c_void,
}

/// A variable-length list of [`AudioBuffer`].
///
/// Declared with one trailing buffer, exactly as the C header does; the real
/// length is `number_buffers` and the rest follow contiguously in memory.
#[repr(C)]
#[derive(Debug)]
pub struct AudioBufferList {
    /// Number of buffers actually present.
    pub number_buffers: u32,
    /// The first buffer. Later ones follow it in memory.
    pub buffers: [AudioBuffer; 1],
}

impl AudioBufferList {
    /// The buffers this list actually holds.
    ///
    /// # Safety
    ///
    /// The caller guarantees that `self` points at a list Core Audio produced,
    /// so that `number_buffers` buffers really do follow the header.
    #[must_use]
    pub const unsafe fn as_slice(&self) -> &[AudioBuffer] {
        // SAFETY: delegated to the caller; Core Audio always allocates the
        // trailing buffers contiguously with the header.
        unsafe { core::slice::from_raw_parts(self.buffers.as_ptr(), self.number_buffers as usize) }
    }

    /// The buffers this list actually holds, mutably.
    ///
    /// # Safety
    ///
    /// As [`Self::as_slice`].
    #[must_use]
    pub const unsafe fn as_mut_slice(&mut self) -> &mut [AudioBuffer] {
        // SAFETY: delegated to the caller, as above.
        unsafe {
            core::slice::from_raw_parts_mut(self.buffers.as_mut_ptr(), self.number_buffers as usize)
        }
    }
}

/// The callback Core Audio invokes once per device buffer.
///
/// Time stamps are passed as opaque pointers: nothing here needs to read them,
/// and declaring `AudioTimeStamp` by hand would be one more struct layout to
/// get exactly right for no gain.
pub type AudioDeviceIoProc = unsafe extern "C" fn(
    device: AudioObjectId,
    now: *const c_void,
    input_data: *const AudioBufferList,
    input_time: *const c_void,
    output_data: *mut AudioBufferList,
    output_time: *const c_void,
    client_data: *mut c_void,
) -> OsStatus;

#[cfg(target_os = "macos")]
#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    /// Size, in bytes, of the value of a property.
    pub fn AudioObjectGetPropertyDataSize(
        object: AudioObjectId,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
    ) -> OsStatus;

    /// Reads the value of a property.
    pub fn AudioObjectGetPropertyData(
        object: AudioObjectId,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> OsStatus;

    /// Writes the value of a property.
    pub fn AudioObjectSetPropertyData(
        object: AudioObjectId,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: u32,
        data: *const c_void,
    ) -> OsStatus;

    /// Whether an object has a given property at all.
    pub fn AudioObjectHasProperty(
        object: AudioObjectId,
        address: *const AudioObjectPropertyAddress,
    ) -> bool;

    /// Creates an aggregate device from a description dictionary.
    pub fn AudioHardwareCreateAggregateDevice(
        description: *const c_void,
        device: *mut AudioObjectId,
    ) -> OsStatus;

    /// Destroys an aggregate device created by this process.
    pub fn AudioHardwareDestroyAggregateDevice(device: AudioObjectId) -> OsStatus;

    /// Registers an I/O callback on a device.
    pub fn AudioDeviceCreateIOProcID(
        device: AudioObjectId,
        proc_: AudioDeviceIoProc,
        client_data: *mut c_void,
        proc_id: *mut AudioDeviceIoProcId,
    ) -> OsStatus;

    /// Unregisters a callback registered by `AudioDeviceCreateIOProcID`.
    pub fn AudioDeviceDestroyIOProcID(
        device: AudioObjectId,
        proc_id: AudioDeviceIoProcId,
    ) -> OsStatus;

    /// Starts delivering buffers to a registered callback.
    pub fn AudioDeviceStart(device: AudioObjectId, proc_id: AudioDeviceIoProcId) -> OsStatus;

    /// Stops delivering buffers to a registered callback.
    pub fn AudioDeviceStop(device: AudioObjectId, proc_id: AudioDeviceIoProcId) -> OsStatus;
}

#[cfg(test)]
mod tests {
    use super::{HARDWARE_DEVICES, SCOPE_GLOBAL, fourcc};

    #[test]
    fn four_character_codes_match_apples_values() {
        // Spot-check against the numeric values in CoreAudio's headers.
        assert_eq!(SCOPE_GLOBAL, 0x676C_6F62);
        assert_eq!(HARDWARE_DEVICES, 0x6465_7623);
        assert_eq!(fourcc(*b"uid "), 0x7569_6420);
        assert_eq!(fourcc(*b"nsrt"), 0x6E73_7274);
    }
}
