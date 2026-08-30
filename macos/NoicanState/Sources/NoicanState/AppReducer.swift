/// The pure transition function of the menu bar app.
///
/// `reduce` maps one (state, event) pair to the next state plus the
/// effects the runtime shell must perform. Every transition — including
/// the ones that used to be buried in detached-task completion callbacks
/// (start results, monitor toggles, the one-shot microphone fallback) —
/// is an explicit event → state change here, so the whole machine is
/// unit-testable without Core Audio or the Rust engine.
///
/// Invariants encoded by construction rather than by discipline:
/// - `AppModel.mode` is the user's intent; no event other than
///   `modeSelected` writes it.
/// - At most one `EngineTransition` is in flight (`EngineMachine.busy`),
///   and every event that would start another one is ignored until it
///   settles — the old `isBusy` serialization.
/// - While busy the UI renders the `rendering` snapshot; state the UI
///   projects changes only when a transition settles.
/// - Message-slot clearing is declared per event, in one place each.
public enum AppReducer {
    /// Applies `event` to `state`, returning the next state and the
    /// effects to perform (in order).
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
        case let .deviceQueryFailed(message):
            return deviceQueryFailed(state, message)
        }
    }
}

// MARK: - User intents

extension AppReducer {
    /// The mode control is intent: the system never changes it, only the
    /// user does. Tapping the already-selected segment retries when the
    /// intent is unfulfilled (e.g. the last start failed).
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
        // Keep a preview failure visible while a Preview retry is in
        // flight (settled-state rendering: only a settled enable success
        // clears it, in `monitorChangeCompleted`); leaving Preview clears
        // it now.
        if newMode != .preview {
            state.messages.previewError = nil
        }
        state.messages.previewUnavailableReason = nil
        if newMode == .preview, let reason = monitorTargetError {
            // Refuse in place and explain: neither the mode nor the
            // engine changes, and `monitorTargetErrorChanged` clears the
            // message as soon as the output becomes safe.
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
                // Running healthily: only the monitor half can differ.
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

    /// A model pick applies immediately when the engine runs; while Off it
    /// only updates the selection (and un-blames a stale failure).
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
        // Changing the model while stopped starts nothing; clear a stale
        // failure message so the menu does not keep blaming the last
        // attempt.
        if state.mode == .off, case .settled(.failed) = state.machine {
            state.machine = .settled(.off)
        }
        guard
            state.mode != .off, !state.isBusy, state.isEngineAvailable,
            id != state.liveSession?.modelID
        else {
            return (state, [])
        }
        state.machine = .busy(
            .switchingModel(to: id, session: state.liveSession),
            rendering: state.phase
        )
        return (state, [.switchModel(to: id)])
    }

    /// The strength slider is picker-like state with an atomic-write
    /// effect: it claims no engine transition (the engine applies it
    /// lock-free mid-stream), so it is accepted even while busy and
    /// regardless of the engine phase — the value persists across engine
    /// lifecycles and seeds the next start.
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

    /// The login-item toggle moves optimistically (the registration
    /// attempt follows as an effect); `launchAtLoginChangeCompleted`
    /// snaps it back to the re-read real status when the attempt fails,
    /// so the toggle never lies for longer than the attempt takes.
    /// Exactly one attempt may be in flight (`isLaunchAtLoginBusy`):
    /// a re-toggle before the outcome lands is ignored, because two
    /// concurrent register/unregister calls would race and the toggle
    /// would settle on whichever completion arrived last.
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

