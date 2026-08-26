import Testing

@testable import NoicanState

// MARK: - Fixtures

private let builtInMic = InputDevice(uid: "builtin", name: "MacBook Pro Microphone", supports48kHz: true)
private let usbMic = InputDevice(uid: "usb", name: "USB Microphone", supports48kHz: true)
private let bluetoothMic = InputDevice(uid: "bt", name: "AirPods", supports48kHz: false)

/// A ready-to-start model: engine available, devices present, defaults
/// selected — the state right after launch on a healthy machine.
private func readyModel(
    devices: [InputDevice] = [builtInMic, usbMic, bluetoothMic],
    selectedInputUID: String = builtInMic.uid,
    selectedModelID: String = "fastenhancer-b"
) -> AppModel {
    AppModel(
        selectedModelID: selectedModelID,
        selectedInputUID: selectedInputUID,
        inputDevices: devices,
        isVirtualOutputPresent: true
    )
}

/// Drives `model` through `events`, discarding intermediate effects.
private func drive(_ model: AppModel, _ events: [AppEvent]) -> AppModel {
    events.reduce(model) { state, event in
        AppReducer.reduce(state, event).state
    }
}

/// One reduce step, returning both halves for assertions.
private func step(_ model: AppModel, _ event: AppEvent) -> (state: AppModel, effects: [AppEffect]) {
    AppReducer.reduce(model, event)
}

/// A user tap on the mode control under a healthy environment (safe
/// preview target; live engine liveness mirrors the machine).
private func tap(_ mode: EngineMode, monitorTargetError: String? = nil, isEngineRunning: Bool? = nil) -> AppEvent {
    .modeSelected(
        mode,
        monitorTargetError: monitorTargetError,
        isEngineRunning: isEngineRunning ?? false
    )
}

/// A running state reached through the real start transition.
private func runningModel(mode: EngineMode = .on) -> AppModel {
    let monitorEvents: [AppEvent] = mode == .preview ? [.monitorChangeCompleted(error: nil)] : []
    return drive(
        readyModel(),
        [tap(mode), .startCompleted(error: nil)] + monitorEvents
    )
}

// MARK: - Start / stop

@Suite("Start and stop transitions")
struct StartStopTests {
    @Test("Off → On claims a start behind the busy machine")
    func startClaim() {
        let (state, effects) = step(readyModel(), tap(.on))
        let attempt = StartAttempt(
            modelID: "fastenhancer-b", inputUID: builtInMic.uid, monitor: false
        )
        #expect(state.mode == .on)
        #expect(state.machine == .busy(.starting(attempt), rendering: .off))
        #expect(state.isBusy)
        #expect(effects == [.stopEngine, .startEngine(attempt)])
        // Settled rendering: nothing the UI projects has moved yet.
        #expect(state.statusText == "Off")
        #expect(!state.showsMonitoring)
    }

