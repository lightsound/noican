//! Thin safe wrappers over the Core Audio HAL surface the engine needs:
//! device enumeration, property access, private aggregate devices, and
//! IOProc lifecycle.

pub mod ffi;

use std::ffi::c_void;

use crate::engine::RtError;
use ffi::{
    AudioObjectID, AudioObjectPropertyAddress, CFTypeRef, OSStatus, fourcc,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal,
};

fn check(status: OSStatus, context: &str) -> Result<(), RtError> {
    if status == 0 {
        Ok(())
    } else {
        Err(RtError::CoreAudio {
            context: context.to_owned(),
            status,
        })
    }
}

const fn global_address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Owned Core Foundation reference released on drop.
struct CfRef(CFTypeRef);

impl CfRef {
    fn string(s: &str) -> Result<Self, RtError> {
        let c = std::ffi::CString::new(s)
            .map_err(|_| RtError::Config("string contains NUL".to_owned()))?;
        // SAFETY: valid NUL-terminated pointer; CF copies the bytes.
        let r = unsafe {
            ffi::CFStringCreateWithCString(
                std::ptr::null(),
                c.as_ptr().cast(),
                ffi::kCFStringEncodingUTF8,
            )
        };
        if r.is_null() {
            return Err(RtError::Config("CFString creation failed".to_owned()));
        }
        Ok(Self(r))
    }

    fn number_i32(v: i32) -> Result<Self, RtError> {
        // SAFETY: value pointer is valid for the duration of the call.
        let r = unsafe {
            ffi::CFNumberCreate(
                std::ptr::null(),
                ffi::kCFNumberSInt32Type,
                std::ptr::from_ref(&v).cast(),
            )
        };
        if r.is_null() {
            return Err(RtError::Config("CFNumber creation failed".to_owned()));
        }
        Ok(Self(r))
    }

    fn dictionary(pairs: &[(&Self, &Self)]) -> Result<Self, RtError> {
        let keys: Vec<CFTypeRef> = pairs.iter().map(|(k, _)| k.0).collect();
        let values: Vec<CFTypeRef> = pairs.iter().map(|(_, v)| v.0).collect();
        // SAFETY: keys/values arrays are valid for the call; CF retains the
        // entries, so our owned refs can be released independently.
        let r = unsafe {
            ffi::CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                isize::try_from(pairs.len()).unwrap_or(0),
                std::ptr::addr_of!(ffi::kCFTypeDictionaryKeyCallBacks).cast(),
                std::ptr::addr_of!(ffi::kCFTypeDictionaryValueCallBacks).cast(),
            )
        };
        if r.is_null() {
            return Err(RtError::Config("CFDictionary creation failed".to_owned()));
        }
        Ok(Self(r))
    }

    fn array(items: &[&Self]) -> Result<Self, RtError> {
        let values: Vec<CFTypeRef> = items.iter().map(|i| i.0).collect();
        // SAFETY: values array is valid for the call; CF retains entries.
        let r = unsafe {
            ffi::CFArrayCreate(
                std::ptr::null(),
                values.as_ptr(),
                isize::try_from(items.len()).unwrap_or(0),
                std::ptr::addr_of!(ffi::kCFTypeArrayCallBacks).cast(),
            )
        };
        if r.is_null() {
            return Err(RtError::Config("CFArray creation failed".to_owned()));
        }
        Ok(Self(r))
    }
}

impl Drop for CfRef {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we own exactly one retain on this reference.
            unsafe { ffi::CFRelease(self.0) };
        }
    }
}

fn cf_string_to_rust(cf: CFTypeRef) -> String {
    if cf.is_null() {
        return String::new();
    }
    let mut buf = vec![0_u8; 512];
    // SAFETY: buffer pointer/length are valid; CF writes a NUL-terminated
    // UTF-8 string on success.
    let ok = unsafe {
        ffi::CFStringGetCString(
            cf,
            buf.as_mut_ptr(),
            isize::try_from(buf.len()).unwrap_or(0),
            ffi::kCFStringEncodingUTF8,
        )
    };
    if ok == 0 {
        return String::new();
    }
    let len = buf.iter().position(|b| *b == 0).unwrap_or(0);
    buf.truncate(len);
    String::from_utf8(buf).unwrap_or_default()
}

/// Reads a fixed-size property.
fn get_property<T>(
    object: AudioObjectID,
    address: &AudioObjectPropertyAddress,
    context: &str,
) -> Result<T, RtError> {
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let mut size = u32::try_from(size_of::<T>())
        .map_err(|_| RtError::Config("property too large".to_owned()))?;
    // SAFETY: `value` provides `size` writable bytes.
    let status = unsafe {
        ffi::AudioObjectGetPropertyData(
            object,
            address,
            0,
            std::ptr::null(),
            &raw mut size,
            value.as_mut_ptr().cast(),
        )
    };
    check(status, context)?;
    // SAFETY: Core Audio filled the value on success.
    Ok(unsafe { value.assume_init() })
}