    /// A microphone pick pre-flights the device, then rebuilds the
    /// transport around it when the engine is meant to run. Because the
    /// mode keeps the user's intent across failures, this same path
    /// auto-recovers: picking a working microphone after a failed start
    /// restarts straight into the selected mode.
    private static func microphoneSelected(
        _ state: AppModel,
        _ uid: String
    ) -> (state: AppModel, effects: [AppEffect]) {
        guard uid != state.selectedInputUID else {
            // Re-clicking the already-selected microphone acknowledges a
            // shown refusal: the message describes a *rejected other*
            // device, so confirming the working one clears it (the
            // engine and the selection are untouched).
            var state = state
            state.messages.microphoneError = nil
            return (state, [])
        }
        var state = state
        state.selectedInputUID = uid
        state.messages.microphoneError = nil
        // Changing the microphone while Off only updates the selection;
        // clear a stale failure so the menu stops blaming the previous
        // device the moment another one is chosen.
        if state.mode == .off, case .settled(.failed) = state.machine {
            state.machine = .settled(.off)
        }
        guard
            state.mode != .off, !state.isBusy, state.isEngineAvailable,
            uid != state.liveSession?.inputUID
        else {
            return (state, [])
        }
        // Pre-flight the new microphone before tearing anything down.
        if let device = state.inputDevices.first(where: { $0.uid == uid }),
           let reason = capabilityError(of: device) {
            if let session = state.liveSession {
                // The engine keeps running on the current microphone;
                // put the checkmark back and explain under the list.
                state.selectedInputUID = session.inputUID
                state.messages.microphoneError = reason
            } else {
                state.machine = .settled(.failed(reason, session: nil))
            }
            return (state, [])
        }
        // A live change rebuilds the transport with the same model and
        // mode (a brief gap is inherent). If the rebuild itself fails,
        // the previous device (which was working a moment ago) is
        // restored by the `startCompleted` failure transition instead of
        // leaving the session dead.
        return startTransition(
            state,
            monitor: state.mode == .preview,
            revertInputUID: state.liveSession?.inputUID
        )
    }
}

// MARK: - Effect completions

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
                // Preview mode is engine + monitor; the monitor half
                // starts once the transport is up, staying busy (and
                // rendering the same snapshot) until its outcome lands.
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
            // A failed live microphone switch must not kill the session:
            // fall back to the device that was working a moment ago —
            // the explicit failure-event → restart-effect transition
            // (one attempt: the fallback start carries no further revert
            // target). The reason stays visible under the microphone
            // list.
            state.messages.microphoneError = error
            state.selectedInputUID = revert
            return startTransition(state, monitor: attempt.monitor, revertInputUID: nil)
        }
        // The mode keeps the user's intent; the red pill tint and the
        // error below the control say it is not running. Retry by
        // tapping the segment again or picking another microphone.
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
        // The engine itself keeps running either way; the mode keeps the
        // user's intent. On failure the pill's warning tint plus the
        // message below the control say the preview is not playing, and
        // re-tapping Preview retries. Settled-state rendering: a kept
        // preview failure clears only when an enable actually succeeds
        // (a successful disable after a feedback trip must not erase the
        // trip's explanation).
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
                ?? EngineSession(modelID: target, inputUID: state.selectedInputUID)
            newSession.modelID = target
            state.machine = .settled(.running(newSession))
            return (state, [])
        }
        // Settle back to exactly the rendered snapshot: on a healthy
        // engine the switch failure is not an engine failure (previous
        // model keeps running, the phase stays running), and on a
        // stopped one (the switch fails fast) painting running would be
        // a green lie. The reason renders under the Model picker, with
        // the picker reverted to stay truthful.
        if let session {
            state.selectedModelID = session.modelID
        }
        state.messages.modelError = error
        state.machine = .settled(rendering)
        return (state, [])
    }

    /// The registration attempt settled: show the *re-read* real status
    /// (an optimistic toggle that failed snaps back here), surface the
    /// failure reason under the toggle, and release the serialization
    /// gate. Registration is inherently environment-dependent (app
    /// location, signature), so the outcome is authoritative and the
    /// request is not. A completion without a claimed attempt is stale
    /// by construction and ignored.
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

// MARK: - Environment observations

extension AppReducer {
    /// Applies persisted preferences at launch. Accepted only while the
    /// user's intent is Off with no transition in flight — the state the
    /// app launches in — so a late or replayed restore can never disturb
    /// a session the user has since started. Selections flow through the
    /// same fields the pickers use; the mode is never restored (the app
    /// always starts Off, so launch never captures the microphone).
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
        // A stored microphone that is not currently connected is skipped
        // (not remembered as a dangling selection): the reducer's other
        // transitions assume the selection exists in the list.
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

    /// Seeds the login-item toggle from the `SMAppService` status read
    /// at launch (the service is the source of truth; the app persists
    /// nothing about it). Ignored while an attempt is in flight — its
    /// completion re-reads the status and is the fresher answer.
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

