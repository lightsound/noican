import Combine
import CoreAudio
import Foundation
import NoicanState
import ServiceManagement

/// The runtime shell around the pure state machine in the `NoicanState`
/// package: it samples the environment (Core Audio device lists,
/// pre-flight checks, engine health), feeds everything into
/// `AppReducer.reduce` as events, performs the effects the reducer
/// requests against the aggregate and the Rust engine, and publishes the
/// resulting `AppModel` for SwiftUI. All transition *decisions* live in
/// the reducer; nothing here changes `model` except `dispatch(_:)`.
@MainActor
final class AppState: ObservableObject {
    /// The reducer state, replaced wholesale by `dispatch(_:)`. The UI
    /// renders its projections (`statusText`, `phase`, message slots, …),
    /// which are all derived from settled snapshots.
    @Published private(set) var model: AppModel
    /// Peak meters, refreshed by `pollLevels()` only while the popover is
    /// open. Display-only samples of lock-free engine atomics — not state
    /// machine state, so they stay outside the reducer. Independent of
    /// the Preview state: they move whenever the engine runs.
    @Published private(set) var inputLevel: Float = 0
    @Published private(set) var outputLevel: Float = 0

    /// Selectable models, read from the Rust registry at launch.
    let models = RustEngine.models()

    /// Model selected on first launch (also labeled "Default" in the
    /// model list, and the target of the Balanced quality preset —
    /// nonisolated so the preset table can reference it).
    nonisolated static let defaultModelID = "fastenhancer-b"

    /// `UserDefaults` keys for the persisted preferences. Only picker
    /// state is persisted (microphone UID, model id, strength) — never
    /// the mode: the app always launches Off so it never captures the
    /// microphone without a user action.
    private enum PreferenceKey {
        static let modelID = "selectedModelID"
        static let inputUID = "selectedInputUID"
        static let intensity = "intensity"
    }

    /// Full Core Audio device snapshot (the reducer sees the pure
    /// `InputDevice` projection; effects need the identifiers here).
    private var allDevices: [AudioDeviceInfo] = []
    private let aggregate = AggregateDevice()
    private var engine: RustEngine?
    /// Watches for engine faults/unexpected stops while a transport is
    /// live. Owned here (not by the menu view) so faults are detected
    /// even when the popover is closed; started/stopped by
    /// `syncFaultPolling()` from the reducer's `hasLiveTransport`.
    private var faultPollTask: Task<Void, Never>?
    /// Heartbeat state for detecting a device that stopped calling back.
    private var lastFrameCount: UInt64 = 0
    private var stalledTicks = 0
    /// Follows the monitor's actual output device while the preview
    /// plays: a data-source flip (headphone jack → internal speakers on
    /// the same built-in device) fires no device-list or default-output
    /// notification, so this per-device listener — plus the 1 Hz health
    /// poll as belt and braces — is what catches it. Registered when a
    /// monitor enable settles; always removed when the monitor disarms
    /// (`syncMonitorSafetyObservation`).
    private var monitorSafetyListener: (device: AudioObjectID, block: AudioObjectPropertyListenerBlock)?

    init() {
        var initialPhase = EnginePhase.off
        do {
            engine = try RustEngine()
        } catch {
            initialPhase = .failed(error.localizedDescription, session: nil)
        }
        var modelID = Self.defaultModelID
        if !models.contains(where: { $0.id == modelID }) {
            modelID = models.first?.id ?? ""
        }
        model = AppModel(
            machine: .settled(initialPhase),
            selectedModelID: modelID,
            isEngineAvailable: engine != nil
        )
        refreshDevices()
        // Restore after the first device read so the reducer can vet the
        // stored microphone against the live list.
        restorePreferences()
        dispatch(.launchAtLoginStatusRead(
            isEnabled: Self.isLoginItemRegistered(SMAppService.mainApp.status)
        ))
        registerDeviceListener()
        registerDefaultOutputListener()
    }

    // MARK: - User intents (forwarded to the reducer with environment reads)

