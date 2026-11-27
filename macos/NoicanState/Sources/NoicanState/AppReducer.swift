public enum AppReducer {
    public static func reduce(_ state: AppModel, _ event: AppEvent) -> (state: AppModel, effects: [AppEffect]) {
        switch event {
        case let .modeSelected(mode, monitorTargetError, isEngineRunning):
            return modeSelected(state, mode, monitorTargetError, isEngineRunning)
        case let .modelSelected(id):
            return modelSelected(state, id)
        case let .microphoneSelected(uid):
            return microphoneSelected(state, uid)
        case let .intensityChanged(value):
            return intensityChanged(state, value)
        case let .launchAtLoginToggled(enabled):
            return launchAtLoginToggled(state, enabled)
        case let .launchAtLoginChangeCompleted(isEnabled, error):
            return launchAtLoginChangeCompleted(state, isEnabled, error)
        case let .preferencesRestored(modelID, inputUID, intensity):
            return preferencesRestored(state, modelID, inputUID, intensity)
        case let .launchAtLoginStatusRead(isEnabled):
            return launchAtLoginStatusRead(state, isEnabled)
        case let .startCompleted(error):
            return startCompleted(state, error)
        case let .monitorChangeCompleted(error):
            return monitorChangeCompleted(state, error)
        case let .modelSwitchCompleted(error):
            return modelSwitchCompleted(state, error)
        case let .devicesChanged(inputs, allInputUIDs, isVirtualOutputPresent):
            return devicesChanged(state, inputs, allInputUIDs, isVirtualOutputPresent)
        case .inputSampleRateChanged:
            return inputSampleRateChanged(state)
        case let .monitorTargetErrorChanged(reason):
            return monitorTargetErrorChanged(state, reason)
        case .monitorTripped:
            return monitorTripped(state)
        case let .monitorTargetBecameUnsafe(reason):
            return monitorTargetBecameUnsafe(state, reason)
        case .engineFaulted:
            return engineFaulted(state)
        case .engineStoppedUnexpectedly:
            return runtimeStopObserved(state, "Engine stopped unexpectedly")
        case .audioStalled:
            return runtimeStopObserved(state, "Audio stalled — device lost or audio system restarted")
        case let .virtualOutputLevelObserved(level):
            return virtualOutputLevelObserved(state, level)
        case let .deviceQueryFailed(message):
            return deviceQueryFailed(state, message)
        case let .processingAllowanceChanged(isAllowed):
            return processingAllowanceChanged(state, isAllowed)
        }
    }
}

extension AppReducer {
    private static func modeSelected(
        _ state: AppModel,
        _ newMode: EngineMode,
        _ monitorTargetError: String?,
        _ isEngineRunning: Bool
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard !state.isBusy, state.isEngineAvailable else {
            return (state, [])
        }
        guard newMode != state.mode || state.isModeUnfulfilled else {
            return (state, [])
        }
        var state = state
        state.messages.microphoneError = nil
        if newMode != .preview {
            state.messages.previewError = nil
        }
        state.messages.previewUnavailableReason = nil
        guard !refusesWithoutLicense(&state, newMode) else { return (state, []) }
        if newMode == .preview, let reason = monitorTargetError {
            state.messages.previewUnavailableReason = reason
            return (state, [])
        }
        let hadError = state.engineErrorMessage != nil
        let previous = state.mode
        state.mode = newMode
        switch newMode {
        case .off:
            return stopTransition(state)
        case .on:
            if !hadError, isEngineRunning, let session = state.liveRunningSession {
                if previous == .preview {
                    return monitorTransition(state, session: session, enabled: false)
                }
                return (state, [])
            }
            return startTransition(state, monitor: false, revertInputUID: nil)
        case .preview:
            if !hadError, isEngineRunning, let session = state.liveRunningSession {
                return monitorTransition(state, session: session, enabled: true)
            }
            return startTransition(state, monitor: true, revertInputUID: nil)
        }
    }

