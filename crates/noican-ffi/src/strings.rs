//! Fixed-size strings for the C ABI.
//!
//! Returning owned `char *` across the boundary would mean Swift has to free
//! them, which is one more thing to get wrong for no benefit at these sizes. A
//! fixed array is copied into the caller's struct and needs no cleanup.

use std::ffi::c_char;

use crate::STRING_CAPACITY;

/// A NUL-terminated byte array of [`STRING_CAPACITY`] bytes.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct StringBuffer {
    /// The bytes, always NUL-terminated.
    pub bytes: [c_char; STRING_CAPACITY],
}

impl std::fmt::Debug for StringBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.to_string())
    }
}

impl std::fmt::Display for StringBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes: Vec<u8> = self
            .bytes
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| byte.to_le_bytes()[0])
            .collect();
        f.write_str(&String::from_utf8_lossy(&bytes))
    }
}

/// Copies `text` into a fixed buffer, truncating on a character boundary.
///
/// Truncating rather than rejecting: a device name long enough to overflow is a
/// display problem, not a reason to hide the device. Truncation respects UTF-8
/// boundaries so the result never becomes invalid.
#[must_use]
pub fn copy_into(text: &str) -> StringBuffer {
    let mut bytes: [c_char; STRING_CAPACITY] = [0; STRING_CAPACITY];
    let limit = STRING_CAPACITY - 1;

    let mut end = text.len().min(limit);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    for (slot, byte) in bytes.iter_mut().zip(text.as_bytes()[..end].iter()) {
        *slot = c_char::from_ne_bytes([*byte]);
    }
    StringBuffer { bytes }
}

#[cfg(test)]
mod tests {
    use super::{StringBuffer, copy_into};
    use crate::STRING_CAPACITY;

    #[test]
    fn short_strings_round_trip() {
        assert_eq!(copy_into("BlackHole 2ch").to_string(), "BlackHole 2ch");
        assert_eq!(copy_into("").to_string(), "");
    }

    #[test]
    fn the_buffer_is_always_terminated() {
        let buffer = copy_into(&"a".repeat(1_000));
        assert_eq!(buffer.bytes[STRING_CAPACITY - 1], 0);
        assert_eq!(buffer.to_string().len(), STRING_CAPACITY - 1);
    }

    #[test]
    fn truncation_respects_character_boundaries() {
        // Three-byte characters that cannot divide the limit evenly.
        let text = "あ".repeat(200);
        let buffer = copy_into(&text);
        let rendered = buffer.to_string();
        // Never invalid UTF-8, so no replacement characters appear.
        assert!(!rendered.contains('\u{fffd}'), "{rendered}");
        assert!(rendered.len() < STRING_CAPACITY);
        assert!(text.starts_with(&rendered));
    }

    #[test]
    fn the_layout_is_exactly_the_byte_array() {
        assert_eq!(size_of::<StringBuffer>(), STRING_CAPACITY);
        assert_eq!(align_of::<StringBuffer>(), 1);
    }

    #[test]
    fn debug_shows_the_text() {
        assert_eq!(format!("{:?}", copy_into("hi")), "\"hi\"");
    }
}
