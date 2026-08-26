import Combine
import CoreAudio
import Foundation

@MainActor
final class AppState: ObservableObject {
    @Published private(set) var inputDevices: [AudioDeviceInfo] = []
    @Published var selectedInputUID = ""
    @Published var selectedModel = AppState.defaultModelID
    @Published private(set) var mode: EngineMode = .off
    @Published private(set) var isBusy = false
    @Published private(set) var phase: EnginePhase = .off
    /// Last preview failure (start failure, feedback trip, …), shown
    /// under the mode control. Preview failures never affect the engine
    /// phase: the meeting-facing path keeps running.
    @Published private(set) var previewError: String?
    /// Why the last Preview attempt was refused (unsafe default output),
    /// shown under the mode control until the user fixes the output —
    /// the availability watchers clear it live — or moves on. Nil until
    /// Preview is actually pressed: an unavailable Preview stays
    /// pressable and explains itself on press, which confuses less than
    /// a segment that cannot be pressed.
    @Published private(set) var previewUnavailableReason: String?
    /// Peak meters, refreshed by `pollLevels()` only while the popover is
    /// open. Independent of the Preview state: they move whenever the
    /// engine runs.
    @Published private(set) var inputLevel: Float = 0
    @Published private(set) var outputLevel: Float = 0

    /// Selectable models, read from the Rust registry at launch.
    let models = RustEngine.models()

    private static let defaultModelID = "fastenhancer-b"

    private var allDevices: [AudioDeviceInfo] = []
    private let aggregate = AggregateDevice()
    private var engine: RustEngine?
    private var activeModelID: String?
    /// Microphone the running transport was built around (the aggregate
    /// is composed at start time, so a live change means a rebuild).
    private var activeInputUID: String?
    /// Watches for engine faults/unexpected stops while enabled. Owned here
    /// (not by the menu view) so faults are detected even when the popover
    /// is closed.
    private var faultPollTask: Task<Void, Never>?
    /// Heartbeat state for detecting a device that stopped calling back.
    private var lastFrameCount: UInt64 = 0
    private var stalledTicks = 0

    init() {
        do {
            engine = try RustEngine()
            refreshDevices()
        } catch {
            phase = .failed(error.localizedDescription)
        }
        if !models.contains(where: { $0.id == selectedModel }) {
            selectedModel = models.first?.id ?? ""
        }
        registerDeviceListener()
        registerDefaultOutputListener()
    }

