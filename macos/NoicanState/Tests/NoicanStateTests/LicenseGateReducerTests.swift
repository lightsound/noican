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

    @Test("Pickers keep working without a license")
    func pickersWork() {
        let state = drive(unlicensed(), [.microphoneSelected(usbMic.uid), .modelSelected("dfn3"), .intensityChanged(0.5)])
        #expect(state.selectedInputUID == usbMic.uid)
        #expect(state.selectedModelID == "dfn3")
        #expect(state.intensity == 0.5)
    }
}