    /// Follows device hot-plug: refreshes the list, reassigns a vanished
    /// selection, and stops the engine with a clear message when the
    /// microphone in use (or the virtual output) disappears.
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
        // The transport is bound to the session's input (the selection
        // can legitimately differ, e.g. right after a refused switch).
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

    /// The running microphone's nominal rate changed under the transport
    /// (Bluetooth A2DP ↔ HFP renegotiation): the transport captures at
    /// the rate fixed at start time, so rebuild it into the current mode
    /// around the new rate — the same automatic recovery a live
    /// microphone switch performs. The shell pre-filters spurious
    /// notifications (same rate) and the busy machine serializes
    /// overlapping rebuilds, so this cannot storm.
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

    /// Maintains a shown refusal message only: once a Preview attempt was
    /// refused, the shell re-samples the pre-flight so the message clears
    /// (or updates) live as the user fixes the default output.
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

    /// Reacts to the engine-side feedback killswitch: the worker already
    /// silenced the preview, so release the playback device and tell the
    /// user why. The mode stays on Preview (it is the user's intent);
    /// the warning tint and the message say it is not playing, and
    /// re-tapping Preview retries.
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

    /// The playing monitor's target lost its safety (headphone jack
    /// unplugged into the internal speakers, or the device vanished):
    /// stop the preview immediately, exactly like a feedback trip — the
    /// engine keeps running, the mode keeps the user's intent, the
    /// reason renders under the control, and re-tapping Preview retries
    /// on the (new) vetted output.
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

    /// An engine fault reports failure without tearing the transport
    /// down (the session stays in the failed phase); the next user
    /// action rebuilds it.
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

    /// Runtime stops (device loss, stalls) tear the engine down but keep
    /// the mode: the pill shows what the user asked for, the red tint and
    /// the message say it is not running, and selecting another
    /// microphone (or re-tapping the segment) restarts into that mode.
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

// MARK: - Shared transitions

extension AppReducer {
    /// Off: tear the transport down synchronously. No busy state — a
    /// stop is immediate and cannot fail.
    private static func stopTransition(
        _ state: AppModel
    ) -> (state: AppModel, effects: [AppEffect]) {
        var state = state
        state.messages.modelError = nil
        state.machine = .settled(.off)
        return (state, [.stopEngine])
    }

    /// Claims a start: tears any previous transport down, pre-flights
    /// everything that can be checked synchronously (an incapable
    /// microphone fails here, before any busy round-trip, so the reason
    /// appears without the UI flashing transitional state), then goes
    /// busy with the attempt.
    private static func startTransition(
        _ state: AppModel,
        monitor: Bool,
        revertInputUID: String?
    ) -> (state: AppModel, effects: [AppEffect]) {
        var state = state
        // The teardown clears any live session; a model-switch message
        // would describe the torn-down engine, so it clears with it.
        state.messages.modelError = nil
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
            // Capture the pre-flighted device: the selection can be
            // reassigned by device hot-plug during the busy window, and
            // the transport is bound to this one.
            inputUID: input.uid,
            monitor: monitor,
            revertInputUID: revertInputUID
        )
        state.machine = .busy(.starting(attempt), rendering: state.phase)
        return (state, [.stopEngine, .startEngine(attempt)])
    }

    /// Claims a monitor toggle on a live transport, serialized like every
    /// other engine transition.
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

    /// Why `device` cannot serve as the engine's microphone, or nil when
    /// it can. Decided from the snapshot taken at device-refresh time —
    /// no Core Audio call, so it is safe before any transition.
    /// Telephony-rate devices (Bluetooth headset microphones) are *not*
    /// refused: the transport captures them natively and resamples
    /// (issue #7). Only rates that cannot reach 48 kHz by an integer
    /// factor remain unusable.
    private static func capabilityError(of device: InputDevice) -> String? {
        guard case let .unsupported(hertz) = device.capture else {
            return nil
        }
        return "The microphone \"\(device.name)\" runs at \(hertz) Hz, which Noican "
            + "can't resample to the 48 kHz engine rate — choose another microphone."
    }
}
