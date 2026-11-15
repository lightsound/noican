import Testing

@testable import NoicanState

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
        let layout = VirtualOutputChannels(outputChannelsAhead: [2], virtualOutputChannels: 2)
        #expect(layout.firstChannel == 2)
        #expect(layout.channelCount == 2)
    }

    @Test("Every subdevice ahead of the virtual output contributes its outputs")
    func multiOutputInterface() {
        let layout = VirtualOutputChannels(outputChannelsAhead: [8, 0], virtualOutputChannels: 2)
        #expect(layout.firstChannel == 8)
        #expect(layout.channelCount == 2)
    }

    @Test("The virtual output's own channel count is passed through untouched")
    func channelCountPassThrough() {
        let layout = VirtualOutputChannels(outputChannelsAhead: [], virtualOutputChannels: 0)
        #expect(layout.firstChannel == 0)
        #expect(layout.channelCount == 0)
    }
}
