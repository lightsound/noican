//! Output-channel routing for the aggregate transport: where the virtual
//! output sits inside the private Aggregate Device, and the AUHAL channel
//! map that steers the engine output there.
//!
//! # The bug this closes
//!
//! The private aggregate is composed as `[microphone, virtual output]`
//! (the microphone is the main subdevice and clock master), and an
//! aggregate's output channels are the concatenation of its subdevices'
//! output channels in that order. AUHAL's default output channel map is
//! the identity: client channel 0 → device output channel 0, every other
//! device channel silent. That only reaches the virtual output when the
//! microphone has no output channels of its own (the built-in
//! microphone). A composite input/output device — a USB microphone with
//! a headphone jack such as the Shure MV7+, or any audio interface — puts
//! its own output channels first, so the processed voice went to the
//! microphone's headphone output and the virtual microphone recorded
//! silence. The preview monitor was unaffected (it has its own AUHAL on
//! the default output), which is why the symptom was "preview works,
//! recordings are silent".
//!
//! # Chosen design: dual mono, one-to-one channel map (B′)
//!
//! The transport renders a client stream with **as many channels as the
//! virtual output has**, and the render callback writes the mono engine
//! sample into every channel of each frame (exactly what the split
//! transport's output callback and the preview monitor already do). The
//! AUHAL channel map then only has to *place* those channels: virtual
//! output channel `i` receives client channel `i`, and every device
//! channel ahead of the virtual output — the microphone's own outputs —
//! is `-1` (silent). On the built-in-microphone layout the map is
//! `[0, 1]`, on a stereo-headphone microphone `[-1, -1, 0, 1]`.
//!
//! This replaces the first fix (PR #26), which kept a mono client stream
//! and mapped it to the virtual output's *first* channel only
//! (`[-1, -1, 0, -1]`). Hardware measurement (2026-09-05, built-in
//! microphone, Passthrough at 100%) showed what that meant for
//! consumers: virtual-microphone channel 0 at −16.5 dBFS, channel 1
//! silent — heard in the left ear only on headphones, and 6 dB quieter
//! than intended in the many meeting applications that average a stereo
//! input down to mono — while the split transport and the monitor fed
//! both channels. Three signal paths with two different channel shapes
//! is the inconsistency this design removes.
//!
//! Alternatives weighed:
//!
//! - **Fan out in the channel map** (`[-1, -1, 0, 0]`: one client channel
//!   named by several device channels). Smallest code change, but no
//!   primary source (AUHAL headers, TN2091, Apple's "Channel Maps" notes
//!   as reproduced in `PortAudio`) states that a device channel may
//!   share a client channel, and a rejected or silently trimmed map
//!   would break every aggregate start — including the built-in
//!   microphone layout that has always worked. A one-to-one map, by
//!   contrast, is the documented shape, and its single-entry form
//!   (`[-1, -1, 0, -1]`) was accepted and read back unchanged on
//!   hardware (an MV7-class microphone, 4-channel aggregate); the
//!   two-entry form this module now builds is pinned by the composite
//!   microphone acceptance checklist rather than measured yet. Rejected.
//! - **Multichannel client stream, duplicate in the callback** (this
//!   design). The duplication is a per-frame copy the other two paths
//!   already perform, and the map stays one-to-one. It costs the
//!   render callback a separate preallocated capture landing buffer
//!   (the mono render buffer could no longer double as the
//!   `AudioUnitRender` target) — bounded by
//!   `kAudioUnitProperty_MaximumFramesPerSlice`, as on the split path.
//!   Chosen.
//! - **Keep the single-channel map and document the asymmetry.** Leaves
//!   the left-ear-only and −6 dB consumer behaviour in place. Rejected.
//!
//! The design assumes nothing about the virtual output's channel count
//! beyond "at least one": with a 1-channel virtual output (the Noican
//! driver from 0.2.0 on — docs/driver.md, "History" — shaped like
//! Krisp's or `JoyCast`'s) the client format is mono, the map ends in a
//! single `0`, and the callback's per-frame loop writes one sample; with
//! a 2-channel one (stock `BlackHole` 2ch, the 0.1.0 driver) it is dual
//! mono. Nor does it depend on the virtual output having input channels;
//! an output-only device changes nothing here.
//!
//! The virtual output's position is computed by the control plane, which
//! composes the subdevice list and therefore knows the layout by
//! construction, and is validated here against the channel count AUHAL
//! reports for the device: a mismatch refuses to start instead of
//! misrouting silently, which is exactly the failure class this module
//! exists to prevent.
//!
//! # Rejected alternatives to the map itself
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
//!   right slots in the callback.** Re-implements in real-time code
//!   what AUHAL's channel map does at setup time, and the callback
//!   would have to know the microphone's output channel count.
//! - **Discover the layout on the Rust side** (`kAudioAggregateDevice-
//!   PropertyActiveSubDeviceList` plus per-subdevice stream
//!   configurations). Self-contained, but it adds a second block of
//!   unsafe Core Audio property code, duplicates the virtual-output
//!   identification policy that lives in the Swift device catalog, and
//!   still needs the same device-count check to be trusted. The control
//!   plane already has every number involved.
//!
//! # The split transport's render format
//!
//! The split transport (non-48 kHz microphones, `macos::split`) has no
//! aggregate and no map: its output-only AUHAL sits on the virtual
//! output device alone, so AUHAL's identity map is already right. What
//! it does share with the aggregate path is the dual-mono contract — one
//! client channel per virtual-output channel — and until PR #29 its
//! render format was hard-wired to two channels, i.e. to the 0.1.0
//! driver. [`split_render_channels`] replaces that constant with the
//! channel count read from the device, so a 1-channel virtual output
//! (the Noican driver from 0.2.0 on, shaped like Krisp's or `JoyCast`'s)
//! gets a mono client format and a 2-channel one (stock `BlackHole` 2ch,
//! the 0.1.0 driver) keeps its stereo one; whether AUHAL would even
//! accept a 2-channel client format on a 1-channel device is unverified,
//! and matching the device removes the question.
//!
//! Where the count comes from was decided against these criteria: no FFI
//! change (the `noican_engine_start_native` signature is a three-file
//! contract — `noican.h`, `RustEngine.swift`, `noican-ffi` — and the
//! Swift call site lives in a file at its lint length limit); as few
//! Core Audio behaviours as possible that hardware has not already
//! shown; every virtual output (1 ch, 2 ch, stock `BlackHole` 2ch)
//! served by the same code; a refusal, never a silent fallback, when
//! the count is unusable; and the real-time rules of the output
//! callback untouched.
//!
//! - **Read it on the Rust side from the output-only AUHAL** — the
//!   device-side stream format of the output element, once the current
//!   device is set (`kAudioUnitScope_Output`, element 0; TN2091: the
//!   device format, never writable, so the client format cannot leak
//!   into the read). This is the exact read the aggregate path already
//!   performs on its unit (`device_output_channels`), where hardware has
//!   shown it to return the device's total output channel count
//!   (2026-09-05, 2- and 4-channel aggregates). No FFI change; the
//!   value describes the very device the unit is bound to; one call, and
//!   its only unverified aspect is that the same property behaves the
//!   same on an output-only unit — which hardware acceptance pins
//!   (`docs/macos-hardware-test.md`, native-rate section: the `Split
//!   output routing` line). **Chosen.**
//! - **Pass it from Swift through the FFI**, as the aggregate path does
//!   (`AudioDeviceCatalog.liveOutputChannelCount` after the 48 kHz
//!   switch). Rejected: it costs the three-file FFI change, and the
//!   number it carries is not a *composed* layout that needs checking
//!   against the device — on this path the virtual output *is* the
//!   device, so the Swift value and the AUHAL read would describe the
//!   same single device through two Core Audio APIs. The aggregate path's
//!   double check guards a derived position (microphone outputs ahead of
//!   the virtual output); nothing is derived here. The read-back after
//!   `AudioUnitInitialize` recorded in the routing description gives the
//!   same diagnostic value without the extra channel.
//! - **Both: Swift passes the count and Rust cross-checks it against the
//!   AUHAL read** (the aggregate pattern verbatim). Superset of the two
//!   above in cost — the FFI change *and* the AUHAL read — and the
//!   cross-check can only turn a start that would work into a refusal
//!   (a disagreement between two readings of one device is a Core Audio
//!   fault, not a layout the app can act on). Rejected.
//! - **Read `kAudioDevicePropertyStreamConfiguration` on the device ID
//!   from Rust** (the property the Swift catalog uses). Same result as
//!   the chosen option with a second, larger block of unsafe property
//!   code (a variable-size `AudioBufferList`), where the AUHAL read is a
//!   fixed-size struct through a helper that already exists. Rejected.
//! - **Set no client format and take AUHAL's default.** The default
//!   stream format of an audio unit is non-interleaved 32-bit float
//!   (Audio Unit Programming Guide, "Commonly Used Properties") — one
//!   buffer per channel — while the output callback writes a single
//!   interleaved buffer; it would silence every channel but the first,
//!   and AUHAL's default channel count on a freshly bound device is not
//!   documented. Rejected.
//! - **Keep a mono client format and let AUHAL fan out.** AUHAL does
//!   not: the identity map sends client channel 0 to device channel 0
//!   and leaves the rest silent — the left-ear-only result PR #26
//!   measured on the aggregate path. Rejected.
//!
//! No candidate dominating the chosen one was found: every alternative
//! either adds the FFI change, adds unsafe code, or adds an unverified
//! AUHAL behaviour without removing the one read the chosen option
//! makes.

