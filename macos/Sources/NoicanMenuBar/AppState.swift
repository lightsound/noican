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
    @Published private(set) var phase: EnginePhase = .off {
        didSet {
            if case .busy = phase {
                return
            }
            settledPhase = phase
        }
    }
    /// The last *settled* (non-busy) phase. Sections, colors, and error
    /// text render from this instead of `phase`, so a transition that
    /// fails quickly never flashes optimistic UI (blue pill, meters
    /// sliding in, error text vanishing) before snapping back — the view
    /// changes once, when the outcome is known. `phase` still drives the
    /// spinner and the transitional status line.
    @Published private(set) var settledPhase: EnginePhase = .off
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
    /// Why the last microphone selection was refused in place (the device
    /// cannot run at 48 kHz) while the engine kept the previous one.
    /// Shown under the microphone list; cleared on the next selection.
    @Published private(set) var microphoneError: String?
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
    /// Model the running transport was started/switched to. Readable by
    /// the status projections in `AppState+Status.swift`.
    private(set) var activeModelID: String?
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
            // Property observers do not fire for direct assignments in an
            // initializer; keep the settled mirror in sync by hand.
            phase = .failed(error.localizedDescription)
            settledPhase = phase
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

    /// The mode control is intent: the system never changes it, only the
    /// user does. Tapping the already-selected segment retries when the
    /// intent is unfulfilled (e.g. the last start failed).
    func setMode(_ newMode: EngineMode) {
        guard !isBusy, let engine else {
            return
        }
        guard newMode != mode || isModeUnfulfilled else {
            return
        }
        microphoneError = nil
        // Keep a preview failure visible while a Preview retry is in
        // flight (settled-state rendering: only a settled success clears
        // it, in finishMonitorChange); leaving Preview clears it now.
        if newMode != .preview {
            previewError = nil
        }
        previewUnavailableReason = nil
        if newMode == .preview, let reason = RustEngine.monitorTargetError {
            // Refuse in place and explain: neither the mode nor the
            // engine changes, and the availability watchers clear the
            // message as soon as the output becomes safe.
            previewUnavailableReason = reason
            return
        }
        let hadError = engineErrorMessage != nil
        let previous = mode
        mode = newMode
        switch newMode {
        case .off:
            stop()
        case .on:
            if !hadError, engine.isRunning {
                // Running healthily: only the monitor half can differ.
                if previous == .preview {
                    applyMonitor(false, engine: engine)
                }
            } else {
                teardownEngine()
                start(monitor: false)
            }
        case .preview:
            if !hadError, engine.isRunning {
                applyMonitor(true, engine: engine)
            } else {
                teardownEngine()
                start(monitor: true)
            }
        }
    }

    func selectMicrophone(_ uid: String) {
        guard uid != selectedInputUID else {
            return
        }
        selectedInputUID = uid
        applySelectedInput()
    }

    private func applySelectedInput() {
        microphoneError = nil
        // Changing the microphone while Off only updates the selection;
        // clear a stale failure so the menu stops blaming the previous
        // device the moment another one is chosen.
        if mode == .off, case .failed = phase {
            phase = .off
        }
        guard mode != .off, !isBusy, engine != nil, selectedInputUID != activeInputUID else {
            return
        }
        // Pre-flight the new microphone before tearing anything down.
        if let reason = microphoneCapabilityError(for: selectedInputUID) {
            if let activeInputUID {
                // The engine keeps running on the current microphone;
                // put the checkmark back and explain under the list.
                selectedInputUID = activeInputUID
                microphoneError = reason
            } else {
                phase = .failed(reason)
            }
            return
        }
        // The private aggregate is composed around the microphone at
        // start time, so a live change rebuilds the transport with the
        // same model and mode (a brief gap is inherent). Because the mode
        // keeps the user's intent across failures, this same path
        // auto-recovers: picking a working microphone after a failed
        // start restarts straight into the selected mode.
        let monitor = mode == .preview
        teardownEngine()
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
            phase = .failed("Rust engine unavailable")
            return
        }
        guard
            let input = inputDevices.first(where: { $0.uid == selectedInputUID })
        else {
            phase = .failed("Select an input device")
            return
        }
        // Pre-flight: an incapable microphone fails here, synchronously,
        // before any busy round-trip — the reason appears without the UI
        // flashing transitional state.
        if let reason = microphoneCapabilityError(for: input.uid) {
            phase = .failed(reason)
            return
        }
        guard let virtualOutput = AudioDeviceCatalog.virtualOutput(in: allDevices) else {
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
            // outcome lands.
            if monitor, let engine {
                applyMonitor(true, engine: engine)
            } else {
                isBusy = false
                phase = .running
            }
        case let .failure(error):
            // The mode keeps the user's intent; the red pill tint and the
            // error below the control say it is not running. Retry by
            // tapping the segment again or picking another microphone.
            isBusy = false
            teardownEngine()
            phase = .failed(error.localizedDescription)
        }
    }

    private func stop() {
        teardownEngine()
        phase = .off
    }

    /// Tears the engine down without touching `mode` (the user's intent)
    /// or the messages under the mode control: user-initiated transitions
    /// clear those in `setMode`.
    private func teardownEngine() {
        stopFaultPolling()
        engine?.stop()
        aggregate.destroy()
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

    /// Runtime stops (device loss, stalls) tear the engine down but keep
    /// the mode: the pill shows what the user asked for, the red tint and
    /// the message say it is not running, and selecting another
    /// microphone (or re-tapping the segment) restarts into that mode.
    private func stopWithError(_ message: String) {
        teardownEngine()
        previewError = nil
        phase = .failed(message)
    }
}

