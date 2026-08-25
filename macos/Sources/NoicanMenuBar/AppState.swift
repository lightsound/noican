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
        Task {
            let result = await Self.perform(engine) { try $0.setModel(model) }
            self.isBusy = false
            switch result {
            case .success:
                self.activeModelID = model
                self.phase = .running
            case let .failure(error):
                // The engine keeps running the previous model; keep the
                // picker truthful.
                if let activeModelID = self.activeModelID {
                    self.selectedModel = activeModelID
                }
                self.phase = .failed(error.localizedDescription)
            }
        }
    }

    func updateStatus() {
        guard isEnabled, !isBusy, let engine else {
            return
        }
        if engine.isFaulted {
            phase = .failed("Audio fault — turn noise cancellation off and on")
        } else if !engine.isRunning {
            isEnabled = false
            aggregate.destroy()
            activeModelID = nil
            phase = .failed("Engine stopped unexpectedly")
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
        isBusy = true
        phase = .busy("Starting \(displayName(for: model))…")
        do {
            let aggregateID = try aggregate.create(input: input, virtualOutput: virtualOutput)
            Task {
                let result = await Self.perform(engine) {
                    try $0.start(aggregateDevice: aggregateID, model: model)
                }
                self.isBusy = false
                switch result {
                case .success:
                    self.isEnabled = true
                    self.activeModelID = model
                    self.phase = .running
                case let .failure(error):
                    engine.stop()
                    self.aggregate.destroy()
                    self.isEnabled = false
                    self.phase = .failed(error.localizedDescription)
                }
            }
        } catch {
            aggregate.destroy()
            isBusy = false
            isEnabled = false
            phase = .failed(error.localizedDescription)
        }
    }

    private func stop() {
        engine?.stop()
        aggregate.destroy()
        isEnabled = false
        activeModelID = nil
        phase = .off
    }

    /// Runs blocking engine work (weight download, model construction, AUHAL
    /// setup) off the main actor so the menu stays responsive.
    private nonisolated static func perform(
        _ engine: RustEngine,
        _ work: @escaping @Sendable (RustEngine) throws -> Void
    ) async -> Result<Void, Error> {
        Result { try work(engine) }
    }
}