    @Test("A successful start settles into Running")
    func startSuccess() {
        let state = drive(readyModel(), [tap(.on), .startCompleted(error: nil)])
        #expect(state.machine == .settled(.running(
            EngineSession(modelID: "fastenhancer-b", inputUID: builtInMic.uid)
        )))
        #expect(state.statusText == "Running")
        #expect(state.showsMonitoring)
        #expect(state.hasLiveTransport)
        #expect(!state.isModeUnfulfilled)
    }

    @Test("On → Off tears down synchronously")
    func stop() {
        let (state, effects) = step(runningModel(), tap(.off, isEngineRunning: true))
        #expect(state.machine == .settled(.off))
        #expect(state.mode == .off)
        #expect(effects == [.stopEngine])
        #expect(!state.hasLiveTransport)
    }

    @Test("A failed start keeps the mode (intent) and shows the reason")
    func startFailureKeepsIntent() {
        let (state, effects) = step(
            drive(readyModel(), [tap(.on)]),
            .startCompleted(error: "mic exploded")
        )
        #expect(state.mode == .on)
        #expect(state.machine == .settled(.failed("mic exploded", session: nil)))
        #expect(state.engineErrorMessage == "mic exploded")
        #expect(state.isModeUnfulfilled)
        #expect(state.statusText == "Error")
        #expect(effects == [.stopEngine])
        #expect(!state.showsMonitoring)
    }

    @Test("Pre-flight failures settle synchronously, without a busy round-trip")
    func preflightFailures() {
        // Incapable microphone.
        let incapable = step(readyModel(selectedInputUID: bluetoothMic.uid), tap(.on))
        #expect(!incapable.state.isBusy)
        #expect(incapable.state.engineErrorMessage?.contains("can't run at 48 kHz") == true)
        #expect(incapable.effects == [.stopEngine])

        // Missing virtual output.
        var noOutput = readyModel()
        noOutput.isVirtualOutputPresent = false
        let refused = step(noOutput, tap(.on))
        #expect(refused.state.engineErrorMessage == "Install the Noican or BlackHole virtual device")

        // No usable selection.
        let noDevices = step(readyModel(devices: [], selectedInputUID: ""), tap(.on))
        #expect(noDevices.state.engineErrorMessage == "Select an input device")
        #expect(noDevices.state.statusText == "Error")

        // No engine at all.
        var noEngine = readyModel()
        noEngine.isEngineAvailable = false
        let ignored = step(noEngine, tap(.on))
        #expect(ignored.state == noEngine, "without an engine, taps change nothing")
        #expect(ignored.effects.isEmpty)
    }

    @Test("Events that would start a second transition are ignored while busy")
    func busySerialization() {
        let busy = drive(readyModel(), [tap(.on)])
        for event in [
            tap(.off), tap(.preview), .modelSelected("dfn3"),
            .microphoneSelected(usbMic.uid), .monitorTripped, .engineFaulted
        ] as [AppEvent] {
            let (state, effects) = step(busy, event)
            #expect(state.machine == busy.machine, "\(event) must not disturb the transition")
            #expect(effects.isEmpty, "\(event) must not spawn effects while busy")
        }
    }
}

// MARK: - Retry semantics

@Suite("Same-segment retap retries")
struct RetryTests {
    @Test("Re-tapping the failed segment retries; the message stays during the retry")
    func retapRetries() {
        let failed = drive(readyModel(), [tap(.on), .startCompleted(error: "mic exploded")])
        let (retry, effects) = step(failed, tap(.on))
        #expect(retry.isBusy)
        #expect(effects.contains(.startEngine(
            StartAttempt(modelID: "fastenhancer-b", inputUID: builtInMic.uid, monitor: false)
        )))
        // Settled-state rendering: the old failure stays visible while
        // the retry is in flight and clears only on settled success.
        #expect(retry.engineErrorMessage == "mic exploded")
        let recovered = drive(retry, [.startCompleted(error: nil)])
        #expect(recovered.engineErrorMessage == nil)
        #expect(recovered.statusText == "Running")
    }

    @Test("Re-tapping a fulfilled segment is a no-op")
    func fulfilledRetapIgnored() {
        let running = runningModel()
        let (state, effects) = step(running, tap(.on, isEngineRunning: true))
        #expect(state == running)
        #expect(effects.isEmpty)
    }

    @Test("A tap in the poll-lag window (machine says running, engine says stopped) rebuilds")
    func staleRunningRebuilds() {
        // isEngineRunning is the live answer; the machine's belief lags
        // by up to one health tick. Preview must rebuild, not arm a
        // monitor on a dead transport.
        let running = drive(runningModel(), [.monitorTargetErrorChanged(nil)])
        let (state, effects) = step(running, tap(.preview, isEngineRunning: false))
        #expect(effects.first == .stopEngine)
        #expect(effects.contains(.startEngine(
            StartAttempt(modelID: "fastenhancer-b", inputUID: builtInMic.uid, monitor: true)
        )))
        #expect(state.isBusy)
    }
}

// MARK: - Preview and monitor

