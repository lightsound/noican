import Testing

@testable import NoicanState

// The Rust side (`noican_coreaudio::routing`) tests the channel map built
// from these numbers; this suite covers the arithmetic that produces them
// from the composed subdevice list — the only number in the chain that
// depends on real device data.

@Suite("Virtual output position inside the aggregate")
struct VirtualOutputChannelsTests {
    @Test("A microphone without outputs leaves the virtual output at channel 0")
    func builtInMicrophone() {
        let layout = VirtualOutputChannels(outputChannelsAhead: [0], virtualOutputChannels: 2)
        #expect(layout.firstChannel == 0)
        #expect(layout.channelCount == 2)
    }

    @Test("A headphone-equipped microphone pushes the virtual output back by its outputs")
    func headphoneJackMicrophone() {
        // Shure MV7+: stereo headphone jack ahead of the virtual output.
        let layout = VirtualOutputChannels(outputChannelsAhead: [2], virtualOutputChannels: 2)
        #expect(layout.firstChannel == 2)
        #expect(layout.channelCount == 2)
    }

    @Test("Every subdevice ahead of the virtual output contributes its outputs")
    func multiOutputInterface() {
        // An 8-output interface, plus a hypothetical second output-less
        // subdevice, ahead of a stereo virtual output.
        let layout = VirtualOutputChannels(outputChannelsAhead: [8, 0], virtualOutputChannels: 2)
        #expect(layout.firstChannel == 8)
        #expect(layout.channelCount == 2)
    }

    @Test("The virtual output's own channel count is passed through untouched")
    func channelCountPassThrough() {
        let layout = VirtualOutputChannels(outputChannelsAhead: [], virtualOutputChannels: 0)
        #expect(layout.firstChannel == 0)
        // Zero is reported as-is; the Rust side refuses an empty range
        // with a precise message rather than this type guessing.
        #expect(layout.channelCount == 0)
    }
}