    private static func modelSelected(
        _ state: AppModel,
        _ id: String
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard id != state.selectedModelID else {
            return (state, [])
        }
        var state = state
        state.selectedModelID = id
        state.messages.modelError = nil
        if state.mode == .off, case .settled(.failed) = state.machine {
            state.machine = .settled(.off)
        }
        guard
            state.mode != .off, !state.isBusy, state.isEngineAvailable,
            id != state.liveSession?.modelID
        else {
            return (state, [])
        }
        guard let session = state.liveSession else {
            return startTransition(state, monitor: state.mode == .preview, revertInputUID: nil)
        }
        state.machine = .busy(
            .switchingModel(to: id, session: session),
            rendering: state.phase
        )
        return (state, [.switchModel(to: id)])
    }

    private static func intensityChanged(
        _ state: AppModel,
        _ value: Double
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard value.isFinite else {
            return (state, [])
        }
        let clamped = min(1, max(0, value))
        guard clamped != state.intensity else {
            return (state, [])
        }
        var state = state
        state.intensity = clamped
        return (state, [.setIntensity(clamped)])
    }

    private static func launchAtLoginToggled(
        _ state: AppModel,
        _ enabled: Bool
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard !state.isLaunchAtLoginBusy, enabled != state.isLaunchAtLoginEnabled else {
            return (state, [])
        }
        var state = state
        state.isLaunchAtLoginEnabled = enabled
        state.isLaunchAtLoginBusy = true
        state.messages.launchAtLoginError = nil
        return (state, [.setLaunchAtLogin(enabled: enabled)])
    }

    private static func microphoneSelected(
        _ state: AppModel,
        _ uid: String
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard uid != state.selectedInputUID else {
            var state = state
            state.messages.microphoneError = nil
            return (state, [])
        }
        var state = state
        state.selectedInputUID = uid
        state.messages.microphoneError = nil
        if state.mode == .off, case .settled(.failed) = state.machine {
            state.machine = .settled(.off)
        }
        guard
            state.mode != .off, !state.isBusy, state.isEngineAvailable,
            uid != state.liveSession?.inputUID
        else {
            return (state, [])
        }
        if let device = state.inputDevices.first(where: { $0.uid == uid }),
           let reason = capabilityError(of: device) {
            if let session = state.liveSession {
                state.selectedInputUID = session.inputUID
                state.messages.microphoneError = reason
            } else {
                state.machine = .settled(.failed(reason, session: nil))
            }
            return (state, [])
        }
        return startTransition(
            state,
            monitor: state.mode == .preview,
            revertInputUID: state.liveSession?.inputUID
        )
    }
}

extension AppReducer {
    private static func startCompleted(
        _ state: AppModel,
        _ error: String?
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard case let .busy(.starting(attempt), rendering) = state.machine else {
            return (state, [])
        }
        var state = state
        guard let error else {
            let session = EngineSession(modelID: attempt.modelID, inputUID: attempt.inputUID)
            if attempt.monitor {
                state.machine = .busy(
                    .settingMonitor(enabled: true, session: session),
                    rendering: rendering
                )
                return (state, [.setMonitor(enabled: true)])
            }
            state.machine = .settled(.running(session))
            return (state, [])
        }
        if let revert = attempt.revertInputUID,
           revert != attempt.inputUID,
           state.inputDevices.contains(where: { $0.uid == revert }) {
            state.messages.microphoneError = error
            state.selectedInputUID = revert
            return startTransition(state, monitor: attempt.monitor, revertInputUID: nil)
        }
        state.messages.modelError = nil
        state.machine = .settled(.failed(error, session: nil))
        return (state, [.stopEngine])
    }

    private static func monitorChangeCompleted(
        _ state: AppModel,
        _ error: String?
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard case .busy(.settingMonitor(let enabled, var session), let rendering) = state.machine else {
            return (state, [])
        }
        var state = state
        guard state.mode != .off else {
            state.machine = .settled(rendering)
            return (state, [])
        }
        session.isMonitorArmed = error == nil && enabled
        state.machine = .settled(.running(session))
        if let error {
            state.messages.previewError = error
        } else if enabled {
            state.messages.previewError = nil
        }
        return (state, [])
    }