    /// The mode control is intent: the reducer never moves it on its own
    /// and re-tapping the selected segment retries an unfulfilled mode.
    func setMode(_ newMode: EngineMode) {
        // Environment reads the pure reducer cannot perform, sampled at
        // dispatch time. Guarded on busy first so a tap during a
        // transition never touches the engine (the monitor-target check
        // is lock-free, but `isRunning` takes the control mutex).
        guard !model.isBusy, let engine else {
            return
        }
        dispatch(.modeSelected(
            newMode,
            monitorTargetError: newMode == .preview ? RustEngine.monitorTargetError : nil,
            isEngineRunning: engine.isRunning
        ))
    }

    /// User-initiated model pick (the Picker's binding setter).
    /// Programmatic reverts happen inside the reducer, which never
    /// re-enters this path — so they cannot wipe the failure message
    /// they accompany.
    func selectModel(_ id: String) {
        dispatch(.modelSelected(id))
    }

    func selectMicrophone(_ uid: String) {
        dispatch(.microphoneSelected(uid))
    }

    /// Strength slider binding setter (0 = raw microphone, 1 = fully
    /// processed). Not busy-gated: applying it is a lock-free atomic
    /// write, so the slider stays live even during a model download.
    func setIntensity(_ value: Double) {
        dispatch(.intensityChanged(value))
    }

    /// Login-item toggle binding setter. The reducer moves the toggle
    /// optimistically; the completion event snaps it back to the re-read
    /// `SMAppService` status when registration fails.
    func setLaunchAtLogin(_ enabled: Bool) {
        dispatch(.launchAtLoginToggled(enabled))
    }

    // MARK: - Reducer plumbing

    /// The single writer of `model`: reduce, publish, persist changed
    /// preferences, perform effects, then re-derive the pollers from the
    /// new state.
    private func dispatch(_ event: AppEvent) {
        let previous = model
        let (newModel, effects) = AppReducer.reduce(model, event)
        model = newModel
        persistPreferences(from: previous, event: event)
        for effect in effects {
            perform(effect)
        }
        syncFaultPolling()
        syncMonitorSafetyObservation()
    }

    private func perform(_ effect: AppEffect) {
        switch effect {
        case .stopEngine:
            engine?.stop()
            aggregate.destroy()
        case let .startEngine(attempt):
            startEngine(attempt)
        case let .setMonitor(enabled):
            setMonitor(enabled)
        case let .switchModel(id):
            switchModel(id)
        case let .setIntensity(value):
            engine?.setIntensity(Float(value))
        case let .setLaunchAtLogin(enabled):
            applyLaunchAtLogin(enabled)
        }
    }

    private func startEngine(_ attempt: StartAttempt) {
        guard
            let engine,
            let input = allDevices.first(where: { $0.uid == attempt.inputUID }),
            let virtualOutput = AudioDeviceCatalog.virtualOutput(in: allDevices)
        else {
            // The reducer pre-flighted all three against the same
            // snapshot, so this cannot happen; fail the attempt cleanly
            // rather than trap. Deferred: completions never re-enter
            // `dispatch` while an effect list is still being performed.
            Task { @MainActor in
                self.dispatch(.startCompleted(error: "The selected device disappeared"))
            }
            return
        }
        let aggregate = self.aggregate
        let modelID = attempt.modelID
        // Detached: aggregate creation polls the device until it is alive
        // (up to ~1.5 s) and engine start may download weights — neither
        // may block the main actor. The busy machine state keeps this the
        // only operation touching `aggregate`/`engine` until it finishes.
        Task.detached {
            let result = Result {
                let aggregateID = try aggregate.create(input: input, virtualOutput: virtualOutput)
                try engine.start(aggregateDevice: aggregateID, model: modelID)
            }
            await self.finish(.startCompleted(error: result.errorMessage))
        }
    }

    /// Sends the monitor toggle to the engine off the main actor,
    /// serialized like every other engine transition: the busy machine
    /// state blocks concurrent mode changes (and the pollers' engine
    /// calls) until the outcome lands, so rapid taps cannot interleave.
    private func setMonitor(_ enabled: Bool) {
        guard let engine else {
            return
        }
        Task.detached {
            let result = Result { try engine.setMonitor(enabled) }
            await self.finish(.monitorChangeCompleted(error: result.errorMessage))
        }
    }