/// Reads a variable-length array property.
fn get_property_vec<T: Copy>(
    object: AudioObjectID,
    address: &AudioObjectPropertyAddress,
    context: &str,
) -> Result<Vec<T>, RtError> {
    let mut size: u32 = 0;
    // SAFETY: out-size pointer is valid.
    let status = unsafe {
        ffi::AudioObjectGetPropertyDataSize(object, address, 0, std::ptr::null(), &raw mut size)
    };
    check(status, context)?;
    let count = size as usize / size_of::<T>();
    let mut out: Vec<T> = Vec::with_capacity(count);
    // SAFETY: the buffer has capacity for `size` bytes.
    let status = unsafe {
        ffi::AudioObjectGetPropertyData(
            object,
            address,
            0,
            std::ptr::null(),
            &raw mut size,
            out.as_mut_ptr().cast(),
        )
    };
    check(status, context)?;
    // SAFETY: Core Audio wrote `size` bytes = `count` elements.
    unsafe { out.set_len(size as usize / size_of::<T>()) };
    Ok(out)
}

/// All HAL devices.
///
/// # Errors
///
/// Returns [`RtError::CoreAudio`] when the HAL query fails.
pub fn all_devices() -> Result<Vec<AudioObjectID>, RtError> {
    get_property_vec(
        ffi::kAudioObjectSystemObject,
        &global_address(ffi::kAudioHardwarePropertyDevices),
        "listing devices",
    )
}

/// The system default input device.
///
/// # Errors
///
/// Returns [`RtError::CoreAudio`] when the HAL query fails.
pub fn default_input_device() -> Result<AudioObjectID, RtError> {
    get_property(
        ffi::kAudioObjectSystemObject,
        &global_address(ffi::kAudioHardwarePropertyDefaultInputDevice),
        "default input device",
    )
}