    private static func modelSwitchCompleted(
        _ state: AppModel,
        _ error: String?
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard case let .busy(.switchingModel(target, session), rendering) = state.machine else {
            return (state, [])
        }
        var state = state
        guard let error else {
            var newSession = session
            newSession.modelID = target
            state.machine = .settled(.running(newSession))
            return (state, [])
        }
        state.selectedModelID = session.modelID
        state.messages.modelError = error
        state.machine = .settled(rendering)
        return (state, [])
    }

    private static func launchAtLoginChangeCompleted(
        _ state: AppModel,
        _ isEnabled: Bool,
        _ error: String?
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard state.isLaunchAtLoginBusy else {
            return (state, [])
        }
        var state = state
        state.isLaunchAtLoginBusy = false
        state.isLaunchAtLoginEnabled = isEnabled
        state.messages.launchAtLoginError = error
        return (state, [])
    }
}

extension AppReducer {
    private static func preferencesRestored(
        _ state: AppModel,
        _ modelID: String?,
        _ inputUID: String?,
        _ intensity: Double?
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard state.mode == .off, !state.isBusy else {
            return (state, [])
        }
        var state = state
        if let modelID {
            state.selectedModelID = modelID
        }
        if let inputUID, state.inputDevices.contains(where: { $0.uid == inputUID }) {
            state.selectedInputUID = inputUID
        }
        var effects: [AppEffect] = []
        if let intensity, intensity.isFinite {
            let clamped = min(1, max(0, intensity))
            state.intensity = clamped
            effects.append(.setIntensity(clamped))
        }
        return (state, effects)
    }

    private static func launchAtLoginStatusRead(
        _ state: AppModel,
        _ isEnabled: Bool
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard !state.isLaunchAtLoginBusy else {
            return (state, [])
        }
        var state = state
        state.isLaunchAtLoginEnabled = isEnabled
        return (state, [])
    }

    private static func devicesChanged(
        _ state: AppModel,
        _ inputs: [InputDevice],
        _ allInputUIDs: Set<String>,
        _ isVirtualOutputPresent: Bool
    ) -> (state: AppModel, effects: [AppEffect]) {
        var state = state
        state.inputDevices = inputs
        state.isVirtualOutputPresent = isVirtualOutputPresent
        if !inputs.contains(where: { $0.uid == state.selectedInputUID }) {
            state.selectedInputUID = inputs.first?.uid ?? ""
        }
        guard state.mode != .off, !state.isBusy, let session = state.liveSession else {
            return (state, [])
        }
        if !allInputUIDs.contains(session.inputUID) {
            return runtimeStopObserved(state, "Microphone disconnected — noise cancellation stopped")
        }
        if !isVirtualOutputPresent {
            return runtimeStopObserved(state, "Virtual output device removed — noise cancellation stopped")
        }
        return (state, [])
    }

    private static func inputSampleRateChanged(
        _ state: AppModel
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard state.mode != .off, !state.isBusy, state.liveSession != nil else {
            return (state, [])
        }
        return startTransition(
            state,
            monitor: state.mode == .preview,
            revertInputUID: nil
        )
    }

    private static func monitorTargetErrorChanged(
        _ state: AppModel,
        _ reason: String?
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard state.messages.previewUnavailableReason != nil else {
            return (state, [])
        }
        var state = state
        state.messages.previewUnavailableReason = reason
        return (state, [])
    }

    private static func monitorTripped(
        _ state: AppModel
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard state.mode != .off, !state.isBusy, let session = state.liveSession else {
            return (state, [])
        }
        var state = state
        state.messages.previewError =
            "Preview stopped itself: feedback detected. Use headphones, then select Preview again."
        return monitorTransition(state, session: session, enabled: false)
    }