// MARK: - Monitoring: meters polling and preview monitor control

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
        guard mode != .off, let engine else {
            if inputLevel != 0 { inputLevel = 0 }
            if outputLevel != 0 { outputLevel = 0 }
            return
        }
        inputLevel = engine.inputLevel
        outputLevel = engine.outputLevel
        checkMonitorTrip()
    }

    /// Sends the monitor toggle to the engine off the main actor,
    /// serialized like every other engine transition: `isBusy` blocks
    /// concurrent mode changes (and the pollers' engine calls) until the
    /// outcome lands, so rapid taps cannot interleave.
    private func applyMonitor(_ enabled: Bool, engine: RustEngine) {
        isBusy = true
        phase = .busy(enabled ? "Starting preview…" : "Stopping preview…")
        Task.detached {
            let result = Result { try engine.setMonitor(enabled) }
            await self.finishMonitorChange(result, enabled: enabled)
        }
    }

    private func finishMonitorChange(_ result: Result<Void, Error>, enabled: Bool) {
        isBusy = false
        guard mode != .off else {
            return
        }
        // The engine itself keeps running either way; the mode keeps the
        // user's intent. On failure the pill's warning tint plus the
        // message below the control say the preview is not playing, and
        // re-tapping Preview retries. Settled-state rendering: a kept
        // preview failure clears only when an enable actually succeeds
        // (a successful disable after a feedback trip must not erase the
        // trip's explanation).
        phase = .running
        switch result {
        case .success:
            if enabled {
                previewError = nil
            }
        case let .failure(error):
            previewError = error.localizedDescription
        }
    }

    /// Reacts to the engine-side feedback killswitch: the worker already
    /// silenced the preview, so release the playback device and tell the
    /// user why. The mode stays on Preview (it is the user's intent);
    /// the warning tint and the message say it is not playing, and
    /// re-tapping Preview retries. Checked lock-free at 20 Hz while the
    /// popover is open and at 1 Hz by the health poll.
    private func checkMonitorTrip() {
        guard mode != .off, !isBusy, let engine, engine.monitorTripped else {
            return
        }
        previewError = "Preview stopped itself: feedback detected. Use headphones, then select Preview again."
        applyMonitor(false, engine: engine)
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
