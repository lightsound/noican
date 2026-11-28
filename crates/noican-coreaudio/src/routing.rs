//! See docs/adr/0004-aggregate-output-routing.md#aggregate-output-routing.

use crate::CoreAudioError;

/// AUHAL channel map entry meaning "no client channel feeds this device
/// channel" (the device channel renders silence).
pub const UNMAPPED_CHANNEL: i32 = -1;

/// See docs/adr/0004-aggregate-output-routing.md#aggregate-output-routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualOutputChannels {
    first: u32,
    count: u32,
}

impl VirtualOutputChannels {
    /// See docs/adr/0004-aggregate-output-routing.md#aggregate-output-routing.
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

    /// Number of virtual output channels (and of client render channels).
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

/// See docs/adr/0004-aggregate-output-routing.md#aggregate-output-routing.
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
    for (client_channel, entry) in (0_i32..).zip(&mut map[first..]) {
        *entry = client_channel;
    }
    Ok(map)
}

/// See docs/adr/0003-coreaudio-transport.md#split-transport-render-format.
///
/// # Errors
///
/// Returns [`CoreAudioError::OutputRouting`] when the device reports no
/// output channels: a virtual output without output channels cannot
/// receive audio, and guessing a width (the old constant 2) would either
/// be refused by AUHAL or misdescribe the device — the transport refuses
/// to start instead.
pub fn split_render_channels(device_output_channels: u32) -> Result<u32, CoreAudioError> {
    if device_output_channels == 0 {
        return Err(CoreAudioError::OutputRouting(
            "the virtual output device reports no output channels (split transport)".to_owned(),
        ));
    }
    Ok(device_output_channels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_render_format_follows_the_virtual_output_width() {
        assert_eq!(split_render_channels(2).expect("stereo"), 2);
        assert_eq!(split_render_channels(1).expect("mono"), 1);
        assert_eq!(split_render_channels(16).expect("wide"), 16);
    }

    #[test]
    fn split_render_format_refuses_a_channelless_virtual_output() {
        let error = split_render_channels(0).expect_err("must refuse");
        assert!(
            matches!(error, CoreAudioError::OutputRouting(_)),
            "unexpected error kind: {error:?}"
        );
        let message = error.to_string();
        assert!(
            message.starts_with("virtual output routing failed")
                && message.contains("no output channels")
                && message.contains("split transport"),
            "unhelpful message: {message}"
        );
    }

    #[test]
    fn built_in_microphone_layout_feeds_both_virtual_output_channels() {
        let target = VirtualOutputChannels::new(0, 2).expect("valid range");
        assert_eq!(render_channel_map(2, target).expect("map"), vec![0, 1]);
    }

    #[test]
    fn headphone_microphone_layout_skips_the_microphone_outputs() {
        let target = VirtualOutputChannels::new(2, 2).expect("valid range");
        assert_eq!(
            render_channel_map(4, target).expect("map"),
            vec![UNMAPPED_CHANNEL, UNMAPPED_CHANNEL, 0, 1]
        );
    }

    #[test]
    fn multichannel_interface_layout_is_handled() {
        let target = VirtualOutputChannels::new(8, 2).expect("valid range");
        let map = render_channel_map(10, target).expect("map");
        assert_eq!(map.len(), 10);
        assert!(map[..8].iter().all(|&entry| entry == UNMAPPED_CHANNEL));
        assert_eq!(&map[8..], &[0, 1]);
    }

    #[test]
    fn single_channel_virtual_output_keeps_the_mono_shape() {
        let target = VirtualOutputChannels::new(0, 1).expect("valid range");
        assert_eq!(render_channel_map(1, target).expect("map"), vec![0]);
        let target = VirtualOutputChannels::new(2, 1).expect("valid range");
        assert_eq!(
            render_channel_map(3, target).expect("map"),
            vec![UNMAPPED_CHANNEL, UNMAPPED_CHANNEL, 0]
        );
    }

    #[test]
    fn every_virtual_output_channel_is_fed_one_to_one() {
        for (first, count) in [(0, 1), (0, 2), (2, 2), (6, 8)] {
            let target = VirtualOutputChannels::new(first, count).expect("valid range");
            let map = render_channel_map(first + count, target).expect("map");
            let fed: Vec<(usize, i32)> = map
                .iter()
                .enumerate()
                .filter(|&(_, &entry)| entry != UNMAPPED_CHANNEL)
                .map(|(index, &entry)| (index, entry))
                .collect();
            let expected: Vec<(usize, i32)> = (0..count)
                .map(|i| {
                    (
                        usize::try_from(first + i).expect("small"),
                        i32::try_from(i).expect("small"),
                    )
                })
                .collect();
            assert_eq!(fed, expected, "layout first={first} count={count}");
            let mut clients: Vec<i32> = fed.iter().map(|&(_, client)| client).collect();
            clients.dedup();
            assert_eq!(clients.len(), fed.len());
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
