import CoreAudio
import Foundation

/// Where the virtual output's channels sit in the private aggregate's
/// output channel list.
///
/// An Aggregate Device exposes its subdevices' output channels
/// concatenated in composition order. Noican composes
/// `[microphone, virtual output]` (the microphone must stay first: it is
/// the clock master, and the capture side relies on aggregate input
/// channel 0 being the microphone), so a microphone with its own output
/// channels — a USB microphone with a headphone jack such as the Shure
/// MV7+, or an audio interface — pushes the virtual output back by that
/// many channels. The Rust transport needs this position to route the
/// engine output to the virtual output instead of AUHAL's default
/// (device channel 0, which on such a device is the microphone's own
/// headphone output — recordings from the virtual microphone were silent
/// while the preview kept working). Mirrors `noican_engine_start` in
/// noican.h; the Rust side re-checks the range against the channel count
/// the aggregate actually reports.
struct VirtualOutputChannels: Equatable {
    /// Output channels ahead of the virtual output (the microphone's own
    /// output channel count; 0 for the built-in microphone).
    let firstChannel: UInt32
    /// The virtual output's own output channel count.
    let channelCount: UInt32

    /// Locates `virtualOutput` within `subdevices`, the aggregate's
    /// composition order: every subdevice ahead of it contributes its
    /// output channels to the offset. Returns nil when the virtual output
    /// is not part of the list.
    static func locate(
        virtualOutput: AudioDeviceInfo,
        in subdevices: [AudioDeviceInfo]
    ) -> VirtualOutputChannels? {
        guard let index = subdevices.firstIndex(where: { $0.uid == virtualOutput.uid }) else {
            return nil
        }
        let ahead = subdevices[..<index].reduce(UInt32(0)) { total, device in
            total + device.outputChannels
        }
        return VirtualOutputChannels(
            firstChannel: ahead,
            channelCount: virtualOutput.outputChannels
        )
    }
}

/// A created private aggregate together with the output layout the Rust
/// transport must route into.
struct AggregateComposition {
    let deviceID: AudioObjectID
    let virtualOutputChannels: VirtualOutputChannels
}