    private static func monitorTargetBecameUnsafe(
        _ state: AppModel,
        _ reason: String
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard
            state.mode != .off, !state.isBusy,
            let session = state.liveSession, session.isMonitorArmed
        else {
            return (state, [])
        }
        var state = state
        state.messages.previewError = "Preview stopped: \(reason)."
        return monitorTransition(state, session: session, enabled: false)
    }

    private static func virtualOutputLevelObserved(
        _ state: AppModel,
        _ level: VirtualOutputLevel
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard state.transportSession != nil else {
            return (state, [])
        }
        var state = state
        state.virtualOutputLevel = level
        return (state, [])
    }

    private static func engineFaulted(
        _ state: AppModel
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard state.mode != .off, !state.isBusy, let session = state.liveSession else {
            return (state, [])
        }
        var state = state
        state.machine = .settled(
            .failed("Audio fault — turn noise cancellation off and on", session: session)
        )
        return (state, [])
    }

    private static func runtimeStopObserved(
        _ state: AppModel,
        _ message: String
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard state.mode != .off, !state.isBusy, state.liveSession != nil else {
            return (state, [])
        }
        var state = state
        state.messages.previewError = nil
        state.messages.modelError = nil
        state.virtualOutputLevel = .nominal
        state.machine = .settled(.failed(message, session: nil))
        return (state, [.stopEngine])
    }

    private static func deviceQueryFailed(
        _ state: AppModel,
        _ message: String
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard case .settled = state.machine else {
            return (state, [])
        }
        var state = state
        state.machine = .settled(.failed(message, session: state.liveSession))
        return (state, [])
    }
}

extension AppReducer {
    private static func stopTransition(
        _ state: AppModel
    ) -> (state: AppModel, effects: [AppEffect]) {
        var state = state
        state.messages.modelError = nil
        state.virtualOutputLevel = .nominal
        state.machine = .settled(.off)
        return (state, [.stopEngine])
    }

    private static func startTransition(
        _ state: AppModel,
        monitor: Bool,
        revertInputUID: String?
    ) -> (state: AppModel, effects: [AppEffect]) {
        var state = state
        guard !refusesStartWithoutLicense(&state) else { return (state, []) }
        state.messages.modelError = nil
        state.virtualOutputLevel = .nominal
        guard state.isEngineAvailable else {
            state.machine = .settled(.failed("Rust engine unavailable", session: nil))
            return (state, [.stopEngine])
        }
        guard let input = state.inputDevices.first(where: { $0.uid == state.selectedInputUID }) else {
            state.machine = .settled(.failed("Select an input device", session: nil))
            return (state, [.stopEngine])
        }
        if let reason = capabilityError(of: input) {
            state.machine = .settled(.failed(reason, session: nil))
            return (state, [.stopEngine])
        }
        guard state.isVirtualOutputPresent else {
            state.machine = .settled(
                .failed("Install the Noican or BlackHole virtual device", session: nil)
            )
            return (state, [.stopEngine])
        }
        let attempt = StartAttempt(
            modelID: state.selectedModelID,
            inputUID: input.uid,
            monitor: monitor,
            revertInputUID: revertInputUID
        )
        state.machine = .busy(.starting(attempt), rendering: state.phase)
        return (state, [.stopEngine, .startEngine(attempt)])
    }

    private static func monitorTransition(
        _ state: AppModel,
        session: EngineSession,
        enabled: Bool
    ) -> (state: AppModel, effects: [AppEffect]) {
        var state = state
        state.machine = .busy(
            .settingMonitor(enabled: enabled, session: session),
            rendering: state.phase
        )
        return (state, [.setMonitor(enabled: enabled)])
    }

    private static func capabilityError(of device: InputDevice) -> String? {
        guard case let .unsupported(hertz) = device.capture else {
            return nil
        }
        return "The microphone \"\(device.name)\" runs at \(hertz) Hz, which Noican "
            + "can't resample to the 48 kHz engine rate — choose another microphone."
    }
}