@Suite("Preview monitor transitions")
struct PreviewTests {
    @Test("Off → Preview starts the engine, then arms the monitor, staying busy throughout")
    func previewStart() {
        let (claimed, claimEffects) = step(readyModel(), tap(.preview))
        #expect(claimEffects.contains(.startEngine(
            StartAttempt(modelID: "fastenhancer-b", inputUID: builtInMic.uid, monitor: true)
        )))
        let (armed, armEffects) = step(claimed, .startCompleted(error: nil))
        #expect(armed.isBusy, "the monitor half keeps the machine busy")
        #expect(armed.statusText == "Off", "still rendering the settled snapshot")
        #expect(armEffects == [.setMonitor(enabled: true)])
        let (settled, _) = step(armed, .monitorChangeCompleted(error: nil))
        #expect(settled.statusText == "Previewing")
        #expect(settled.liveRunningSession?.isMonitorArmed == true)
    }

    @Test("Preview ↔ On only toggles the monitor on a live engine")
    func previewOnToggle() {
        let previewing = runningModel(mode: .preview)
        let (toOn, effects) = step(previewing, tap(.on, isEngineRunning: true))
        #expect(effects == [.setMonitor(enabled: false)])
        #expect(toOn.isBusy)
        let settled = drive(toOn, [.monitorChangeCompleted(error: nil)])
        #expect(settled.statusText == "Running")
        #expect(settled.liveRunningSession?.isMonitorArmed == false)

        let (backToPreview, backEffects) = step(settled, tap(.preview, isEngineRunning: true))
        #expect(backEffects == [.setMonitor(enabled: true)])
        #expect(drive(backToPreview, [.monitorChangeCompleted(error: nil)]).statusText == "Previewing")
    }

    @Test("An unsafe output refuses Preview in place: mode and engine untouched")
    func unsafeOutputRefusal() {
        let (refused, effects) = step(
            readyModel(),
            tap(.preview, monitorTargetError: "built-in speakers would feed back")
        )
        #expect(refused.mode == .off, "the mode never moves on a refusal")
        #expect(refused.machine == .settled(.off), "the engine never starts")
        #expect(effects.isEmpty)
        #expect(refused.messages.previewUnavailableReason == "built-in speakers would feed back")

        // The message clears live once the output becomes safe…
        let cleared = drive(refused, [.monitorTargetErrorChanged(nil)])
        #expect(cleared.messages.previewUnavailableReason == nil)
        // …and only maintains an already-shown message.
        let idle = drive(cleared, [.monitorTargetErrorChanged("some reason")])
        #expect(idle.messages.previewUnavailableReason == nil)
    }

    @Test("A monitor enable failure keeps the engine running and explains under the control")
    func monitorFailureKeepsEngine() {
        let running = runningModel()
        let failed = drive(running, [
            tap(.preview, isEngineRunning: true),
            .monitorChangeCompleted(error: "output device refused")
        ])
        #expect(failed.mode == .preview, "intent is kept")
        #expect(failed.statusText == "Running", "engine keeps running; preview does not")
        #expect(failed.messages.previewError == "output device refused")
        #expect(failed.isModeUnfulfilled, "the pill warns")
        #expect(failed.engineErrorMessage == nil, "not an engine failure")

        // Retap retries and a settled success clears the message.
        let (retry, retryEffects) = step(failed, tap(.preview, isEngineRunning: true))
        #expect(retryEffects == [.setMonitor(enabled: true)])
        #expect(retry.messages.previewError == "output device refused", "kept while in flight")
        let recovered = drive(retry, [.monitorChangeCompleted(error: nil)])
        #expect(recovered.messages.previewError == nil)
        #expect(recovered.statusText == "Previewing")
    }

    @Test("Leaving Preview clears a preview failure; a Preview retry keeps it")
    func previewErrorClearing() {
        let failed = drive(runningModel(), [
            tap(.preview, isEngineRunning: true),
            .monitorChangeCompleted(error: "output device refused")
        ])
        let left = drive(failed, [tap(.on, isEngineRunning: true)])
        #expect(left.messages.previewError == nil)
    }
}

// MARK: - Feedback trip

@Suite("Feedback killswitch")
struct TripTests {
    @Test("A trip disarms the monitor, keeps the mode, and explains itself")
    func tripDisarms() {
        let previewing = runningModel(mode: .preview)
        let (tripped, effects) = step(previewing, .monitorTripped)
        #expect(effects == [.setMonitor(enabled: false)])
        #expect(tripped.messages.previewError?.contains("feedback detected") == true)
        #expect(tripped.mode == .preview, "intent is kept")

        let settled = drive(tripped, [.monitorChangeCompleted(error: nil)])
        #expect(settled.statusText == "Running", "engine keeps running after the trip")
        #expect(
            settled.messages.previewError?.contains("feedback detected") == true,
            "a successful disable must not erase the trip's explanation"
        )
        #expect(settled.isModeUnfulfilled)