    private func switchModel(_ id: String) {
        guard let engine else {
            return
        }
        // Detached: weight download and model construction must not run
        // on (or inherit) the main actor.
        Task.detached {
            let result = Result { try engine.setModel(id) }
            await self.finish(.modelSwitchCompleted(error: result.errorMessage))
        }
    }

    /// Whether the login item counts as on for the toggle. Registered
    /// but pending the user's consent (`requiresApproval` — common for
    /// development builds and previously denied items) reads as on: the
    /// registration itself succeeded and the remaining step lives in
    /// System Settings, so snapping the toggle off would misreport a
    /// successful attempt (and invite a pointless re-register loop).
    private static func isLoginItemRegistered(_ status: SMAppService.Status) -> Bool {
        status == .enabled || status == .requiresApproval
    }

    /// Registers or unregisters the login item off the main actor
    /// (registration can touch the service database). The completion
    /// carries the *re-read* status: registration depends on the app's
    /// location and signature (development builds under dist/ commonly
    /// fail), so the outcome — not the request — is what the reducer
    /// must show, together with the reason under the toggle: a failure,
    /// or the pending-approval notice when macOS wants the user's
    /// consent in System Settings.
    private func applyLaunchAtLogin(_ enabled: Bool) {
        Task.detached {
            var message: String?
            do {
                if enabled {
                    try SMAppService.mainApp.register()
                } else {
                    try await SMAppService.mainApp.unregister()
                }
            } catch {
                message = "Could not update the login item: \(error.localizedDescription)"
            }
            let status = SMAppService.mainApp.status
            if message == nil, enabled, status == .requiresApproval {
                message = "Registered — allow Noican in System Settings › General › Login Items to finish enabling it."
            }
            await self.finish(.launchAtLoginChangeCompleted(
                isEnabled: Self.isLoginItemRegistered(status),
                error: message
            ))
        }
    }

    /// Completion entry point for detached effect tasks.
    private func finish(_ event: AppEvent) {
        dispatch(event)
    }

    // MARK: - Preference persistence

    /// Feeds the persisted preferences back through the reducer (the
    /// restore is an event like everything else, so it stays testable).
    /// The stored model id is validated against the live registry here —
    /// the reducer has no model catalog — and enrollment-gated models
    /// are skipped like the picker skips them.
    private func restorePreferences() {
        let defaults = UserDefaults.standard
        var modelID = defaults.string(forKey: PreferenceKey.modelID)
        if !models.contains(where: { $0.id == modelID && !$0.needsEnrollment }) {
            modelID = nil
        }
        dispatch(.preferencesRestored(
            modelID: modelID,
            inputUID: defaults.string(forKey: PreferenceKey.inputUID),
            intensity: defaults.object(forKey: PreferenceKey.intensity) as? Double
        ))
    }

    /// Persists preference fields the reduce step changed. Skipped for
    /// `devicesChanged` (hot-plug reassignment of a vanished selection is
    /// not a user choice — the stored microphone must survive being
    /// temporarily unplugged across a relaunch) and for the restore
    /// event itself. Reverts (a failed model switch or microphone
    /// fallback) do persist: they record what actually runs.
    private func persistPreferences(from previous: AppModel, event: AppEvent) {
        switch event {
        case .devicesChanged, .preferencesRestored:
            return
        default:
            break
        }
        let defaults = UserDefaults.standard
        if model.selectedModelID != previous.selectedModelID {
            defaults.set(model.selectedModelID, forKey: PreferenceKey.modelID)
        }
        if model.selectedInputUID != previous.selectedInputUID {
            defaults.set(model.selectedInputUID, forKey: PreferenceKey.inputUID)
        }
        if model.intensity != previous.intensity {
            defaults.set(model.intensity, forKey: PreferenceKey.intensity)
        }
    }

    // MARK: - Device catalog