use crate::CoreAudioError;

/// AUHAL channel map entry meaning "no client channel feeds this device
/// channel" (the device channel renders silence).
pub const UNMAPPED_CHANNEL: i32 = -1;

/// Position of the virtual output's channels within an Aggregate Device's
/// output channel list.
///
/// `first` is the number of output channels contributed by the subdevices
/// ahead of the virtual output (0 for a microphone without outputs, 2 for
/// a microphone with a stereo headphone jack), and `count` is the virtual
/// output's own channel count — which is also the channel count of the
/// client stream the transport renders. Constructed by the control plane
/// from the subdevice list it composes; see [`render_channel_map`] for
/// how it is checked against the device.
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

/// Builds the AUHAL output channel map that places a `target.count()`-
/// channel client stream on `target` in a device with `device_channels`
/// output channels.
///
/// The map holds one `i32` per device channel: the client channel that
/// feeds it, or [`UNMAPPED_CHANNEL`]. Virtual output channel `i`
/// receives client channel `i` (one-to-one — the render callback puts
/// the same engine sample into every client channel, see the module
/// docs for why the duplication is not expressed in the map), and every
/// channel ahead of the virtual output is silent.
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
    // plain indexing rather than a silent `get_mut`, because an all-`-1`
    // map would be exactly the silent virtual microphone this module
    // prevents.
    for (client_channel, entry) in (0_i32..).zip(&mut map[first..]) {
        *entry = client_channel;
    }
    Ok(map)
}

