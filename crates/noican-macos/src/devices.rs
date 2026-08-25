//! Enumerating and describing audio devices.

/// What the UI needs to know about one audio device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// The HAL's identifier. Not stable across reboots or replugs.
    pub id: u32,
    /// The device's persistent unique identifier. Stable; use this to remember
    /// a user's choice.
    pub uid: String,
    /// The name shown in Sound Settings.
    pub name: String,
    /// Channels the device can capture.
    pub input_channels: u32,
    /// Channels the device can play.
    pub output_channels: u32,
    /// Current nominal sample rate, in hertz.
    pub sample_rate: u32,
    /// Whether the device is a virtual one, such as `BlackHole`.
    ///
    /// Reported by the HAL as its transport type, which is how the UI can
    /// suggest a sensible output device without matching on names.
    pub is_virtual: bool,
}

impl Device {
    /// Whether this device can be used as the microphone.
    #[must_use]
    pub const fn can_capture(&self) -> bool {
        self.input_channels > 0
    }

    /// Whether this device can be used as the virtual microphone's feed.
    #[must_use]
    pub const fn can_play(&self) -> bool {
        self.output_channels > 0
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use core::ffi::c_void;
    use core::mem;
    use core::ptr;

    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};

    use super::Device;
    use crate::error::{Error, Result, check};
    use crate::sys::{
        self, AudioObjectId, AudioObjectPropertyAddress, DEVICE_NOMINAL_SAMPLE_RATE,
        DEVICE_STREAM_CONFIGURATION, DEVICE_TRANSPORT_TYPE, DEVICE_UID, HARDWARE_DEFAULT_INPUT,
        HARDWARE_DEVICES, OBJECT_NAME, SCOPE_INPUT, SCOPE_OUTPUT, SYSTEM_OBJECT,
        TRANSPORT_TYPE_AGGREGATE, TRANSPORT_TYPE_VIRTUAL,
    };

    /// Reads a fixed-size property into a value of type `T`.
    fn property<T>(
        object: AudioObjectId,
        address: &AudioObjectPropertyAddress,
        operation: &'static str,
    ) -> Result<T> {
        let mut value = mem::MaybeUninit::<T>::uninit();
        let mut size = u32::try_from(mem::size_of::<T>()).unwrap_or(u32::MAX);
        // SAFETY: `value` has room for exactly `size` bytes, and Core Audio
        // writes at most that much because it is told the size.
        let status = unsafe {
            sys::AudioObjectGetPropertyData(
                object,
                address,
                0,
                ptr::null(),
                &raw mut size,
                value.as_mut_ptr().cast::<c_void>(),
            )
        };
        check(operation, status)?;
        // SAFETY: a successful call initialised the value.
        Ok(unsafe { value.assume_init() })
    }

    /// Reads a variable-size property into a byte buffer.
    fn property_bytes(
        object: AudioObjectId,
        address: &AudioObjectPropertyAddress,
        operation: &'static str,
    ) -> Result<Vec<u8>> {
        let mut size = 0u32;
        // SAFETY: querying the size writes only to `size`.
        let status = unsafe {
            sys::AudioObjectGetPropertyDataSize(object, address, 0, ptr::null(), &raw mut size)
        };
        check(operation, status)?;

        let mut buffer = vec![0u8; size as usize];
        // SAFETY: `buffer` holds exactly `size` bytes, which is what Core Audio
        // just said it needs.
        let status = unsafe {
            sys::AudioObjectGetPropertyData(
                object,
                address,
                0,
                ptr::null(),
                &raw mut size,
                buffer.as_mut_ptr().cast::<c_void>(),
            )
        };
        check(operation, status)?;
        buffer.truncate(size as usize);
        Ok(buffer)
    }

    /// Reads a `CFString` property and converts it to a Rust string.
    fn property_string(
        object: AudioObjectId,
        address: &AudioObjectPropertyAddress,
        operation: &'static str,
    ) -> Result<String> {
        let raw: CFStringRef = property(object, address, operation)?;
        if raw.is_null() {
            return Ok(String::new());
        }
        // SAFETY: Core Audio returns a +1 reference for these properties, so we
        // take ownership and let `CFString` release it.
        let string = unsafe { CFString::wrap_under_create_rule(raw) };
        Ok(string.to_string())
    }

