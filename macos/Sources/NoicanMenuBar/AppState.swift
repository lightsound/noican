import Combine
import Foundation

@MainActor
final class AppState: ObservableObject {
    @Published private(set) var inputDevices: [AudioDeviceInfo] = []
    @Published var selectedInputUID = ""
    @Published var selectedModel = "fastenhancer-b"
    @Published private(set) var isEnabled = false
    @Published private(set) var status = "Off"

    let models = RustEngine.modelSlugs()

    private var allDevices: [AudioDeviceInfo] = []
    private let aggregate = AggregateDevice()
    private var engine: RustEngine?

    init() {
        do {
            engine = try RustEngine()
            refreshDevices()
        } catch {
            status = error.localizedDescription
        }
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
            if !isEnabled {
                status = inputDevices.isEmpty ? "No input device" : "Off"
            }
        } catch {
            status = error.localizedDescription
        }
    }

    func setEnabled(_ enabled: Bool) {
        if enabled {
            start()
        } else {
            stop()
        }
    }

    func applySelectedModel() {
        guard isEnabled, let engine else {
            return
        }
        do {
            status = "Loading \(selectedModel)…"
            try engine.setModel(selectedModel)
            status = "Running · \(selectedModel)"
        } catch {
            status = error.localizedDescription
        }
    }

    func updateStatus() {
        guard isEnabled, let engine else {
            return
        }
        if engine.isFaulted {
            status = "Audio fault · restart required"
        } else if !engine.isRunning {
            status = "Stopped unexpectedly"
            isEnabled = false
            aggregate.destroy()
        }
    }

    private func start() {
        guard let engine else {
            status = "Rust engine unavailable"
            return
        }
        guard
            let input = inputDevices.first(where: { $0.uid == selectedInputUID })
        else {
            status = "Select an input device"
            return
        }
        guard let virtualOutput = AudioDeviceCatalog.virtualOutput(in: allDevices) else {
            status = "Install the noican or BlackHole virtual device"
            return
        }
        do {
            status = "Starting…"
            let aggregateID = try aggregate.create(input: input, virtualOutput: virtualOutput)
            try engine.start(aggregateDevice: aggregateID, model: selectedModel)
            isEnabled = true
            status = "Running · \(selectedModel)"
        } catch {
            engine.stop()
            aggregate.destroy()
            isEnabled = false
            status = error.localizedDescription
        }
    }

    private func stop() {
        engine?.stop()
        aggregate.destroy()
        isEnabled = false
        status = "Off"
    }
}