/// Client render channel count for the split transport's output-only
/// AUHAL, from the output channel count AUHAL reports for the virtual
/// output device it is bound to.
///
/// One client channel per device channel (dual mono — the callback
/// writes the engine sample into every one), so AUHAL's identity map
/// feeds the whole device whatever its width (see the module docs, "The
/// split transport's render format").
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
        // Stock BlackHole 2ch and the 0.1.0 Noican driver: stereo client
        // stream, as the old constant produced.
        assert_eq!(split_render_channels(2).expect("stereo"), 2);
        // The 1-channel Noican driver (0.2.0 on): mono client stream.
        assert_eq!(split_render_channels(1).expect("mono"), 1);
        // Wider loopbacks are served the same way (BlackHole 16ch).
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
        // No microphone outputs, 2-channel virtual output (stock BlackHole
        // 2ch, the 0.1.0 driver): the virtual output is channels 0..2 and
        // the two-channel client stream lands on it one-to-one.
        let target = VirtualOutputChannels::new(0, 2).expect("valid range");
        assert_eq!(render_channel_map(2, target).expect("map"), vec![0, 1]);
    }

    #[test]
    fn headphone_microphone_layout_skips_the_microphone_outputs() {
        // Shure MV7+: stereo headphone jack first, then the virtual output.
        let target = VirtualOutputChannels::new(2, 2).expect("valid range");
        assert_eq!(
            render_channel_map(4, target).expect("map"),
            vec![UNMAPPED_CHANNEL, UNMAPPED_CHANNEL, 0, 1]
        );
    }

    #[test]
    fn multichannel_interface_layout_is_handled() {
        // An 8-output interface ahead of a stereo virtual output.
        let target = VirtualOutputChannels::new(8, 2).expect("valid range");
        let map = render_channel_map(10, target).expect("map");
        assert_eq!(map.len(), 10);
        assert!(map[..8].iter().all(|&entry| entry == UNMAPPED_CHANNEL));
        assert_eq!(&map[8..], &[0, 1]);
    }

    #[test]
    fn single_channel_virtual_output_keeps_the_mono_shape() {
        // The 1-channel Noican driver (0.2.0 on): mono client stream, map
        // ends in a single 0 — the shape the pre-dual-mono release had on
        // a 2-channel device. Built-in microphone: [0]; MV7i (stereo
        // headphone jack ahead of the virtual output): [-1, -1, 0].
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
            // No client channel is named twice: the map stays one-to-one.
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