    /// Total channels across every stream in `scope`.
    fn channel_count(object: AudioObjectId, scope: u32) -> Result<u32> {
        let address = AudioObjectPropertyAddress::scoped(DEVICE_STREAM_CONFIGURATION, scope);
        let buffer = property_bytes(object, &address, "reading the stream configuration")?;
        if buffer.len() < mem::size_of::<u32>() {
            return Ok(0);
        }
        // SAFETY: the buffer holds an `AudioBufferList` that Core Audio just
        // wrote, and it is at least as long as the header.
        let list = unsafe { &*buffer.as_ptr().cast::<sys::AudioBufferList>() };
        // SAFETY: as above; the trailing buffers are inside `buffer`.
        let buffers = unsafe { list.as_slice() };
        Ok(buffers.iter().map(|buffer| buffer.number_channels).sum())
    }

    /// Describes one device.
    fn describe(id: AudioObjectId) -> Result<Device> {
        let uid = property_string(
            id,
            &AudioObjectPropertyAddress::global(DEVICE_UID),
            "reading the device UID",
        )?;
        let name = property_string(
            id,
            &AudioObjectPropertyAddress::global(OBJECT_NAME),
            "reading the device name",
        )?;
        let sample_rate: f64 = property(
            id,
            &AudioObjectPropertyAddress::global(DEVICE_NOMINAL_SAMPLE_RATE),
            "reading the nominal sample rate",
        )
        .unwrap_or(0.0);
        let transport: u32 = property(
            id,
            &AudioObjectPropertyAddress::global(DEVICE_TRANSPORT_TYPE),
            "reading the transport type",
        )
        .unwrap_or(0);

        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a nominal sample rate is a small positive integer in practice"
        )]
        let sample_rate = sample_rate.round() as u32;

        Ok(Device {
            id,
            uid,
            name,
            input_channels: channel_count(id, SCOPE_INPUT).unwrap_or(0),
            output_channels: channel_count(id, SCOPE_OUTPUT).unwrap_or(0),
            sample_rate,
            is_virtual: transport == TRANSPORT_TYPE_VIRTUAL,
        })
    }

    /// Every device the HAL knows about.
    pub fn all() -> Result<Vec<Device>> {
        let address = AudioObjectPropertyAddress::global(HARDWARE_DEVICES);
        let buffer = property_bytes(SYSTEM_OBJECT, &address, "listing audio devices")?;
        let ids: Vec<AudioObjectId> = buffer
            .chunks_exact(mem::size_of::<AudioObjectId>())
            .map(|chunk| AudioObjectId::from_ne_bytes(chunk.try_into().unwrap_or([0; 4])))
            .collect();

        let mut devices = Vec::with_capacity(ids.len());
        for id in ids {
            match describe(id) {
                Ok(device) => devices.push(device),
                // A device can disappear between the listing and the query.
                Err(error) => tracing::debug!(id, %error, "skipping a device"),
            }
        }
        Ok(devices)
    }

    /// Devices that can be used as the microphone.
    ///
    /// Aggregate devices are excluded: ours is private and should never be
    /// offered back to the user as an input.
    pub fn inputs() -> Result<Vec<Device>> {
        let mut devices = all()?;
        devices.retain(Device::can_capture);
        Ok(devices)
    }

    /// Devices that can receive the cleaned signal.
    pub fn outputs() -> Result<Vec<Device>> {
        let mut devices = all()?;
        devices.retain(Device::can_play);
        Ok(devices)
    }

    /// The device the system is currently using as its microphone.
    pub fn default_input() -> Result<Device> {
        let id: AudioObjectId = property(
            SYSTEM_OBJECT,
            &AudioObjectPropertyAddress::global(HARDWARE_DEFAULT_INPUT),
            "reading the default input device",
        )?;
        describe(id)
    }

    /// Looks a device up by its persistent UID.
    pub fn by_uid(uid: &str) -> Result<Device> {
        all()?
            .into_iter()
            .find(|device| device.uid == uid)
            .ok_or_else(|| Error::DeviceNotFound(uid.to_owned()))
    }

    /// Whether `id` is an aggregate device.
    #[must_use]
    pub fn is_aggregate(id: AudioObjectId) -> bool {
        property::<u32>(
            id,
            &AudioObjectPropertyAddress::global(DEVICE_TRANSPORT_TYPE),
            "reading the transport type",
        )
        .is_ok_and(|transport| transport == TRANSPORT_TYPE_AGGREGATE)
    }
}