    /// Follows device hot-plug: refreshes the picker automatically and stops
    /// the engine with a clear message when the microphone in use (or the
    /// virtual output) disappears.
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
                self?.handleDevicesChanged()
            }
        }
    }

    private func handleDevicesChanged() {
        let runningInputUID = selectedInputUID
        refreshDevices()
        refreshPreviewAvailability()
        guard mode != .off, !isBusy else {
            return
        }
        if !allDevices.contains(where: { $0.uid == runningInputUID && $0.inputChannels > 0 }) {
            stopWithError("Microphone disconnected — noise cancellation stopped")
        } else if AudioDeviceCatalog.virtualOutput(in: allDevices) == nil {
            stopWithError("Virtual output device removed — noise cancellation stopped")
        }
    }

    /// One-line status for the header. Deliberately never multi-line:
    /// text above the mode control must not change the control's
    /// vertical position (the sliding pill would move mid-animation).
    /// Full failure text lives in `engineErrorMessage`, shown below the
    /// control instead.
    var statusText: String {
        switch phase {
        case .off:
            return inputDevices.isEmpty ? "No input device" : "Off"
        case let .busy(message):
            return message
        case .running:
            let name = displayName(for: activeModelID ?? selectedModel)
            return mode == .preview ? "Previewing · \(name)" : "Running · \(name)"
        case .failed:
            return "Error"
        }
    }

    /// Full engine failure text, displayed under the mode control (with
    /// the preview messages) so the header height stays constant.
    var engineErrorMessage: String? {
        if case let .failed(message) = phase {
            return message
        }
        return nil
    }

    func displayName(for modelID: String) -> String {
        models.first { $0.id == modelID }?.displayName ?? modelID
    }

    func refreshDevices() {
        do {
            allDevices = try AudioDeviceCatalog.devices()
            inputDevices = allDevices
                .filter(AudioDeviceCatalog.isSelectableInput)
                .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
            if !inputDevices.contains(where: { $0.uid == selectedInputUID }) {
                selectedInputUID = inputDevices.first?.uid ?? ""
            }
        } catch {
            phase = .failed(error.localizedDescription)
        }
    }

    func setMode(_ newMode: EngineMode) {
        guard !isBusy, newMode != mode, let engine else {
            return
        }
        previewError = nil
        previewUnavailableReason = nil
        if newMode == .preview, let reason = RustEngine.monitorTargetError {
            // Refuse in place and explain: neither the mode nor the
            // engine changes, and the availability watchers clear the
            // message as soon as the output becomes safe.
            previewUnavailableReason = reason
            return
        }
        let previous = mode
        mode = newMode
        switch (previous, newMode) {
        case (.off, _):
            start(monitor: newMode == .preview)
        case (_, .off):
            stop()
        case (.on, .preview):
            applyMonitor(true, engine: engine, fallback: .on)
        case (.preview, .on):
            applyMonitor(false, engine: engine, fallback: .preview)
        default:
            break
        }
    }

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
        guard mode != .off, let engine else {
            if inputLevel != 0 { inputLevel = 0 }
            if outputLevel != 0 { outputLevel = 0 }
            return
        }
        inputLevel = engine.inputLevel
        outputLevel = engine.outputLevel
        checkMonitorTrip()
    }

    func selectMicrophone(_ uid: String) {
        guard uid != selectedInputUID else {
            return
        }
        selectedInputUID = uid
        applySelectedInput()
    }

    private func applySelectedInput() {
        // Changing the microphone while stopped only updates the
        // selection; clear a stale failure (e.g. a 48 kHz-incapable
        // Bluetooth microphone) so the menu stops blaming the previous
        // device the moment another one is chosen.
        if mode == .off, case .failed = phase {
            phase = .off
        }
        guard mode != .off, !isBusy, let engine, selectedInputUID != activeInputUID else {
            return
        }
        // The private aggregate is composed around the microphone at
        // start time, so a live change rebuilds the transport with the
        // same model and mode (a brief gap is inherent).
        let monitor = mode == .preview
        stopFaultPolling()
        engine.stop()
        aggregate.destroy()
        start(monitor: monitor)
    }

    func applySelectedModel() {
        // Changing the model while stopped starts nothing; clear a stale
        // failure message so the menu does not keep blaming the last
        // attempt.
        if mode == .off, case .failed = phase {
            phase = .off
        }
        guard mode != .off, !isBusy, let engine, selectedModel != activeModelID else {
            return
        }
        let model = selectedModel
        isBusy = true
        phase = .busy("Loading \(displayName(for: model))…")
        // Detached: weight download and model construction must not run on
        // (or inherit) the main actor.
        Task.detached {
            let result = Result { try engine.setModel(model) }
            await self.finishModelSwitch(result, model: model)
        }
    }

    private func finishModelSwitch(_ result: Result<Void, Error>, model: String) {
        isBusy = false
        switch result {
        case .success:
            activeModelID = model
            phase = .running
        case let .failure(error):
            // The engine keeps running the previous model; keep the picker
            // truthful.
            if let activeModelID {
                selectedModel = activeModelID
            }
            phase = .failed(error.localizedDescription)
        }
    }

    private func start(monitor: Bool) {
        guard let engine else {
            mode = .off
            phase = .failed("Rust engine unavailable")
            return
        }
        guard
            let input = inputDevices.first(where: { $0.uid == selectedInputUID })
        else {
            mode = .off
            phase = .failed("Select an input device")
            return
        }
        guard let virtualOutput = AudioDeviceCatalog.virtualOutput(in: allDevices) else {
            mode = .off
            phase = .failed("Install the Noican or BlackHole virtual device")
            return
        }
        let model = selectedModel
        let aggregate = self.aggregate
        isBusy = true
        phase = .busy("Starting \(displayName(for: model))…")
        // Detached: aggregate creation polls the device until it is alive
        // (up to ~1.5 s) and engine start may download weights — neither may
        // block the main actor. `isBusy` keeps this the only operation
        // touching `aggregate`/`engine` until it finishes.
        Task.detached {
            let result = Result {
                let aggregateID = try aggregate.create(input: input, virtualOutput: virtualOutput)
                try engine.start(aggregateDevice: aggregateID, model: model)
            }
            await self.finishStart(result, model: model, monitor: monitor)
        }
    }

    private func finishStart(_ result: Result<Void, Error>, model: String, monitor: Bool) {
        switch result {
        case .success:
            activeModelID = model
            activeInputUID = selectedInputUID
            startFaultPolling()
            // Preview mode is engine + monitor; the monitor half starts
            // once the transport is up, keeping `isBusy` until its
            // outcome is reconciled. A failure rolls back to Off — the
            // engine was started only for this preview.
            if monitor, let engine {
                applyMonitor(true, engine: engine, fallback: .off)
            } else {
                isBusy = false
                phase = .running
            }
        case let .failure(error):
            isBusy = false
            stopEngine()
            phase = .failed(error.localizedDescription)
        }
    }

    private func stop() {
        stopEngine()
        phase = .off
    }

    /// Tears the engine down and returns the mode to Off. Deliberately
    /// leaves `previewError` alone: user-initiated transitions clear it
    /// in `setMode`, while a preview rollback keeps its reason visible.
    private func stopEngine() {
        stopFaultPolling()
        engine?.stop()
        aggregate.destroy()
        mode = .off
        activeModelID = nil
        activeInputUID = nil
    }

    private func startFaultPolling() {
        faultPollTask?.cancel()
        lastFrameCount = 0
        stalledTicks = 0
        faultPollTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                self?.checkEngineHealth()
            }
        }
    }

    private func stopFaultPolling() {
        faultPollTask?.cancel()
        faultPollTask = nil
    }

    private func checkEngineHealth() {
        guard mode != .off, !isBusy, let engine else {
            return
        }
        checkMonitorTrip()
        if engine.isFaulted {
            phase = .failed("Audio fault — turn noise cancellation off and on")
            return
        }
        if !engine.isRunning {
            stopWithError("Engine stopped unexpectedly")
            return
        }
        // Heartbeat: the device must keep delivering callbacks while
        // running. Three silent seconds means it stopped (unplugged mic,
        // coreaudiod restart, post-sleep stall) without reporting an error.
        let frames = engine.framesProcessed
        if frames == lastFrameCount {
            stalledTicks += 1
            if stalledTicks >= 3 {
                stopWithError("Audio stalled — device lost or audio system restarted")
            }
        } else {
            lastFrameCount = frames
            stalledTicks = 0
        }
    }

    private func stopWithError(_ message: String) {
        stopEngine()
        previewError = nil
        phase = .failed(message)
    }
}