        // Retap Preview → re-arm; the settled enable success clears it.
        let rearmed = drive(settled, [
            tap(.preview, isEngineRunning: true),
            .monitorChangeCompleted(error: nil)
        ])
        #expect(rearmed.messages.previewError == nil)
        #expect(rearmed.statusText == "Previewing")
    }

    @Test("A trip without a live transport is ignored")
    func tripIgnoredWhileOff() {
        let (state, effects) = step(readyModel(), .monitorTripped)
        #expect(state == readyModel())
        #expect(effects.isEmpty)
    }
}

// MARK: - Monitor target safety loss

@Suite("Monitor target losing its safety")
struct MonitorSafetyTests {
    @Test("An unsafe target stops the playing preview and explains why")
    func unsafeTargetStopsPreview() {
        let previewing = runningModel(mode: .preview)
        let reason = "the system default output is the built-in speakers, "
            + "which would feed back; connect headphones and try again"
        let (stopped, effects) = step(previewing, .monitorTargetBecameUnsafe(reason: reason))
        #expect(effects == [.setMonitor(enabled: false)])
        #expect(stopped.mode == .preview, "intent is kept")
        #expect(stopped.messages.previewError == "Preview stopped: \(reason).")

        let settled = drive(stopped, [.monitorChangeCompleted(error: nil)])
        #expect(settled.statusText == "Running", "engine keeps running; preview does not")
        #expect(settled.isModeUnfulfilled, "the pill warns")
        #expect(settled.liveRunningSession?.isMonitorArmed == false)

        // Re-tapping Preview retries on the (new) vetted output.
        let rearmed = drive(settled, [
            tap(.preview, isEngineRunning: true),
            .monitorChangeCompleted(error: nil)
        ])
        #expect(rearmed.messages.previewError == nil)
        #expect(rearmed.statusText == "Previewing")
    }

    @Test("Safety loss is ignored while the monitor is not armed")
    func ignoredWhileDisarmed() {
        // Running without a monitor (On mode).
        let running = runningModel()
        let (state, effects) = step(running, .monitorTargetBecameUnsafe(reason: "whatever"))
        #expect(state == running)
        #expect(effects.isEmpty)

        // And while a transition is already in flight.
        let busy = drive(readyModel(), [tap(.preview)])
        let (busyState, busyEffects) = step(busy, .monitorTargetBecameUnsafe(reason: "whatever"))
        #expect(busyState == busy)
        #expect(busyEffects.isEmpty)
    }
}

// MARK: - Microphone switching and fallback

