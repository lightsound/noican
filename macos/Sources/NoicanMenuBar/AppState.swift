import Combine
import CoreAudio
import Foundation

/// Engine lifecycle as shown in the menu: drives the status dot and text.
enum EnginePhase: Equatable {
    case off
    case busy(String)
    case running
    case failed(String)
}

/// The single top-level control: Off, Preview (engine + self-monitor on
/// the default output), or On (engine only, for meetings). Preview and On
/// both feed the virtual microphone; the only difference is the monitor
/// tee, so switching between them is instant and click-free.
enum EngineMode: String, CaseIterable, Identifiable {
    case off
    case preview
    case on

    var id: String { rawValue }

    var label: String {
        switch self {
        case .off: "Off"
        case .preview: "Preview"
        case .on: "On"
        }
    }
}

@MainActor
final class AppState: ObservableObject {
    @Published private(set) var inputDevices: [AudioDeviceInfo] = []
    @Published var selectedInputUID = ""
    @Published var selectedModel = AppState.defaultModelID
    @Published private(set) var mode: EngineMode = .off
    @Published private(set) var isBusy = false
    @Published private(set) var phase: EnginePhase = .off
    /// Last preview failure (loopback/speaker refusal, feedback trip, …),
    /// shown in the monitoring section. Preview failures never affect the
    /// engine phase: the meeting-facing path keeps running.
    @Published private(set) var previewError: String?
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
        guard mode != .off, !isBusy else {
            return
        }
        if !allDevices.contains(where: { $0.uid == runningInputUID && $0.inputChannels > 0 }) {
            stopWithError("Microphone disconnected — noise cancellation stopped")
        } else if AudioDeviceCatalog.virtualOutput(in: allDevices) == nil {
            stopWithError("Virtual output device removed — noise cancellation stopped")
        }
    }

    var statusText: String {
        switch phase {
        case .off:
            return inputDevices.isEmpty ? "No input device" : "Off"
        case let .busy(message):
            return message
        case .running:
            let name = displayName(for: activeModelID ?? selectedModel)
            return mode == .preview ? "Previewing · \(name)" : "Running · \(name)"
        case let .failed(message):
            return message
        }
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
        let previous = mode
        mode = newMode
        switch (previous, newMode) {
        case (.off, _):
            start(monitor: newMode == .preview)
        case (_, .off):
            stop()
        case (.on, .preview):
            applyMonitor(true, engine: engine)
        case (.preview, .on):
            applyMonitor(false, engine: engine)
        default:
            break
        }
    }

    /// Runs while the menu popover is visible (bound to the menu view's
    /// task) and stops when it closes: ~20 Hz, two non-blocking atomic
    /// reads per tick.
    func pollLevels() async {
        while !Task.isCancelled {
            refreshLevels()
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
    /// outcome is reconciled, so rapid taps can never leave the published
    /// mode disagreeing with the actual monitor state.
    private func applyMonitor(_ enabled: Bool, engine: RustEngine) {
        isBusy = true
        phase = .busy(enabled ? "Starting preview…" : "Stopping preview…")
        Task.detached {
            let result = Result { try engine.setMonitor(enabled) }
            await self.finishMonitorChange(result, engine: engine)
        }
    }

    private func finishMonitorChange(_ result: Result<Void, Error>, engine: RustEngine) {
        isBusy = false
        guard mode != .off else {
            return
        }
        // Reconcile with the engine's actual monitor state on every
        // completion — success or failure — so the segmented control
        // never lies about what is audible.
        mode = engine.isMonitoring ? .preview : .on
        phase = .running
        if case let .failure(error) = result {
            previewError = error.localizedDescription
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
        applyMonitor(false, engine: engine)
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
            startFaultPolling()
            // Preview mode is engine + monitor; the monitor half starts
            // once the transport is up, keeping `isBusy` until its
            // outcome is reconciled.
            if monitor, let engine {
                applyMonitor(true, engine: engine)
            } else {
                isBusy = false
                phase = .running
            }
        case let .failure(error):
            isBusy = false
            engine?.stop()
            aggregate.destroy()
            mode = .off
            phase = .failed(error.localizedDescription)
        }
    }

    private func stop() {
        stopFaultPolling()
        engine?.stop()
        aggregate.destroy()
        mode = .off
        activeModelID = nil
        previewError = nil
        phase = .off
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
        stopFaultPolling()
        engine?.stop()
        aggregate.destroy()
        mode = .off
        activeModelID = nil
        previewError = nil
        phase = .failed(message)
    }
}