/// Number of streams a device has in the given scope.
///
/// # Errors
///
/// Returns [`RtError::CoreAudio`] when the HAL query fails.
pub fn stream_count(device: AudioObjectID, scope: u32) -> Result<usize, RtError> {
    let address = AudioObjectPropertyAddress {
        mSelector: ffi::kAudioDevicePropertyStreams,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut size: u32 = 0;
    // SAFETY: out-size pointer is valid.
    let status = unsafe {
        ffi::AudioObjectGetPropertyDataSize(
            device,
            &raw const address,
            0,
            std::ptr::null(),
            &raw mut size,
        )
    };
    check(status, "stream count")?;
    Ok(size as usize / size_of::<AudioObjectID>())
}

/// Device display name.
///
/// # Errors
///
/// Returns [`RtError::CoreAudio`] when the HAL query fails.
pub fn device_name(device: AudioObjectID) -> Result<String, RtError> {
    let cf: CFTypeRef = get_property(
        device,
        &global_address(ffi::kAudioObjectPropertyName),
        "device name",
    )?;
    let name = cf_string_to_rust(cf);
    // SAFETY: the get-property call transferred one retain to us.
    unsafe { ffi::CFRelease(cf) };
    Ok(name)
}

/// Device UID (stable identifier).
///
/// # Errors
///
/// Returns [`RtError::CoreAudio`] when the HAL query fails.
pub fn device_uid(device: AudioObjectID) -> Result<String, RtError> {
    let cf: CFTypeRef = get_property(
        device,
        &global_address(ffi::kAudioDevicePropertyDeviceUID),
        "device uid",
    )?;
    let uid = cf_string_to_rust(cf);
    // SAFETY: the get-property call transferred one retain to us.
    unsafe { ffi::CFRelease(cf) };
    Ok(uid)
}

/// Sets the nominal sample rate of a device.
///
/// # Errors
///
/// Returns [`RtError::CoreAudio`] when the HAL rejects the value.
pub fn set_nominal_sample_rate(device: AudioObjectID, rate: f64) -> Result<(), RtError> {
    let address = global_address(ffi::kAudioDevicePropertyNominalSampleRate);
    // SAFETY: data pointer/size describe a valid f64.
    let status = unsafe {
        ffi::AudioObjectSetPropertyData(
            device,
            &raw const address,
            0,
            std::ptr::null(),
            u32::try_from(size_of::<f64>()).unwrap_or(8),
            std::ptr::from_ref(&rate).cast(),
        )
    };
    check(status, "setting sample rate")
}

/// Sets the I/O buffer size in frames.
///
/// # Errors
///
/// Returns [`RtError::CoreAudio`] when the HAL rejects the value.
pub fn set_buffer_frame_size(device: AudioObjectID, frames: u32) -> Result<(), RtError> {
    let address = global_address(ffi::kAudioDevicePropertyBufferFrameSize);
    // SAFETY: data pointer/size describe a valid u32.
    let status = unsafe {
        ffi::AudioObjectSetPropertyData(
            device,
            &raw const address,
            0,
            std::ptr::null(),
            u32::try_from(size_of::<u32>()).unwrap_or(4),
            std::ptr::from_ref(&frames).cast(),
        )
    };
    check(status, "setting buffer frame size")
}

/// A private aggregate device for drift-free duplex I/O.
///
/// Combines the physical input device and the virtual output device, with
/// drift compensation on the output sub-device and the input device as
/// clock reference (docs/tech-research.md §4.2).
#[derive(Debug)]
pub struct AggregateDevice {
    id: AudioObjectID,
}

impl AggregateDevice {
    /// Creates the aggregate. `input_uid` becomes the clock master;
    /// `output_uid` gets `kAudioSubDeviceDriftCompensationKey` enabled.
    ///
    /// # Errors
    ///
    /// Returns [`RtError::CoreAudio`] when the HAL rejects the description.
    pub fn create(input_uid: &str, output_uid: &str) -> Result<Self, RtError> {
        let one = CfRef::number_i32(1)?;

        let k_uid = CfRef::string("uid")?;
        let k_name = CfRef::string("name")?;
        let k_subdevices = CfRef::string("subdevices")?;
        let k_master = CfRef::string("master")?;
        let k_private = CfRef::string("private")?;
        let k_drift = CfRef::string("drift")?;

        let in_uid = CfRef::string(input_uid)?;
        let out_uid = CfRef::string(output_uid)?;
        let sub_in = CfRef::dictionary(&[(&k_uid, &in_uid)])?;
        let sub_out = CfRef::dictionary(&[(&k_uid, &out_uid), (&k_drift, &one)])?;
        let subdevices = CfRef::array(&[&sub_in, &sub_out])?;

        let agg_uid = CfRef::string(&format!("com.noican.aggregate.{}", std::process::id()))?;
        let agg_name = CfRef::string("noican private aggregate")?;
        let description = CfRef::dictionary(&[
            (&k_uid, &agg_uid),
            (&k_name, &agg_name),
            (&k_subdevices, &subdevices),
            (&k_master, &in_uid),
            (&k_private, &one),
        ])?;

        let mut id: AudioObjectID = 0;
        // SAFETY: description is a valid CFDictionary; out pointer is valid.
        let status = unsafe { ffi::AudioHardwareCreateAggregateDevice(description.0, &raw mut id) };
        check(status, "creating aggregate device")?;
        Ok(Self { id })
    }

    /// The aggregate's device id.
    #[must_use]
    pub const fn id(&self) -> AudioObjectID {
        self.id
    }
}

impl Drop for AggregateDevice {
    fn drop(&mut self) {
        // SAFETY: `id` came from AudioHardwareCreateAggregateDevice and is
        // destroyed exactly once.
        let _ = unsafe { ffi::AudioHardwareDestroyAggregateDevice(self.id) };
    }
}

/// An installed + started IOProc, stopped and destroyed on drop.
#[derive(Debug)]
pub struct IoProcHandle {
    device: AudioObjectID,
    proc_id: ffi::AudioDeviceIOProcID,
}

impl IoProcHandle {
    /// Installs `proc_` on `device` with `client_data` and starts it.
    ///
    /// # Errors
    ///
    /// Returns [`RtError::CoreAudio`] when installation or start fails.
    ///
    /// # Safety
    ///
    /// `client_data` must stay valid (and its invariants hold under
    /// concurrent access from the audio thread) until the returned handle
    /// is dropped.
    pub unsafe fn install_and_start(
        device: AudioObjectID,
        proc_: ffi::AudioDeviceIOProc,
        client_data: *mut c_void,
    ) -> Result<Self, RtError> {
        let mut proc_id: ffi::AudioDeviceIOProcID = std::ptr::null_mut();
        // SAFETY: caller guarantees client_data validity; out pointer valid.
        let status =
            unsafe { ffi::AudioDeviceCreateIOProcID(device, proc_, client_data, &raw mut proc_id) };
        check(status, "creating IOProc")?;
        // SAFETY: proc_id was just created on this device.
        let status = unsafe { ffi::AudioDeviceStart(device, proc_id) };
        if status != 0 {
            // SAFETY: destroying the proc we created.
            let _ = unsafe { ffi::AudioDeviceDestroyIOProcID(device, proc_id) };
            return Err(RtError::CoreAudio {
                context: "starting device".to_owned(),
                status,
            });
        }
        Ok(Self { device, proc_id })
    }
}

impl Drop for IoProcHandle {
    fn drop(&mut self) {
        // SAFETY: stopping/destroying the proc we created on this device.
        unsafe {
            let _ = ffi::AudioDeviceStop(self.device, self.proc_id);
            let _ = ffi::AudioDeviceDestroyIOProcID(self.device, self.proc_id);
        }
    }
}

/// Convenience: `'inpt'` scope constant re-export for the engine.
#[must_use]
pub const fn input_scope() -> u32 {
    ffi::kAudioObjectPropertyScopeInput
}

/// Convenience: `'outp'` scope constant re-export for the engine.
#[must_use]
pub const fn output_scope() -> u32 {
    ffi::kAudioObjectPropertyScopeOutput
}

/// Suppress an unused-constant warning for the `FourCC` helper used only
/// in constant definitions.
const _: u32 = fourcc(b"glob");
