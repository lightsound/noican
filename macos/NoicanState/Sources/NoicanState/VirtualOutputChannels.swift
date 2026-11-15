public struct VirtualOutputChannels: Hashable, Sendable {
    public let firstChannel: UInt32
    public let channelCount: UInt32

    public init(outputChannelsAhead: [UInt32], virtualOutputChannels: UInt32) {
        firstChannel = outputChannelsAhead.reduce(UInt32(0), +)
        channelCount = virtualOutputChannels
    }
}
