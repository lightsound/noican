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
///
/// Pure arithmetic, kept in this package so it is unit-tested on every
/// CI target; the app's `AggregateDevice` feeds it the channel counts of
/// the very subdevice list it composes.
public struct VirtualOutputChannels: Hashable, Sendable {
    /// Output channels ahead of the virtual output (the microphone's own
    /// output channel count; 0 for the built-in microphone).
    public let firstChannel: UInt32
    /// The virtual output's own output channel count.
    public let channelCount: UInt32

    /// Locates a virtual output with `virtualOutputChannels` output
    /// channels behind the subdevices whose output channel counts are
    /// `outputChannelsAhead`, in composition order.
    public init(outputChannelsAhead: [UInt32], virtualOutputChannels: UInt32) {
        firstChannel = outputChannelsAhead.reduce(UInt32(0), +)
        channelCount = virtualOutputChannels
    }
}