// MARK: - Preview monitor control

extension AppState {
    /// Sends the monitor toggle to the engine off the main actor,
    /// serialized like every other engine transition: `isBusy` blocks
    /// concurrent mode changes (and the pollers' engine calls) until the
    /// outcome is reconciled, so rapid taps can never leave the published
    /// mode disagreeing with the actual monitor state.
    ///
    /// `fallback` is the pre-transition mode to return to when the
    /// toggle fails: `.on` keeps the engine running, `.off` rolls the
    /// whole start back (the engine was started only for this preview).
    private func applyMonitor(_ enabled: Bool, engine: RustEngine, fallback: EngineMode) {
        isBusy = true
        phase = .busy(enabled ? "Starting preview…" : "Stopping preview…")
        Task.detached {
            let result = Result { try engine.setMonitor(enabled) }
            await self.finishMonitorChange(result, engine: engine, fallback: fallback)
        }
    }

    private func finishMonitorChange(
        _ result: Result<Void, Error>,
        engine: RustEngine,
        fallback: EngineMode
    ) {
        isBusy = false
        guard mode != .off else {
            return
        }
        switch result {
        case .success:
            // Reconcile with the engine's actual monitor state so the
            // mode control never lies about what is audible.
            mode = engine.isMonitoring ? .preview : .on
            phase = .running
        case let .failure(error):
            previewError = error.localizedDescription
            if fallback == .off {
                // Return to exactly the pre-transition state; the reason
                // stays visible under the mode control.
                stopEngine()
                phase = .off
            } else {
                mode = engine.isMonitoring ? .preview : fallback
                phase = .running
            }
        }
    }

    /// Reacts to the engine-side feedback killswitch: the worker already
    /// silenced the preview, so release the playback device and tell the
    /// user why (the mode falls back to On when the monitor is torn
    /// down). Checked lock-free at 20 Hz while the popover is open and at
    /// 1 Hz by the health poll, in any running mode — a trip is handled
    /// even if a transition already moved the mode off Preview.
    private func checkMonitorTrip() {
        guard mode != .off, !isBusy, let engine, engine.monitorTripped else {
            return
        }
        previewError = "Preview stopped itself: feedback detected. Use headphones, then select Preview again."
        applyMonitor(false, engine: engine, fallback: .on)
    }

    /// Maintains a shown refusal message only: once a Preview attempt was
    /// refused, re-check the default output so the message clears (or
    /// updates) live as the user fixes it. Called from the default-output
    /// listener, device hot-plug, and popover polling.
    private func refreshPreviewAvailability() {
        guard previewUnavailableReason != nil else {
            return
        }
        let reason = RustEngine.monitorTargetError
        if reason != previewUnavailableReason {
            previewUnavailableReason = reason
        }
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
