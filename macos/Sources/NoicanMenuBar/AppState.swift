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
                .filter { $0.inputChannels > 0 }
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

    func applySelectedModel() {
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
            phase = .failed("Install the noican or BlackHole virtual device")
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
        phase = .off
    }

    private func startFaultPolling() {
        faultPollTask?.cancel()
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
        } else if !engine.isRunning {
            stopFaultPolling()
            isEnabled = false
            aggregate.destroy()
            activeModelID = nil
            phase = .failed("Engine stopped unexpectedly")
        }
    }
}
