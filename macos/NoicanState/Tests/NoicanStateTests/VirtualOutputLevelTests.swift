import Testing

@testable import NoicanState

// Fixtures and drivers are shared with the other reducer suites; see
// ReducerTestSupport.swift.

// MARK: - Classification (pure)

@Suite("Virtual output level classification")
struct VirtualOutputLevelClassificationTests {
    @Test("Unity volume without mute is nominal")
    func nominal() {
        #expect(VirtualOutputLevel.classify(volumeScalar: 1.0, isMuted: false) == .nominal)
        #expect(VirtualOutputLevel.classify(volumeScalar: 1.0, isMuted: nil) == .nominal)
    }

    @Test("A slider parked at the top may read a hair below 1.0 and is still nominal")
    func nearUnityTolerance() {
        #expect(VirtualOutputLevel.classify(volumeScalar: 0.9995, isMuted: false) == .nominal)
        #expect(VirtualOutputLevel.classify(volumeScalar: 0.999, isMuted: false) == .nominal)
    }

    @Test("A volume below unity is reported with the scalar reading")
    func turnedDown() {
        #expect(VirtualOutputLevel.classify(volumeScalar: 0.5, isMuted: false) == .turnedDown(scalar: 0.5))
        #expect(VirtualOutputLevel.classify(volumeScalar: 0.99, isMuted: nil) == .turnedDown(scalar: 0.99))
        #expect(VirtualOutputLevel.classify(volumeScalar: 0, isMuted: false) == .turnedDown(scalar: 0))
    }

    @Test("Mute outranks a low volume")
    func mutedWins() {
        #expect(VirtualOutputLevel.classify(volumeScalar: 0.5, isMuted: true) == .muted)
        #expect(VirtualOutputLevel.classify(volumeScalar: 1.0, isMuted: true) == .muted)
        #expect(VirtualOutputLevel.classify(volumeScalar: nil, isMuted: true) == .muted)
    }

    @Test("Absent or unreadable controls never produce a finding (fail open)")
    func failOpen() {
        #expect(VirtualOutputLevel.classify(volumeScalar: nil, isMuted: nil) == .nominal)
        #expect(VirtualOutputLevel.classify(volumeScalar: nil, isMuted: false) == .nominal)
        #expect(VirtualOutputLevel.classify(volumeScalar: .nan, isMuted: false) == .nominal)
    }

    @Test("Notices are one line each and name the place to fix it")
    func notices() {
        #expect(VirtualOutputLevel.nominal.notice == nil)
        let turnedDown = VirtualOutputLevel.turnedDown(scalar: 0.3).notice
        #expect(turnedDown?.contains("System Settings › Sound › Input") == true)
        #expect(turnedDown?.contains("\n") == false)
        let muted = VirtualOutputLevel.muted.notice
        #expect(muted?.contains("muted") == true)
        #expect(muted?.contains("System Settings › Sound › Input") == true)
        #expect(muted != turnedDown)
    }
}

// MARK: - Reducer transitions

@Suite("Virtual output level notice")
struct VirtualOutputLevelReducerTests {
    @Test("A turned-down reading shows the notice; a nominal reading clears it")
    func showAndClear() {
        let running = runningModel()
        let (shown, effects) = step(running, .virtualOutputLevelObserved(.turnedDown(scalar: 0.18)))
        #expect(shown.messages.virtualOutputLevelNotice == VirtualOutputLevel.turnedDown(scalar: 0.18).notice)
        // Only the message moves: no engine transition, no effect.
        #expect(effects.isEmpty, "the level is never restored automatically")
        #expect(shown.machine == running.machine)
        #expect(shown.statusText == "Running")
        #expect(!shown.isModeUnfulfilled)

        let cleared = drive(shown, [.virtualOutputLevelObserved(.nominal)])
        #expect(cleared.messages.virtualOutputLevelNotice == nil)
        #expect(cleared.machine == running.machine)
    }

    @Test("Mute replaces the turned-down notice and clears the same way")
    func mutedNotice() {
        let muted = drive(runningModel(), [
            .virtualOutputLevelObserved(.turnedDown(scalar: 0.5)),
            .virtualOutputLevelObserved(.muted)
        ])
        #expect(muted.messages.virtualOutputLevelNotice == VirtualOutputLevel.muted.notice)
        let cleared = drive(muted, [.virtualOutputLevelObserved(.nominal)])
        #expect(cleared.messages.virtualOutputLevelNotice == nil)
    }

    @Test("Readings are accepted through busy monitor and model transitions")
    func acceptedWhileTransportBusy() {
        // Preview start: the transport is up while the monitor half is
        // still settling, and the start-completion check lands here.
        let armingMonitor = drive(readyModel(), [tap(.preview), .startCompleted(error: nil)])
        #expect(armingMonitor.isBusy)
        let noticed = drive(armingMonitor, [.virtualOutputLevelObserved(.muted)])
        #expect(noticed.messages.virtualOutputLevelNotice == VirtualOutputLevel.muted.notice)
        #expect(noticed.machine == armingMonitor.machine)

        let switching = drive(runningModel(), [.modelSelected("dfn3")])
        #expect(switching.isBusy)
        let noticedSwitching = drive(switching, [.virtualOutputLevelObserved(.turnedDown(scalar: 0.4))])
        #expect(noticedSwitching.messages.virtualOutputLevelNotice != nil)
    }

    @Test("Readings are ignored without a live transport")
    func ignoredWithoutTransport() {
        for model in [readyModel(), drive(readyModel(), [tap(.on)])] {
            let (state, effects) = step(model, .virtualOutputLevelObserved(.muted))
            #expect(state == model)
            #expect(effects.isEmpty)
        }
    }

    @Test("Any engine teardown clears the notice")
    func clearedOnTeardown() {
        let shown = drive(runningModel(), [.virtualOutputLevelObserved(.turnedDown(scalar: 0.2))])
        #expect(shown.messages.virtualOutputLevelNotice != nil)

        let off = drive(shown, [tap(.off, isEngineRunning: true)])
        #expect(off.messages.virtualOutputLevelNotice == nil)

        let stalled = drive(shown, [.audioStalled])
        #expect(stalled.messages.virtualOutputLevelNotice == nil)

        // A live microphone switch rebuilds the transport; the new one
        // is re-read once it is up.
        let rebuilding = drive(shown, [.microphoneSelected(usbMic.uid)])
        #expect(rebuilding.isBusy)
        #expect(rebuilding.messages.virtualOutputLevelNotice == nil)
    }

    @Test("The notice survives an engine fault while the transport stays up")
    func keptThroughFault() {
        let faulted = drive(runningModel(), [
            .virtualOutputLevelObserved(.muted),
            .engineFaulted
        ])
        #expect(faulted.messages.virtualOutputLevelNotice == VirtualOutputLevel.muted.notice)
        // The poll keeps running on the faulted-but-live transport, so a
        // nominal reading still clears it.
        let cleared = drive(faulted, [.virtualOutputLevelObserved(.nominal)])
        #expect(cleared.messages.virtualOutputLevelNotice == nil)
    }
}