@Suite("Microphone selection")
struct MicrophoneTests {
    @Test("A live switch rebuilds the transport with a one-shot revert target")
    func liveSwitchCarriesRevert() {
        let (state, effects) = step(runningModel(), .microphoneSelected(usbMic.uid))
        let attempt = StartAttempt(
            modelID: "fastenhancer-b",
            inputUID: usbMic.uid,
            monitor: false,
            revertInputUID: builtInMic.uid
        )
        #expect(effects == [.stopEngine, .startEngine(attempt)])
        #expect(state.machine == .busy(.starting(attempt), rendering: .running(
            EngineSession(modelID: "fastenhancer-b", inputUID: builtInMic.uid)
        )))
    }

    @Test("A failed live switch falls back to the previous microphone once")
    func failedSwitchFallsBack() {
        let switching = drive(runningModel(), [.microphoneSelected(usbMic.uid)])
        let (fallback, effects) = step(switching, .startCompleted(error: "usb start failed"))
        // Explicit failure-event → restart-effect transition.
        let fallbackAttempt = StartAttempt(
            modelID: "fastenhancer-b", inputUID: builtInMic.uid, monitor: false
        )
        #expect(effects == [.stopEngine, .startEngine(fallbackAttempt)])
        #expect(fallback.selectedInputUID == builtInMic.uid, "the checkmark returns")
        #expect(fallback.messages.microphoneError == "usb start failed")

        // The fallback succeeding restores the session on the old device.
        let recovered = drive(fallback, [.startCompleted(error: nil)])
        #expect(recovered.liveRunningSession?.inputUID == builtInMic.uid)
        #expect(recovered.messages.microphoneError == "usb start failed", "reason stays under the list")

        // …and a failing fallback (no further revert) settles as failed.
        let dead = drive(fallback, [.startCompleted(error: "builtin also failed")])
        #expect(dead.engineErrorMessage == "builtin also failed")
        #expect(dead.mode == .on, "intent is kept even after the fallback dies")
    }

    @Test("Preview survives a microphone fallback (monitor is re-armed)")
    func fallbackKeepsMonitor() {
        let previewing = runningModel(mode: .preview)
        let fallback = drive(previewing, [
            .microphoneSelected(usbMic.uid),
            .startCompleted(error: "usb start failed")
        ])
        guard case let .busy(.starting(attempt), _) = fallback.machine else {
            Issue.record("expected a fallback start, got \(fallback.machine)")
            return
        }
        #expect(attempt.monitor, "the fallback start preserves the preview intent")
    }

    @Test("An incapable microphone is refused in place while running")
    func incapableRefusedInPlace() {
        let running = runningModel()
        let (state, effects) = step(running, .microphoneSelected(bluetoothMic.uid))
        #expect(effects.isEmpty, "the engine keeps running uninterrupted")
        #expect(state.machine == running.machine)
        #expect(state.selectedInputUID == builtInMic.uid, "the checkmark returns")
        #expect(state.messages.microphoneError?.contains("can't run at 48 kHz") == true)
    }

    @Test("Picking a working microphone after a failure restarts into the selected mode")
    func autoRecovery() {
        let failed = drive(readyModel(), [tap(.on), .startCompleted(error: "mic exploded")])
        let (state, effects) = step(failed, .microphoneSelected(usbMic.uid))
        #expect(effects.contains(.startEngine(
            StartAttempt(modelID: "fastenhancer-b", inputUID: usbMic.uid, monitor: false)
        )))
        #expect(state.mode == .on)
        let recovered = drive(state, [.startCompleted(error: nil)])
        #expect(recovered.statusText == "Running")
    }

    @Test("Changing the microphone while Off clears a stale failure")
    func offSelectionClearsStaleFailure() {
        var failed = readyModel()
        failed.machine = .settled(.failed("old failure", session: nil))
        let (state, effects) = step(failed, .microphoneSelected(usbMic.uid))
        #expect(state.machine == .settled(.off))
        #expect(effects.isEmpty)
    }
}

// MARK: - Model switching

@Suite("Model selection")
struct ModelTests {
    @Test("A live pick switches behind the busy machine and settles into Running")
    func liveSwitch() {
        let (state, effects) = step(runningModel(), .modelSelected("dfn3"))
        #expect(effects == [.switchModel(to: "dfn3")])
        #expect(state.isBusy)
        #expect(state.statusText == "Running", "meters and status hold the settled snapshot")
        let settled = drive(state, [.modelSwitchCompleted(error: nil)])
        #expect(settled.liveRunningSession?.modelID == "dfn3")
    }

    @Test("A failed switch reverts the picker and keeps the previous model running")
    func failedSwitchReverts() {
        let running = runningModel()
        let failed = drive(running, [
            .modelSelected("dfn3"),
            .modelSwitchCompleted(error: "weights download failed")
        ])
        #expect(failed.selectedModelID == "fastenhancer-b", "the picker never lies")
        #expect(failed.messages.modelError == "weights download failed")
        #expect(failed.statusText == "Running", "not an engine failure")
        #expect(failed.machine == running.machine)
    }

    @Test("Picking a model while Off only updates the selection and un-blames stale failures")
    func offPick() {
        var failed = readyModel()
        failed.machine = .settled(.failed("old failure", session: nil))
        let (state, effects) = step(failed, .modelSelected("dfn3"))
        #expect(state.machine == .settled(.off))
        #expect(state.selectedModelID == "dfn3")
        #expect(effects.isEmpty)
    }