/// Stubs so that the crate still builds, lints, and tests away from macOS.
///
/// Every entry point reports [`crate::Error::Unsupported`] rather than
/// panicking, so a caller that ignores the platform gets an error it can show
/// rather than a crash.
#[cfg(not(target_os = "macos"))]
mod imp {
    use super::Device;
    use crate::error::{Error, Result};

    /// Every device the HAL knows about.
    ///
    /// # Errors
    ///
    /// Always [`Error::Unsupported`] away from macOS.
    pub const fn all() -> Result<Vec<Device>> {
        Err(Error::Unsupported)
    }

    /// Devices that can be used as the microphone.
    ///
    /// # Errors
    ///
    /// Always [`Error::Unsupported`] away from macOS.
    pub const fn inputs() -> Result<Vec<Device>> {
        Err(Error::Unsupported)
    }

    /// Devices that can receive the cleaned signal.
    ///
    /// # Errors
    ///
    /// Always [`Error::Unsupported`] away from macOS.
    pub const fn outputs() -> Result<Vec<Device>> {
        Err(Error::Unsupported)
    }

    /// The device the system is currently using as its microphone.
    ///
    /// # Errors
    ///
    /// Always [`Error::Unsupported`] away from macOS.
    pub const fn default_input() -> Result<Device> {
        Err(Error::Unsupported)
    }

    /// Looks a device up by its persistent UID.
    ///
    /// # Errors
    ///
    /// Always [`Error::Unsupported`] away from macOS.
    pub const fn by_uid(_uid: &str) -> Result<Device> {
        Err(Error::Unsupported)
    }

    /// Whether `id` is an aggregate device.
    #[must_use]
    pub const fn is_aggregate(_id: u32) -> bool {
        false
    }
}

pub use imp::{all, by_uid, default_input, inputs, is_aggregate, outputs};

/// Picks the most likely virtual output device from `devices`.
///
/// Prefers a device the HAL reports as virtual with at least two output
/// channels. Name matching is only a tie-breaker, because a renamed
/// `BlackHole` fork — which is exactly what this project ships
/// (`docs/tech-research.md` §3) — would not match its own name.
#[must_use]
pub fn suggest_virtual_output(devices: &[Device]) -> Option<&Device> {
    let candidates: Vec<&Device> = devices
        .iter()
        .filter(|device| device.is_virtual && device.can_play())
        .collect();
    candidates
        .iter()
        .find(|device| {
            let name = device.name.to_ascii_lowercase();
            name.contains("noican") || name.contains("blackhole")
        })
        .or_else(|| candidates.first())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::{Device, suggest_virtual_output};

    fn device(name: &str, is_virtual: bool, inputs: u32, outputs: u32) -> Device {
        Device {
            id: 1,
            uid: format!("uid-{name}"),
            name: name.to_owned(),
            input_channels: inputs,
            output_channels: outputs,
            sample_rate: 48_000,
            is_virtual,
        }
    }

    #[test]
    fn capability_flags_follow_the_channel_counts() {
        let microphone = device("MacBook Pro Microphone", false, 3, 0);
        assert!(microphone.can_capture());
        assert!(!microphone.can_play());

        let speakers = device("MacBook Pro Speakers", false, 0, 2);
        assert!(!speakers.can_capture());
        assert!(speakers.can_play());
    }

    #[test]
    fn the_virtual_device_is_found_by_transport_type() {
        let devices = vec![
            device("MacBook Pro Microphone", false, 3, 0),
            device("External Headphones", false, 0, 2),
            device("Some Virtual Thing", true, 2, 2),
        ];
        assert_eq!(
            suggest_virtual_output(&devices).map(|d| d.name.as_str()),
            Some("Some Virtual Thing")
        );
    }

    #[test]
    fn a_known_name_wins_over_another_virtual_device() {
        let devices = vec![
            device("Some Other Virtual Device", true, 2, 2),
            device("noican 2ch", true, 2, 2),
        ];
        assert_eq!(
            suggest_virtual_output(&devices).map(|d| d.name.as_str()),
            Some("noican 2ch")
        );
    }

    #[test]
    fn nothing_is_suggested_when_no_virtual_output_exists() {
        let devices = vec![
            device("MacBook Pro Microphone", false, 3, 0),
            // Virtual but capture-only, so it cannot receive the signal.
            device("Capture Only", true, 2, 0),
        ];
        assert!(suggest_virtual_output(&devices).is_none());
        assert!(suggest_virtual_output(&[]).is_none());
    }
}
