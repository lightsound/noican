import Testing

@testable import NoicanState

// Fixtures and drivers are shared with the other reducer suites; see
// ReducerTestSupport.swift.

// MARK: - Capture-capability classification

@Suite("Capture-support classification")
struct CaptureSupportTests {
    @Test("48 kHz-capable devices take the aggregate path regardless of idle rate")
    func engineRate() {
        #expect(CaptureSupport.classify(supports48kHz: true, nominalRate: 44_100) == .engineRate)
        #expect(CaptureSupport.classify(supports48kHz: true, nominalRate: 48_000) == .engineRate)
        #expect(CaptureSupport.classify(supports48kHz: true, nominalRate: 16_000) == .engineRate)
    }

    @Test("Telephony-profile rates are captured natively")
    func nativeRates() {
        for hertz in [8_000, 12_000, 16_000, 24_000] {
            #expect(
                CaptureSupport.classify(supports48kHz: false, nominalRate: Double(hertz))
                    == .nativeRate(hertz: hertz),
                "\(hertz) Hz must be a native-capture rate"
            )
        }
    }

    @Test("Rates that no integer factor reaches 48 kHz from are unsupported")
    func unsupportedRates() {
        #expect(
            CaptureSupport.classify(supports48kHz: false, nominalRate: 44_100)
                == .unsupported(hertz: 44_100)
        )
        #expect(
            CaptureSupport.classify(supports48kHz: false, nominalRate: 32_000)
                == .unsupported(hertz: 32_000)
        )
        #expect(
            CaptureSupport.classify(supports48kHz: false, nominalRate: 0)
                == .unsupported(hertz: 0)
        )
        #expect(
            CaptureSupport.classify(supports48kHz: false, nominalRate: -1)
                == .unsupported(hertz: 0)
        )
        // A device idling exactly at 48 kHz without advertising it in
        // its rate list cannot be composed nor resampled (factor 1).
        #expect(
            CaptureSupport.classify(supports48kHz: false, nominalRate: 48_000)
                == .unsupported(hertz: 48_000)
        )
    }

    @Test("Rate labels annotate only non-48 kHz devices")
    func rateLabels() {
        #expect(builtInMic.rateLabel == nil)
        #expect(bluetoothMic.rateLabel == "16 kHz")
        #expect(unsupportedMic.rateLabel == "44100 Hz")
    }
}

// MARK: - Native-rate selection semantics

@Suite("Native-rate microphone selection")
struct NativeRateSelectionTests {
    @Test("A Bluetooth telephony microphone starts the engine (no refusal)")
    func bluetoothStarts() {
        let (state, effects) = step(readyModel(selectedInputUID: bluetoothMic.uid), tap(.on))
        let attempt = StartAttempt(
            modelID: "fastenhancer-b", inputUID: bluetoothMic.uid, monitor: false
        )
        #expect(state.isBusy, "the selection pre-flights clean and claims a start")
        #expect(state.engineErrorMessage == nil)
        #expect(effects == [.stopEngine, .startEngine(attempt)])

        let running = drive(state, [.startCompleted(error: nil)])
        #expect(running.statusText == "Running")
        #expect(running.liveRunningSession?.inputUID == bluetoothMic.uid)
    }

    @Test("A live switch to a Bluetooth microphone rebuilds with a revert target")
    func liveSwitchToBluetooth() {
        let (state, effects) = step(runningModel(), .microphoneSelected(bluetoothMic.uid))
        let attempt = StartAttempt(
            modelID: "fastenhancer-b",
            inputUID: bluetoothMic.uid,
            monitor: false,
            revertInputUID: builtInMic.uid
        )
        #expect(effects == [.stopEngine, .startEngine(attempt)])
        #expect(state.messages.microphoneError == nil)
    }

    @Test("The telephony notice renders for the selected Bluetooth microphone only")
    func telephonyNotice() {
        let bluetooth = readyModel(selectedInputUID: bluetoothMic.uid)
        #expect(bluetooth.microphoneNotice?.contains("16 kHz") == true)
        #expect(bluetooth.microphoneNotice?.contains("playback") == true)

        #expect(readyModel().microphoneNotice == nil, "48 kHz devices need no notice")
        #expect(
            readyModel(selectedInputUID: unsupportedMic.uid).microphoneNotice == nil,
            "unsupported devices are refused via capabilityError, not a notice"
        )
    }
}

// MARK: - Nominal-rate change while running

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
            modelID: "fastenhancer-b", inputUID: bluetoothMic.uid, monitor: false
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

    @Test("Rate changes are ignored while off, busy, or without a live session")
    func rateChangeIgnoredOutsideLiveSessions() {
        // Off: nothing to rebuild.
        let off = readyModel()
        let (offState, offEffects) = step(off, .inputSampleRateChanged)
        #expect(offState == off)
        #expect(offEffects.isEmpty)

        // Busy: the in-flight transition owns the transport.
        let busy = drive(readyModel(), [tap(.on)])
        let (busyState, busyEffects) = step(busy, .inputSampleRateChanged)
        #expect(busyState == busy)
        #expect(busyEffects.isEmpty)

        // Failed with the transport torn down: nothing to rebuild (the
        // next user action does).
        let failed = drive(readyModel(), [tap(.on), .startCompleted(error: "mic exploded")])
        let (failedState, failedEffects) = step(failed, .inputSampleRateChanged)
        #expect(failedState == failed)
        #expect(failedEffects.isEmpty)
    }
}