    @Test("Teardown clears a model-switch message (it describes the torn-down engine)")
    func teardownClearsModelError() {
        let withModelError = drive(runningModel(), [
            .modelSelected("dfn3"),
            .modelSwitchCompleted(error: "weights download failed")
        ])
        let stopped = drive(withModelError, [tap(.off, isEngineRunning: true)])
        #expect(stopped.messages.modelError == nil)
    }
}

// MARK: - Device environment and runtime health

@Suite("Device changes and health findings")
struct EnvironmentTests {
    private func devicesEvent(
        _ inputs: [InputDevice],
        virtualOutput: Bool = true
    ) -> AppEvent {
        .devicesChanged(
            inputs: inputs,
            allInputUIDs: Set(inputs.map(\.uid)),
            isVirtualOutputPresent: virtualOutput
        )
    }

    @Test("Losing the running microphone stops the engine but keeps the intent")
    func micLossStops() {
        let (state, effects) = step(runningModel(), devicesEvent([usbMic]))
        #expect(effects == [.stopEngine])
        #expect(state.engineErrorMessage == "Microphone disconnected — noise cancellation stopped")
        #expect(state.mode == .on, "the pill shows what the user asked for")
        #expect(state.selectedInputUID == usbMic.uid, "the vanished selection is reassigned")
    }

    @Test("Losing the virtual output stops the engine")
    func virtualOutputLossStops() {
        let (state, effects) = step(
            runningModel(),
            devicesEvent([builtInMic, usbMic], virtualOutput: false)
        )
        #expect(effects == [.stopEngine])
        #expect(state.engineErrorMessage == "Virtual output device removed — noise cancellation stopped")
    }

    @Test("A runtime stop clears the preview message but keeps the microphone one")
    func runtimeStopMessageRules() {
        var previewing = runningModel(mode: .preview)
        previewing.messages.previewError = "some stale preview failure"
        previewing.messages.microphoneError = "some microphone note"
        let (state, _) = step(previewing, .audioStalled)
        #expect(state.messages.previewError == nil)
        #expect(state.messages.microphoneError == "some microphone note")
        #expect(state.engineErrorMessage == "Audio stalled — device lost or audio system restarted")
    }

    @Test("An engine fault keeps the session for the next user action")
    func faultKeepsSession() {
        let (state, effects) = step(runningModel(), .engineFaulted)
        #expect(effects.isEmpty, "a fault does not tear the transport down")
        let session = EngineSession(modelID: "fastenhancer-b", inputUID: builtInMic.uid)
        #expect(state.machine == .settled(
            .failed("Audio fault — turn noise cancellation off and on", session: session)
        ))
        #expect(state.hasLiveTransport, "the health poll keeps watching it")

        // Re-tapping the segment rebuilds from the fault.
        let (retry, retryEffects) = step(state, tap(.on, isEngineRunning: true))
        #expect(retryEffects.first == .stopEngine)
        #expect(retry.isBusy)
    }

    @Test("An unexpected stop settles as failed with the transport torn down")
    func unexpectedStop() {
        let (state, effects) = step(runningModel(), .engineStoppedUnexpectedly)
        #expect(effects == [.stopEngine])
        #expect(state.engineErrorMessage == "Engine stopped unexpectedly")
        #expect(!state.hasLiveTransport)
    }

    @Test("Device changes while busy refresh the list but never disturb the transition")
    func deviceChangeWhileBusy() {
        let busy = drive(readyModel(), [tap(.on)])
        let (state, effects) = step(busy, devicesEvent([usbMic]))
        #expect(effects.isEmpty)
        #expect(state.machine == busy.machine)
        #expect(state.inputDevices == [usbMic])
    }

    @Test("Mode taps clear the refusal and microphone messages declaratively")
    func modeTapClearsMessages() {
        var state = runningModel()
        state.messages.microphoneError = "stale microphone message"
        state.messages.previewUnavailableReason = "stale refusal"
        let (cleared, _) = step(state, tap(.preview, isEngineRunning: true))
        #expect(cleared.messages.microphoneError == nil)
        #expect(cleared.messages.previewUnavailableReason == nil)
    }
}
