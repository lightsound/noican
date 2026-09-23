import Testing

@testable import NoicanState

// Reducer coverage for the license gate: without a valid license, Preview
// and On taps are refused in place with an explanation, while Off, the
// pickers, and any session already running are untouched.

@Suite("License gate")
struct LicenseGateReducerTests {
    private func unlicensed(_ model: AppModel = readyModel()) -> AppModel {
        drive(model, [.processingAllowanceChanged(false)])
    }

    @Test("Processing is allowed by default, so an unconfigured build behaves as before")
    func allowedByDefault() {
        #expect(readyModel().isProcessingAllowed)
    }

    @Test("Without a license, On and Preview are refused in place with the reason")
    func tapsRefused() {
        for mode in [EngineMode.on, .preview] {
            let (state, effects) = step(unlicensed(), tap(mode))
            #expect(state.mode == .off, "the mode control does not move")
            #expect(state.machine == .settled(.off), "no engine transition is claimed")
            #expect(effects.isEmpty)
            #expect(state.messages.licenseRequired == AppReducer.licenseRequiredMessage)
            #expect(!state.isModeUnfulfilled, "a refusal is not a failed session")
        }
    }

    @Test("Off always works and clears the refusal")
    func offWorks() {
        let refused = drive(runningModel(), [.processingAllowanceChanged(false), tap(.preview)])
        #expect(refused.messages.licenseRequired != nil)
        let (state, effects) = step(refused, tap(.off))
        #expect(state.mode == .off)
        #expect(effects == [.stopEngine])
        #expect(state.messages.licenseRequired == nil)
    }

    @Test("A license arriving clears the refusal and the next tap starts the engine")
    func licenseUnblocks() {
        let refused = drive(unlicensed(), [tap(.on)])
        let allowed = drive(refused, [.processingAllowanceChanged(true)])
        #expect(allowed.isProcessingAllowed)
        #expect(allowed.messages.licenseRequired == nil)
        let (state, effects) = step(allowed, tap(.on))
        #expect(state.mode == .on)
        #expect(effects.contains { if case .startEngine = $0 { true } else { false } })
    }

    @Test("A license lapsing mid-session never stops the running engine")
    func runningSessionUntouched() {
        let running = runningModel()
        let (state, effects) = step(running, .processingAllowanceChanged(false))
        #expect(effects.isEmpty)
        #expect(state.machine == running.machine)
        #expect(state.mode == .on)
        // Automatic rebuilds of that session keep working.
        let (rebuilt, rebuildEffects) = step(state, .microphoneSelected(usbMic.uid))
        #expect(rebuildEffects.contains { if case .startEngine = $0 { true } else { false } })
        #expect(rebuilt.messages.licenseRequired == nil)
    }

    @Test("A session that died is not restarted by a microphone pick without a license")
    func deadSessionNotRestarted() {
        let died = drive(runningModel(), [.processingAllowanceChanged(false), .audioStalled])
        #expect(died.mode == .on)
        #expect(died.liveSession == nil, "the runtime stop tore the transport down")
        let (state, effects) = step(died, .microphoneSelected(usbMic.uid))
        #expect(effects.isEmpty, "no new engine start")
        #expect(state.selectedInputUID == usbMic.uid, "the pick itself is kept")
        #expect(state.messages.licenseRequired == AppReducer.licenseRequiredMessage)
        #expect(state.machine == died.machine)
    }

    @Test("A failed live microphone switch still falls back to the working device without a license")
    func liveSwitchFallbackAllowed() {
        let switching = drive(runningModel(), [.processingAllowanceChanged(false), .microphoneSelected(usbMic.uid)])
        let (state, effects) = step(switching, .startCompleted(error: "USB device busy"))
        #expect(state.selectedInputUID == builtInMic.uid)
        #expect(effects.contains { if case .startEngine = $0 { true } else { false } })
        #expect(state.messages.licenseRequired == nil)
    }

    @Test("A rate-change rebuild of a live session passes without a license")
    func rateRebuildAllowed() {
        let running = drive(runningModel(), [.processingAllowanceChanged(false)])
        let (_, effects) = step(running, .inputSampleRateChanged)
        #expect(effects.contains { if case .startEngine = $0 { true } else { false } })
    }

    @Test("Pickers keep working without a license")
    func pickersWork() {
        let state = drive(unlicensed(), [.microphoneSelected(usbMic.uid), .modelSelected("dpdfnet2"), .intensityChanged(0.5)])
        #expect(state.selectedInputUID == usbMic.uid)
        #expect(state.selectedModelID == "dpdfnet2")
        #expect(state.intensity == 0.5)
    }
}
