//! The private aggregate device that keeps the two clocks together.
//!
//! The physical microphone and the virtual output device run on different
//! clocks — 48000.0 Hz against 47999.8 Hz, say. Left alone, the ring buffer
//! between them slowly over- or underflows and produces a click every few
//! minutes. `docs/tech-research.md` §4.2 makes an aggregate device with drift
//! compensation the mandatory countermeasure, and this is it.
//!
//! Three details matter beyond simply creating one. The aggregate is marked
//! private so it never shows up in Sound Settings — a user should not be able
//! to select our plumbing as their microphone. Drift compensation is enabled on
//! the *non-clock* sub-device only: the clock source defines the timeline, so
//! asking it to compensate against itself is meaningless.
//!
//! And creation **returns before the device works.** Opening an I/O proc on a
//! freshly created aggregate delivers a stream of silent buffers with no error
//! anywhere, so [`AggregateDevice::create`] waits for
//! `kAudioDevicePropertyDeviceIsAlive` before handing the device back.

/// A private aggregate device, destroyed when dropped.
#[derive(Debug)]
pub struct AggregateDevice {
    id: u32,
    uid: String,
}

impl AggregateDevice {
    /// The HAL identifier to open an I/O proc on.
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// The UID this aggregate was created with.
    #[must_use]
    pub fn uid(&self) -> &str {
        &self.uid
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use core::ffi::c_void;
    use std::time::{Duration, Instant};

    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;

    use super::AggregateDevice;
    use crate::error::{Error, Result, check};
    use crate::sys::{
        self, AudioObjectId, AudioObjectPropertyAddress, DRIFT_COMPENSATION_MAX_QUALITY,
        aggregate_keys as keys,
    };

    /// How long to wait for a freshly created aggregate to report itself alive.
    const ALIVE_TIMEOUT: Duration = Duration::from_secs(3);

    /// How often to re-check while waiting.
    const ALIVE_POLL_INTERVAL: Duration = Duration::from_millis(50);

    /// Builds the sub-device dictionary for one member of the aggregate.
    fn sub_device(uid: &str, drift_compensation: bool) -> CFDictionary<CFString, CFType> {
        let mut entries: Vec<(CFString, CFType)> = vec![(
            CFString::from_static_string(keys::SUB_DEVICE_UID),
            CFString::new(uid).as_CFType(),
        )];
        if drift_compensation {
            entries.push((
                CFString::from_static_string(keys::DRIFT_COMPENSATION),
                CFNumber::from(1i32).as_CFType(),
            ));
            entries.push((
                CFString::from_static_string(keys::DRIFT_COMPENSATION_QUALITY),
                CFNumber::from(DRIFT_COMPENSATION_MAX_QUALITY).as_CFType(),
            ));
        }
        CFDictionary::from_CFType_pairs(&entries)
    }

    impl AggregateDevice {
        /// Creates a private aggregate of `clock_source_uid` and `follower_uid`.
        ///
        /// `clock_source_uid` becomes the timing reference — it should be the
        /// physical microphone, whose clock we cannot influence. The follower
        /// gets drift compensation at maximum quality.
        ///
        /// # Errors
        ///
        /// Returns [`Error::CoreAudio`] if the HAL refuses to create the
        /// device, or [`Error::UnsuitableDevice`] if the two UIDs are the same.
        pub fn create(
            name: &str,
            uid: &str,
            clock_source_uid: &str,
            follower_uid: &str,
        ) -> Result<Self> {
            if clock_source_uid == follower_uid {
                return Err(Error::UnsuitableDevice(format!(
                    "the microphone and the virtual output cannot both be `{clock_source_uid}`"
                )));
            }

            let sub_devices = CFArray::from_CFTypes(&[
                sub_device(clock_source_uid, false).as_CFType(),
                sub_device(follower_uid, true).as_CFType(),
            ]);

            let description = CFDictionary::from_CFType_pairs(&[
                (
                    CFString::from_static_string(keys::NAME),
                    CFString::new(name).as_CFType(),
                ),
                (
                    CFString::from_static_string(keys::UID),
                    CFString::new(uid).as_CFType(),
                ),
                (
                    CFString::from_static_string(keys::SUB_DEVICE_LIST),
                    sub_devices.as_CFType(),
                ),
                (
                    CFString::from_static_string(keys::MAIN_SUB_DEVICE),
                    CFString::new(clock_source_uid).as_CFType(),
                ),
                (
                    CFString::from_static_string(keys::IS_PRIVATE),
                    CFBoolean::true_value().as_CFType(),
                ),
                (
                    CFString::from_static_string(keys::IS_STACKED),
                    CFBoolean::false_value().as_CFType(),
                ),
            ]);

            let mut id: AudioObjectId = 0;
            // SAFETY: `description` is a valid CFDictionary for the duration of
            // the call, and `id` is a valid out-parameter.
            let status = unsafe {
                sys::AudioHardwareCreateAggregateDevice(
                    description.as_CFTypeRef().cast::<c_void>(),
                    &raw mut id,
                )
            };
            check("creating the aggregate device", status)?;

            let device = Self {
                id,
                uid: uid.to_owned(),
            };
            device.wait_until_alive()?;

            tracing::info!(id, uid, clock_source_uid, follower_uid, "aggregate created");
            Ok(device)
        }

        /// Blocks until the HAL reports the device alive.
        ///
        /// Creation returns before the device is usable. Opening an I/O proc
        /// too early yields silent buffers and no error, so this has to be a
        /// hard gate rather than a hopeful sleep.
        fn wait_until_alive(&self) -> Result<()> {
            let address = AudioObjectPropertyAddress::global(sys::DEVICE_IS_ALIVE);
            let deadline = Instant::now() + ALIVE_TIMEOUT;
            loop {
                let mut alive: u32 = 0;
                let mut size = u32::try_from(size_of::<u32>()).unwrap_or(4);
                // SAFETY: `alive` has room for exactly `size` bytes.
                let status = unsafe {
                    sys::AudioObjectGetPropertyData(
                        self.id,
                        &raw const address,
                        0,
                        core::ptr::null(),
                        &raw mut size,
                        (&raw mut alive).cast::<c_void>(),
                    )
                };
                if status == 0 && alive != 0 {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(Error::UnsuitableDevice(format!(
                        "the aggregate device `{}` never reported itself alive; an I/O proc \
                         opened on it would deliver silence",
                        self.uid
                    )));
                }
                std::thread::sleep(ALIVE_POLL_INTERVAL);
            }
        }

        /// Asks for 48 kHz and reports the rate the device actually settled on.
        ///
        /// Setting the nominal rate returns success before the change takes
        /// effect, and a device may refuse the request outright, so the return
        /// value is a read-back rather than the requested value. The caller
        /// configures the engine from what came back: the resampler handles any
        /// rational ratio, but only if it is told the truth about the host rate.
        ///
        /// # Errors
        ///
        /// Returns [`Error::CoreAudio`] if the rate cannot be read at all.
        pub fn negotiate_sample_rate(&self, preferred: u32) -> Result<u32> {
            let address = AudioObjectPropertyAddress::global(sys::DEVICE_NOMINAL_SAMPLE_RATE);
            let requested = f64::from(preferred);
            // SAFETY: the property takes a `Float64`, which is what is passed.
            let status = unsafe {
                sys::AudioObjectSetPropertyData(
                    self.id,
                    &raw const address,
                    0,
                    core::ptr::null(),
                    u32::try_from(size_of::<f64>()).unwrap_or(8),
                    (&raw const requested).cast::<c_void>(),
                )
            };
            if status != 0 {
                tracing::debug!(
                    status,
                    preferred,
                    "the aggregate refused the requested sample rate; using its own"
                );
            }

            let mut actual = 0f64;
            let mut size = u32::try_from(size_of::<f64>()).unwrap_or(8);
            // SAFETY: `actual` has room for exactly `size` bytes. The device is
            // not running I/O yet, which is what makes this read-back reliable.
            let status = unsafe {
                sys::AudioObjectGetPropertyData(
                    self.id,
                    &raw const address,
                    0,
                    core::ptr::null(),
                    &raw mut size,
                    (&raw mut actual).cast::<c_void>(),
                )
            };
            check("reading the aggregate's sample rate", status)?;

            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a nominal sample rate is a small positive integer"
            )]
            let actual = actual.round() as u32;
            if actual == 0 {
                return Err(Error::UnsuitableDevice(
                    "the aggregate device reports a sample rate of zero".to_owned(),
                ));
            }
            if actual != preferred {
                tracing::warn!(
                    preferred,
                    actual,
                    "running at the device's rate rather than the preferred one"
                );
            }
            Ok(actual)
        }
    }

    impl Drop for AggregateDevice {
        fn drop(&mut self) {
            // SAFETY: `self.id` came from a successful create and has not been
            // destroyed, because destroying it is what this does.
            let status = unsafe { sys::AudioHardwareDestroyAggregateDevice(self.id) };
            if status != 0 {
                tracing::warn!(
                    id = self.id,
                    status,
                    "could not destroy the aggregate device; it may linger until logout"
                );
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::AggregateDevice;
    use crate::error::{Error, Result};

    impl AggregateDevice {
        /// Creates a private aggregate of `clock_source_uid` and `follower_uid`.
        ///
        /// # Errors
        ///
        /// Always returns [`Error::Unsupported`] away from macOS.
        pub const fn create(
            _name: &str,
            _uid: &str,
            _clock_source_uid: &str,
            _follower_uid: &str,
        ) -> Result<Self> {
            Err(Error::Unsupported)
        }

        /// Asks for `preferred` and reports the rate the device settled on.
        ///
        /// # Errors
        ///
        /// Always returns [`Error::Unsupported`] away from macOS.
        pub const fn negotiate_sample_rate(&self, _preferred: u32) -> Result<u32> {
            Err(Error::Unsupported)
        }
    }
}

/// A UID for our aggregate that will not collide with anyone else's.
#[must_use]
pub fn default_uid() -> String {
    format!("com.noican.aggregate.{}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::default_uid;

    #[test]
    fn the_uid_is_namespaced_and_per_process() {
        let uid = default_uid();
        assert!(uid.starts_with("com.noican.aggregate."), "{uid}");
        // Two runs of the app must not fight over one aggregate.
        assert!(uid.ends_with(&std::process::id().to_string()));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn creation_is_unsupported_away_from_macos() {
        use super::AggregateDevice;
        assert!(matches!(
            AggregateDevice::create("n", "u", "a", "b"),
            Err(crate::Error::Unsupported)
        ));
    }
}
