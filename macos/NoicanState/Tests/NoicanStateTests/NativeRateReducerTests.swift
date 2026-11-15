import Testing

@testable import NoicanState

@Suite("Capture-support classification")
struct CaptureSupportTests {
    @Test("48 kHz-capable devices take the aggregate path regardless of idle rate")
    func engineRate() {
        #expect(CaptureSupport.classify(supports48kHz: true, nominalRate: 44_100) == .engineRate)
        #expect(CaptureSupport.classify(supports48kHz: true, nominalRate: 48_000) == .engineRate)
        #expect(CaptureSupport.classify(supports48kHz: true, nominalRate: 16_000) == .engineRate)
    }

    @Test("Every rate the resampler converts is captured natively")
    func nativeRates() {
        let rates = [8_000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 88_200]
        for hertz in rates + [96_000, 192_000] {
            #expect(
                CaptureSupport.classify(supports48kHz: false, nominalRate: Double(hertz))
                    == .nativeRate(hertz: hertz),
                "\(hertz) Hz must be a native-capture rate"
            )
        }
        #expect(
            CaptureSupport.classify(supports48kHz: false, nominalRate: 48_000)
                == .nativeRate(hertz: 48_000)
        )
    }

    @Test("Rates outside the resampler's range are unsupported")
    func unsupportedRates() {
        #expect(
            CaptureSupport.classify(supports48kHz: false, nominalRate: 7_999)
                == .unsupported(hertz: 7_999)
        )
        #expect(
            CaptureSupport.classify(supports48kHz: false, nominalRate: 192_001)
                == .unsupported(hertz: 192_001)
        )
        #expect(
            CaptureSupport.classify(supports48kHz: false, nominalRate: 0)
                == .unsupported(hertz: 0)
        )
        #expect(
            CaptureSupport.classify(supports48kHz: false, nominalRate: -1)
                == .unsupported(hertz: 0)
        )
    }

    @Test("Rate labels annotate only non-48 kHz devices, in audio notation")
    func rateLabels() {
        #expect(builtInMic.rateLabel == nil)
        #expect(bluetoothMic.rateLabel == "16 kHz")
        #expect(fortyFourMic.rateLabel == "44.1 kHz")
        #expect(InputDevice.rateLabel(hertz: 22_050) == "22.05 kHz")
        #expect(InputDevice.rateLabel(hertz: 11_025) == "11.025 kHz")
        #expect(InputDevice.rateLabel(hertz: 96_000) == "96 kHz")
        #expect(unsupportedMic.rateLabel == "0 Hz")
    }

    @Test("Only telephony rates are narrow-band; the 44.1 kHz family never is")
    func telephonyRates() {
        for hertz in [8_000, 12_000, 16_000, 24_000, 32_000] {
            #expect(CaptureSupport.isTelephonyRate(hertz))
        }
        for hertz in [11_025, 22_050, 44_100, 48_000, 96_000] {
            #expect(!CaptureSupport.isTelephonyRate(hertz), "\(hertz) Hz is not telephony")
        }
    }
}

@Suite("Native-rate microphone selection")
struct NativeRateSelectionTests {
    @Test("A Bluetooth telephony microphone starts the engine (no refusal)")
    func bluetoothStarts() {
        let (state, effects) = step(readyModel(selectedInputUID: bluetoothMic.uid), tap(.on))
        let attempt = StartAttempt(
            modelID: "dfn3", inputUID: bluetoothMic.uid, monitor: false
        )
        #expect(state.isBusy, "the selection pre-flights clean and claims a start")
        #expect(state.engineErrorMessage == nil)
        #expect(effects == [.stopEngine, .startEngine(attempt)])

        let running = drive(state, [.startCompleted(error: nil)])
        #expect(running.statusText == "Running")
        #expect(running.liveRunningSession?.inputUID == bluetoothMic.uid)
    }

    @Test("A 44.1 kHz-only microphone starts the engine (no refusal)")
    func fortyFourStarts() {
        let (state, effects) = step(readyModel(selectedInputUID: fortyFourMic.uid), tap(.on))
        let attempt = StartAttempt(
            modelID: "dfn3", inputUID: fortyFourMic.uid, monitor: false
        )
        #expect(state.isBusy, "the 44.1 kHz device pre-flights clean and claims a start")
        #expect(state.engineErrorMessage == nil)
        #expect(effects == [.stopEngine, .startEngine(attempt)])

        let running = drive(state, [.startCompleted(error: nil)])
        #expect(running.statusText == "Running")
        #expect(running.liveRunningSession?.inputUID == fortyFourMic.uid)
    }

