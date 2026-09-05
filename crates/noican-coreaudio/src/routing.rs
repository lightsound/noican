//! Output-channel routing for the aggregate transport: where the virtual
//! output sits inside the private Aggregate Device, and the AUHAL channel
//! map that steers the mono engine output there.
//!
//! # The bug this closes
//!
//! The private aggregate is composed as `[microphone, virtual output]`
//! (the microphone is the main subdevice and clock master), and an
//! aggregate's output channels are the concatenation of its subdevices'
//! output channels in that order. The transport renders a **mono** client
//! stream, and AUHAL's default output channel map is the identity:
//! client channel 0 → device output channel 0, every other device channel
//! silent. That only reaches the virtual output when the microphone has
//! no output channels of its own (the built-in microphone). A composite
//! input/output device — a USB microphone with a headphone jack such as
//! the Shure MV7+, or any audio interface — puts its own output channels
//! first, so the processed voice went to the microphone's headphone
//! output and the virtual microphone recorded silence. The preview
//! monitor was unaffected (it has its own AUHAL on the default output),
//! which is why the symptom was "preview works, recordings are silent".
//!
//! # Chosen fix
//!
//! Set `kAudioOutputUnitProperty_ChannelMap` on the aggregate AUHAL's
//! output element explicitly: the virtual output's **first** channel
//! receives client channel 0 and every other device channel — the
//! microphone's own outputs and the virtual output's remaining channels
//! — is `-1` (silent). That reproduces, at the virtual output's actual
//! position, exactly the signal shape the working built-in-microphone
//! layout always had (engine signal on the virtual output's channel 0,
//! silence on channel 1), so consumers see no level or channel change
//! on that layout. Duplicating the mono signal into every virtual
//! output channel (as the split transport and the monitor do in their
//! own callbacks) was considered and deferred: no primary source states
//! that an AUHAL map may point several device channels at one client
//! channel, a rejected map would make every aggregate start fail —
//! including the layout that works today — and it would shift the level
//! seen by stereo-averaging consumers; it can be revisited on its own
//! once hardware has confirmed the map, and no test here assumes it.
//! The virtual output's position is computed by the control plane, which
//! composes the subdevice list and therefore knows the layout by
//! construction, and is validated here against the channel count AUHAL
//! reports for the device: a mismatch refuses to start instead of
//! misrouting silently, which is exactly the failure class this module
//! exists to prevent.
//!
//! # Rejected alternatives
//!
//! - **Reorder the subdevices to `[virtual output, microphone]`.** The
//!   `BlackHole`-derived loopback registers input channels too, so the
//!   aggregate's *input* channel 0 would become the loopback's own input
//!   and the (unchanged, identity-mapped) capture side would record the
//!   loopback instead of the microphone: the silent-microphone bug
//!   would only change sides. The capture invariant — aggregate input
//!   channel 0 is the microphone — must hold, and this fix leaves the
//!   input side untouched.
//! - **Render a device-wide multichannel client format and write the
//!   right slots in the callback.** The render callback reuses the
//!   device buffer for `AudioUnitRender` capture, so it would need a
//!   separate preallocated capture buffer and per-frame scatter loops;
//!   it re-implements in real-time code what AUHAL's channel map does
//!   at setup time.
//! - **Discover the layout on the Rust side** (`kAudioAggregateDevice-
//!   PropertyActiveSubDeviceList` plus per-subdevice stream
//!   configurations). Self-contained, but it adds a second block of
//!   unsafe Core Audio property code, duplicates the virtual-output
//!   identification policy that lives in the Swift device catalog, and
//!   still needs the same device-count check to be trusted. The control
//!   plane already has every number involved.

use crate::CoreAudioError;

/// AUHAL channel map entry meaning "no client channel feeds this device
/// channel" (the device channel renders silence).
pub const UNMAPPED_CHANNEL: i32 = -1;

/// The client channel carrying the mono engine output.
const ENGINE_CHANNEL: i32 = 0;

/// Position of the virtual output's channels within an Aggregate Device's
/// output channel list.
///
/// `first` is the number of output channels contributed by the subdevices
/// ahead of the virtual output (0 for a microphone without outputs, 2 for
/// a microphone with a stereo headphone jack), and `count` is the virtual
/// output's own channel count. Constructed by the control plane from the
/// subdevice list it composes; see [`render_channel_map`] for how it is
/// checked against the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualOutputChannels {
    first: u32,
    count: u32,
}

impl VirtualOutputChannels {
    /// Describes `count` virtual-output channels starting at device
    /// output channel `first`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreAudioError::OutputRouting`] when `count` is zero (a
    /// virtual output without output channels cannot receive audio) or
    /// when the range does not fit a `u32`.
    pub fn new(first: u32, count: u32) -> Result<Self, CoreAudioError> {
        if count == 0 {
            return Err(CoreAudioError::OutputRouting(
                "the virtual output reports no output channels".to_owned(),
            ));
        }
        if first.checked_add(count).is_none() {
            return Err(CoreAudioError::OutputRouting(format!(
                "virtual output channel range {first}+{count} overflows"
            )));
        }
        Ok(Self { first, count })
    }

    /// Device output channel index of the virtual output's first channel.
    #[must_use]
    pub const fn first(self) -> u32 {
        self.first
    }

    /// Number of virtual output channels.
    #[must_use]
    pub const fn count(self) -> u32 {
        self.count
    }