    func refreshDevices() {
        do {
            allDevices = try AudioDeviceCatalog.devices()
            let inputs = allDevices
                .filter(AudioDeviceCatalog.isSelectableInput)
                .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
                .map { device in
                    InputDevice(
                        uid: device.uid,
                        name: device.name,
                        supports48kHz: AudioDeviceCatalog.supportsSampleRate(device.id, 48_000)
                    )
                }
            dispatch(.devicesChanged(
                inputs: inputs,
                // The transport-loss check runs against every device with
                // input channels, not just the selectable ones.
                allInputUIDs: Set(allDevices.filter { $0.inputChannels > 0 }.map(\.uid)),
                isVirtualOutputPresent: AudioDeviceCatalog.virtualOutput(in: allDevices) != nil
            ))
        } catch {
            dispatch(.deviceQueryFailed(error.localizedDescription))
        }
    }

    /// Follows device hot-plug: refreshes the picker automatically (the
    /// reducer stops the engine when the microphone in use or the virtual
    /// output disappears).
    private func registerDeviceListener() {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDevices,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        _ = AudioObjectAddPropertyListenerBlock(
            AudioObjectID(kAudioObjectSystemObject),
            &address,
            DispatchQueue.main
        ) { [weak self] _, _ in
            // Delivered on the main queue, which is the main actor.
            MainActor.assumeIsolated {
                self?.refreshDevices()
                self?.refreshPreviewAvailability()
                // The monitor's target can vanish as a whole device (on
                // machines whose headphone jack is a separate device).
                self?.checkMonitorSafety()
            }
        }
    }

    // MARK: - Health polling

    /// Derives the health-poll task from the reducer state: it runs
    /// exactly while a transport is live (including the failed-but-live
    /// window after an engine fault, matching the teardown-bounded
    /// polling this replaces).
    private func syncFaultPolling() {
        if model.hasLiveTransport {
            guard faultPollTask == nil else {
                return
            }
            lastFrameCount = 0
            stalledTicks = 0
            faultPollTask = Task { [weak self] in
                while !Task.isCancelled {
                    try? await Task.sleep(for: .seconds(1))
                    self?.checkEngineHealth()
                }
            }
        } else {
            faultPollTask?.cancel()
            faultPollTask = nil
        }
    }

    private func checkEngineHealth() {
        guard model.mode != .off, !model.isBusy, let engine else {
            return
        }
        checkMonitorTrip()
        checkMonitorSafety()
        if engine.isFaulted {
            dispatch(.engineFaulted)
            return
        }
        if !engine.isRunning {
            dispatch(.engineStoppedUnexpectedly)
            return
        }
        // Heartbeat: the device must keep delivering callbacks while
        // running. Three silent seconds means it stopped (unplugged mic,
        // coreaudiod restart, post-sleep stall) without reporting an error.
        let frames = engine.framesProcessed
        if frames == lastFrameCount {
            stalledTicks += 1
            if stalledTicks >= 3 {
                dispatch(.audioStalled)
            }
        } else {
            lastFrameCount = frames
            stalledTicks = 0
        }
    }
}

// MARK: - Monitoring: meters polling and preview availability

extension AppState {
    /// Runs while the menu popover is visible (bound to the menu view's
    /// task) and stops when it closes: ~20 Hz, two non-blocking atomic
    /// reads per tick, plus a preview-availability re-check about once
    /// per second (the default-output listener misses same-device
    /// data-source flips such as the headphone jack).
    func pollLevels() async {
        refreshPreviewAvailability()
        var ticks = 0
        while !Task.isCancelled {
            refreshLevels()
            ticks += 1
            if ticks.isMultiple(of: 20) {
                refreshPreviewAvailability()
            }
            try? await Task.sleep(for: .milliseconds(50))
        }
    }

    private func refreshLevels() {
        guard model.mode != .off, let engine else {
            if inputLevel != 0 { inputLevel = 0 }
            if outputLevel != 0 { outputLevel = 0 }
            return
        }
        inputLevel = engine.inputLevel
        outputLevel = engine.outputLevel
        checkMonitorTrip()
    }

    /// Forwards the engine-side feedback killswitch to the reducer, which
    /// releases the playback device and explains why. Checked lock-free
    /// at 20 Hz while the popover is open and at 1 Hz by the health poll.
    private func checkMonitorTrip() {
        guard
            model.mode != .off, !model.isBusy, let engine,
            engine.monitorState == .tripped
        else {
            return
        }
        dispatch(.monitorTripped)
    }