    @Test("A live switch to a Bluetooth microphone rebuilds with a revert target")
    func liveSwitchToBluetooth() {
        let (state, effects) = step(runningModel(), .microphoneSelected(bluetoothMic.uid))
        let attempt = StartAttempt(
            modelID: "dfn3",
            inputUID: bluetoothMic.uid,
            monitor: false,
            revertInputUID: builtInMic.uid
        )
        #expect(effects == [.stopEngine, .startEngine(attempt)])
        #expect(state.messages.microphoneError == nil)
    }

    @Test("The notice matches the selected native-rate microphone's kind")
    func telephonyNotice() {
        let bluetooth = readyModel(selectedInputUID: bluetoothMic.uid)
        #expect(bluetooth.microphoneNotice?.contains("16 kHz") == true)
        #expect(bluetooth.microphoneNotice?.contains("playback") == true)

        let fortyFour = readyModel(selectedInputUID: fortyFourMic.uid)
        #expect(fortyFour.microphoneNotice?.contains("44.1 kHz") == true)
        #expect(fortyFour.microphoneNotice?.contains("48 kHz") == true)
        #expect(fortyFour.microphoneNotice?.contains("narrow-band") == false)

        #expect(readyModel().microphoneNotice == nil, "48 kHz devices need no notice")
        #expect(
            readyModel(selectedInputUID: unsupportedMic.uid).microphoneNotice == nil,
            "unsupported devices are refused via capabilityError, not a notice"
        )
    }
}

@Suite("Input sample-rate changes")
struct InputRateChangeTests {
    @Test("A rate change under a running transport rebuilds into the same mode")
    func rateChangeRebuilds() {
        let running = drive(
            readyModel(selectedInputUID: bluetoothMic.uid),
            [tap(.on), .startCompleted(error: nil)]
        )
        let (state, effects) = step(running, .inputSampleRateChanged)
        let attempt = StartAttempt(
            modelID: "dfn3", inputUID: bluetoothMic.uid, monitor: false
        )
        #expect(effects == [.stopEngine, .startEngine(attempt)])
        #expect(state.isBusy)
        #expect(state.mode == .on, "intent is kept across the rebuild")

        let rebuilt = drive(state, [.startCompleted(error: nil)])
        #expect(rebuilt.statusText == "Running")
    }

    @Test("A rate change while previewing keeps the monitor intent")
    func rateChangeKeepsPreview() {
        let previewing = runningModel(mode: .preview)
        let (state, _) = step(previewing, .inputSampleRateChanged)
        guard case let .busy(.starting(attempt), _) = state.machine else {
            Issue.record("expected a rebuild, got \(state.machine)")
            return
        }
        #expect(attempt.monitor, "the rebuild re-arms the preview monitor")
    }

    @Test("The transport session survives busy monitor/model transitions")
    func transportSessionSurvivesBusyTransitions() {
        let togglingMonitor = drive(runningModel(), [tap(.preview, isEngineRunning: true)])
        #expect(togglingMonitor.isBusy)
        #expect(togglingMonitor.liveSession == nil, "busy transitions own their session")
        #expect(togglingMonitor.transportSession?.inputUID == builtInMic.uid)

        let switchingModel = drive(runningModel(), [.modelSelected("dpdfnet2")])
        #expect(switchingModel.isBusy)
        #expect(switchingModel.transportSession?.inputUID == builtInMic.uid)

        let starting = drive(readyModel(), [tap(.on)])
        #expect(starting.transportSession == nil)
        #expect(readyModel().transportSession == nil)
    }

    @Test("Rate changes are ignored while off, busy, or without a live session")
    func rateChangeIgnoredOutsideLiveSessions() {
        let off = readyModel()
        let (offState, offEffects) = step(off, .inputSampleRateChanged)
        #expect(offState == off)
        #expect(offEffects.isEmpty)

        let busy = drive(readyModel(), [tap(.on)])
        let (busyState, busyEffects) = step(busy, .inputSampleRateChanged)
        #expect(busyState == busy)
        #expect(busyEffects.isEmpty)

        let failed = drive(readyModel(), [tap(.on), .startCompleted(error: "mic exploded")])
        let (failedState, failedEffects) = step(failed, .inputSampleRateChanged)
        #expect(failedState == failed)
        #expect(failedEffects.isEmpty)
    }
}