    /// One past the virtual output's last device output channel; equals
    /// the aggregate's total output channel count when the virtual
    /// output is the last subdevice (the control plane's layout).
    #[must_use]
    pub const fn end(self) -> u32 {
        self.first + self.count
    }
}

/// Builds the AUHAL output channel map that routes a mono client stream
/// to `target` on a device with `device_channels` output channels.
///
/// The map holds one `i32` per device channel: the client channel that
/// feeds it, or [`UNMAPPED_CHANNEL`]. Only `target`'s first channel
/// receives client channel 0; every other channel — ahead of the virtual
/// output and inside it — is silent (see the module docs for why the
/// mono signal is not fanned out across the virtual output).
///
/// The map is only valid when `target` ends exactly at the device's last
/// channel: the control plane composes the aggregate as
/// `[microphone, virtual output]`, so the virtual output's channels are
/// the trailing ones and `target.end()` must equal `device_channels`. Any
/// other value means the composition and the device disagree about the
/// layout, and the transport must not guess where the virtual output is.
///
/// # Errors
///
/// Returns [`CoreAudioError::OutputRouting`] when `target.end()` differs
/// from `device_channels`.
pub fn render_channel_map(
    device_channels: u32,
    target: VirtualOutputChannels,
) -> Result<Vec<i32>, CoreAudioError> {
    if target.end() != device_channels {
        return Err(CoreAudioError::OutputRouting(format!(
            "the aggregate device reports {device_channels} output channel(s), but the virtual \
             output was expected at channels {}..{} (microphone outputs first)",
            target.first(),
            target.end()
        )));
    }
    let len = usize::try_from(device_channels).map_err(|error| {
        CoreAudioError::OutputRouting(format!("channel count overflow: {error}"))
    })?;
    let first = usize::try_from(target.first()).map_err(|error| {
        CoreAudioError::OutputRouting(format!("channel offset overflow: {error}"))
    })?;
    let mut map = vec![UNMAPPED_CHANNEL; len];
    // In range by construction (`count >= 1` and `end() == device_channels`);
    // a plain index rather than a silent `get_mut`, because an all-`-1` map
    // would be exactly the silent virtual microphone this module prevents.
    map[first] = ENGINE_CHANNEL;
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_microphone_layout_matches_auhal_default() {
        // No microphone outputs: the virtual output is channels 0..2, and
        // the map must equal AUHAL's identity default for a mono client
        // (signal on channel 0, channel 1 silent) so the layout that
        // always worked keeps its exact signal shape.
        let target = VirtualOutputChannels::new(0, 2).expect("valid range");
        assert_eq!(
            render_channel_map(2, target).expect("map"),
            vec![0, UNMAPPED_CHANNEL]
        );
    }

    #[test]
    fn headphone_microphone_layout_skips_the_microphone_outputs() {
        // Shure MV7+: stereo headphone jack first, then the virtual output.
        let target = VirtualOutputChannels::new(2, 2).expect("valid range");
        assert_eq!(
            render_channel_map(4, target).expect("map"),
            vec![UNMAPPED_CHANNEL, UNMAPPED_CHANNEL, 0, UNMAPPED_CHANNEL]
        );
    }

    #[test]
    fn multichannel_interface_layout_is_handled() {
        // An 8-output interface ahead of a stereo virtual output.
        let target = VirtualOutputChannels::new(8, 2).expect("valid range");
        let map = render_channel_map(10, target).expect("map");
        assert_eq!(map.len(), 10);
        assert!(map[..8].iter().all(|&entry| entry == UNMAPPED_CHANNEL));
        assert_eq!(&map[8..], &[0, UNMAPPED_CHANNEL]);
    }

    #[test]
    fn exactly_one_device_channel_carries_the_engine() {
        for (first, count) in [(0, 1), (0, 2), (2, 2), (6, 8)] {
            let target = VirtualOutputChannels::new(first, count).expect("valid range");
            let map = render_channel_map(first + count, target).expect("map");
            let fed: Vec<usize> = map
                .iter()
                .enumerate()
                .filter(|&(_, &entry)| entry != UNMAPPED_CHANNEL)
                .map(|(index, _)| index)
                .collect();
            assert_eq!(fed, vec![usize::try_from(first).expect("small")]);
        }
    }

    #[test]
    fn layout_mismatch_refuses_instead_of_guessing() {
        let target = VirtualOutputChannels::new(2, 2).expect("valid range");
        for device_channels in [2, 3, 5] {
            let error = render_channel_map(device_channels, target).expect_err("must refuse");
            assert!(
                matches!(error, CoreAudioError::OutputRouting(_)),
                "unexpected error kind: {error:?}"
            );
            let message = error.to_string();
            assert!(
                message.contains(&device_channels.to_string()) && message.contains("2..4"),
                "unhelpful message: {message}"
            );
        }
    }

    #[test]
    fn empty_or_overflowing_ranges_are_rejected() {
        assert!(matches!(
            VirtualOutputChannels::new(0, 0),
            Err(CoreAudioError::OutputRouting(_))
        ));
        assert!(matches!(
            VirtualOutputChannels::new(u32::MAX, 1),
            Err(CoreAudioError::OutputRouting(_))
        ));
        let range = VirtualOutputChannels::new(3, 4).expect("valid range");
        assert_eq!(range.first(), 3);
        assert_eq!(range.count(), 4);
        assert_eq!(range.end(), 7);
    }
}
