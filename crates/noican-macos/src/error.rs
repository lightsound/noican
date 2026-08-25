//! Error type for the Core Audio layer.

/// Errors produced while talking to the HAL.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A Core Audio call returned a non-zero status.
    #[error("{operation} failed: {}", describe(*status))]
    CoreAudio {
        /// What was being attempted.
        operation: &'static str,
        /// The `OSStatus` returned.
        status: i32,
    },

    /// No device matched the requested identifier or UID.
    #[error("no audio device matches `{0}`")]
    DeviceNotFound(String),

    /// A device cannot be used for the role it was chosen for.
    #[error("{0}")]
    UnsuitableDevice(String),

    /// The engine rejected the configuration.
    #[error(transparent)]
    Engine(#[from] noican_engine::Error),

    /// This platform has no Core Audio.
    #[error("Core Audio is only available on macOS")]
    Unsupported,
}

/// Convenience alias for results produced by this crate.
pub type Result<T> = core::result::Result<T, Error>;

/// Renders an `OSStatus` as its four-character code where it is one.
///
/// Core Audio reports almost everything as a four-character code packed into an
/// integer, so the decimal value alone is close to useless when reading a log.
fn describe(status: i32) -> String {
    let bytes = status.to_be_bytes();
    if bytes.iter().all(u8::is_ascii_graphic) {
        let code = String::from_utf8_lossy(&bytes);
        format!("'{code}' ({status})")
    } else {
        format!("OSStatus {status}")
    }
}

/// Converts a Core Audio status into a result.
// `allow` rather than `expect`: away from macOS this is unused in the library
// build but used by the tests, so an `expect` would be unfulfilled in one of
// the two compilations no matter which way it is written.
#[cfg_attr(
    not(target_os = "macos"),
    allow(
        dead_code,
        reason = "only the macOS implementation modules call this; it stays compiled elsewhere so \
                  that its status formatting remains under test on Linux CI"
    )
)]
pub(crate) const fn check(operation: &'static str, status: i32) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(Error::CoreAudio { operation, status })
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, check, describe};

    #[test]
    fn success_is_not_an_error() {
        assert!(check("test", 0).is_ok());
    }

    #[test]
    fn four_character_codes_are_readable() {
        // 'nope' is what the HAL returns for an unknown property.
        let status = i32::from_be_bytes(*b"nope");
        assert_eq!(describe(status), format!("'nope' ({status})"));
        // -10875 has no printable form and must fall back to the number.
        assert_eq!(describe(-10_875), "OSStatus -10875");
    }

    #[test]
    fn errors_mention_the_operation() {
        let error = check("creating the aggregate device", -50).unwrap_err();
        assert!(
            error.to_string().contains("creating the aggregate device"),
            "{error}"
        );
        assert!(matches!(error, Error::CoreAudio { status: -50, .. }));
    }
}