    /// Maintains a shown refusal message only: once a Preview attempt was
    /// refused, re-sample the pre-flight so the reducer clears (or
    /// updates) the message live as the user fixes the output. Called
    /// from the default-output listener, device hot-plug, and popover
    /// polling.
    private func refreshPreviewAvailability() {
        guard model.messages.previewUnavailableReason != nil else {
            return
        }
        dispatch(.monitorTargetErrorChanged(RustEngine.monitorTargetError))
    }

    /// Re-vets the device the running monitor actually plays on and
    /// auto-stops the preview (via the reducer) when its safety is gone.
    /// Two loss shapes exist, machine-dependent: the same built-in device
    /// flips its data source from the headphone jack to the internal
    /// speakers (caught by the per-device listener and the health poll),
    /// or the jack is a separate device that disappears (caught by the
    /// device-list listener). `noican_monitor_target_error` cannot serve
    /// here: it judges the *current default output*, which may have moved
    /// on while the monitor stayed on the old device.
    private func checkMonitorSafety() {
        guard
            let session = model.liveSession, session.isMonitorArmed,
            !model.isBusy, let engine
        else {
            return
        }
        let device = engine.monitorDeviceID
        guard device != AudioObjectID(kAudioObjectUnknown) else {
            return
        }
        let reason: String?
        if !allDevices.contains(where: { $0.id == device }) {
            reason = "the monitor output device was disconnected; select Preview again to play on the new output"
        } else {
            reason = RustEngine.monitorDeviceError(device)
        }
        if let reason {
            dispatch(.monitorTargetBecameUnsafe(reason: reason))
        }
    }

    /// Keeps the per-device data-source listener in lockstep with the
    /// settled monitor state: registered on the monitor's device when an
    /// enable settles, and always removed the moment the monitor is no
    /// longer armed (disable claimed, trip, teardown) — a listener left
    /// behind would fire for a monitor that no longer exists.
    private func syncMonitorSafetyObservation() {
        let isArmed = model.liveSession?.isMonitorArmed == true
        if isArmed {
            guard monitorSafetyListener == nil, let engine else {
                return
            }
            let device = engine.monitorDeviceID
            guard device != AudioObjectID(kAudioObjectUnknown) else {
                return
            }
            let block: AudioObjectPropertyListenerBlock = { [weak self] _, _ in
                // Delivered on the main queue, which is the main actor.
                MainActor.assumeIsolated {
                    self?.checkMonitorSafety()
                }
            }
            var address = Self.monitorDataSourceAddress
            _ = AudioObjectAddPropertyListenerBlock(device, &address, DispatchQueue.main, block)
            monitorSafetyListener = (device, block)
        } else if let listener = monitorSafetyListener {
            var address = Self.monitorDataSourceAddress
            _ = AudioObjectRemovePropertyListenerBlock(
                listener.device,
                &address,
                DispatchQueue.main,
                listener.block
            )
            monitorSafetyListener = nil
        }
    }

    private static var monitorDataSourceAddress: AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyDataSource,
            mScope: kAudioObjectPropertyScopeOutput,
            mElement: kAudioObjectPropertyElementMain
        )
    }

    /// Follows default-output changes so a shown refusal message clears
    /// the moment the user switches to a safe device.
    private func registerDefaultOutputListener() {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDefaultOutputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        _ = AudioObjectAddPropertyListenerBlock(
            AudioObjectID(kAudioObjectSystemObject),
            &address,
            DispatchQueue.main
        ) { [weak self] _, _ in
            // Delivered on the main queue, which is the main actor.
            MainActor.assumeIsolated {
                self?.refreshPreviewAvailability()
            }
        }
    }
}

extension Result where Success == Void {
    /// The failure as a user-facing message, or nil on success — the
    /// shape completion events carry (`Result` itself is not `Equatable`).
    fileprivate var errorMessage: String? {
        if case let .failure(error) = self {
            return error.localizedDescription
        }
        return nil
    }
}
