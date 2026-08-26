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

@MainActor
final class AppState: ObservableObject {
    @Published private(set) var inputDevices: [AudioDeviceInfo] = []
    @Published var selectedInputUID = ""
    @Published var selectedModel = AppState.defaultModelID
    @Published private(set) var isEnabled = false
    @Published private(set) var isBusy = false
    @Published private(set) var phase: EnginePhase = .off
    /// User preference for the preview self-monitor. Survives engine
    /// off/on: the preview is re-applied automatically after a successful
    /// start (the Rust monitor itself does not outlive the transport).
    @Published private(set) var isPreviewEnabled = false
    /// Last preview failure, shown under the Preview toggle. Preview
    /// failures never affect the engine phase: the meeting-facing path
    /// keeps running.
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
        guard isEnabled, !isBusy else {
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
            return "Running · \(displayName(for: activeModelID ?? selectedModel))"
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

    func setEnabled(_ enabled: Bool) {
        guard !isBusy else {
            return
        }
        if enabled {
            start()
        } else {
            stop()
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
        guard isEnabled, let engine else {
            if inputLevel != 0 { inputLevel = 0 }
            if outputLevel != 0 { outputLevel = 0 }
            return
        }
        inputLevel = engine.inputLevel
        outputLevel = engine.outputLevel
    }

    func setPreview(_ enabled: Bool) {
        guard isEnabled, !isBusy, let engine else {
            return
        }
        previewError = nil
        isPreviewEnabled = enabled
        applyMonitor(enabled, engine: engine)
    }

    /// Sends the monitor toggle to the engine off the main actor and
    /// reconciles the published state with the actual outcome.
    private func applyMonitor(_ enabled: Bool, engine: RustEngine) {
        Task.detached {
            let result = Result { try engine.setMonitor(enabled) }
            await self.finishPreviewChange(result, engine: engine)
        }
    }

    private func finishPreviewChange(_ result: Result<Void, Error>, engine: RustEngine) {
        if case let .failure(error) = result {
            isPreviewEnabled = engine.isMonitoring
            previewError = error.localizedDescription
        }
    }

    func applySelectedModel() {
        // Changing the model while stopped starts nothing; clear a stale
        // failure message so the menu does not keep blaming the last
        // attempt.
        if !isEnabled, case .failed = phase {
            phase = .off
        }
        guard isEnabled, !isBusy, let engine, selectedModel != activeModelID else {
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

    private func start() {
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
            await self.finishStart(result, model: model)
        }
    }

    private func finishStart(_ result: Result<Void, Error>, model: String) {
        isBusy = false
        switch result {
        case .success:
            isEnabled = true
            activeModelID = model
            phase = .running
            startFaultPolling()
            // The Rust monitor does not survive an engine restart;
            // re-apply the user's preview preference.
            if isPreviewEnabled, let engine {
                applyMonitor(true, engine: engine)
            }
        case let .failure(error):
            engine?.stop()
            aggregate.destroy()
            isEnabled = false
            phase = .failed(error.localizedDescription)
        }
    }

    private func stop() {
        stopFaultPolling()
        engine?.stop()
        aggregate.destroy()
        isEnabled = false
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
        guard isEnabled, !isBusy, let engine else {
            return
        }
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
        isEnabled = false
        activeModelID = nil
        previewError = nil
        phase = .failed(message)
    }
}
