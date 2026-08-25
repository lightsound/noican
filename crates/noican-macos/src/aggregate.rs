//! The private aggregate device that keeps the two clocks together.
//!
//! The physical microphone and the virtual output device run on different
//! clocks — 48000.0 Hz against 47999.8 Hz, say. Left alone, the ring buffer
//! between them slowly over- or underflows and produces a click every few
//! minutes. `docs/tech-research.md` §4.2 makes an aggregate device with drift
//! compensation the mandatory countermeasure, and this is it.
//!
//! Two details matter beyond simply creating one. The aggregate is marked
//! private so it never shows up in Sound Settings — a user should not be able
//! to select our plumbing as their microphone. And drift compensation is
//! enabled on the *non-clock* sub-device only: the clock source defines the
//! timeline, so asking it to compensate against itself is meaningless.

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

    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;

    use super::AggregateDevice;
    use crate::error::{Error, Result, check};
    use crate::sys::{self, AudioObjectId, DRIFT_COMPENSATION_MAX_QUALITY, aggregate_keys as keys};

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

            tracing::info!(id, uid, clock_source_uid, follower_uid, "aggregate created");
            Ok(Self {
                id,
                uid: uid.to_owned(),
            })
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
