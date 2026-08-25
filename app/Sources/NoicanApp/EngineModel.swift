// Observable wrapper around the noican C ABI.
import CNoican
import Foundation

struct InputDevice: Identifiable, Decodable, Hashable {
    let uid: String
    let name: String
    var id: String { uid }
}

struct Model: Identifiable, Decodable, Hashable {
    let id: String
    let name: String
    let fetched: Bool
    let needsEnrollment: Bool
}

struct EngineStatus: Decodable {
    var running: Bool
    var model: String?
    var blocks: UInt64?
    var underruns: UInt64?
    var overruns: UInt64?
    var stageFailed: Bool?
}

/// Takes ownership of a C string returned by the library and decodes it.
private func takeString(_ pointer: UnsafeMutablePointer<CChar>?) -> String? {
    guard let pointer else { return nil }
    defer { noican_string_free(pointer) }
    return String(cString: pointer)
}

private func decodeJSON<T: Decodable>(_ type: T.Type, from string: String?) -> T? {
    guard let data = string?.data(using: .utf8) else { return nil }
    return try? JSONDecoder().decode(type, from: data)
}

@MainActor
final class EngineModel: ObservableObject {
    @Published var devices: [InputDevice] = []
    @Published var models: [Model] = []
    @Published var selectedDeviceUID: String = ""
    @Published var selectedModelID: String = "passthrough"
    @Published var isRunning = false
    @Published var statusLine = "stopped"
    @Published var lastError: String?

    private var handle: OpaquePointer?
    private var timer: Timer?

    /// Model weights directory: ~/Library/Application Support/noican/models
    /// (override with the NOICAN_MODELS_DIR environment variable).
    static func modelsDirectory() -> String {
        if let dir = ProcessInfo.processInfo.environment["NOICAN_MODELS_DIR"] {
            return dir
        }
        let base = FileManager.default.urls(
            for: .applicationSupportDirectory, in: .userDomainMask
        ).first!
        return base.appendingPathComponent("noican/models").path
    }

    init() {
        let dir = Self.modelsDirectory()
        handle = noican_new(dir)
        refreshLists()
    }

    // No deinit: this object lives for the whole app process; the engine
    // is stopped explicitly from the Quit button.

    private var rawHandle: OpaquePointer? {
        handle
    }

    func refreshLists() {
        devices = decodeJSON(
            [InputDevice].self, from: takeString(noican_list_input_devices())
        ) ?? []
        if selectedDeviceUID.isEmpty, let first = devices.first {
            selectedDeviceUID = first.uid
        }
        if let rawHandle {
            models = decodeJSON(
                [Model].self, from: takeString(noican_list_models(rawHandle))
            ) ?? []
        }
    }

    private func readError() {
        guard let rawHandle else { return }
        lastError = takeString(noican_last_error(rawHandle))
    }

    func toggle() {
        isRunning ? stop() : start()
    }

    func start() {
        guard let rawHandle else { return }
        lastError = nil
        let device = selectedDeviceUID.isEmpty ? nil : selectedDeviceUID
        let result = selectedModelID.withCString { model in
            if let device {
                device.withCString { uid in
                    noican_start(rawHandle, uid, model, nil)
                }
            } else {
                noican_start(rawHandle, nil, model, nil)
            }
        }
        if result == 0 {
            isRunning = true
            startPolling()
        } else {
            readError()
        }
    }

    func stop() {
        guard let rawHandle else { return }
        noican_stop(rawHandle)
        isRunning = false
        statusLine = "stopped"
        timer?.invalidate()
        timer = nil
    }

    func switchModel(to modelID: String) {
        selectedModelID = modelID
        guard isRunning, let rawHandle else { return }
        lastError = nil
        let result = modelID.withCString { model in
            noican_set_model(rawHandle, model, nil)
        }
        if result != 0 {
            readError()
        }
    }

    private func startPolling() {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in
            Task { @MainActor in
                self?.pollStatus()
            }
        }
    }

    private func pollStatus() {
        guard let rawHandle else { return }
        guard let status = decodeJSON(
            EngineStatus.self, from: takeString(noican_status_json(rawHandle))
        ) else { return }
        if status.running {
            var line = "running: \(status.model ?? "?")"
            if let underruns = status.underruns, underruns > 0 {
                line += " · underruns \(underruns)"
            }
            if status.stageFailed == true {
                line += " · MODEL FAILED (bypassing)"
            }
            statusLine = line
        } else {
            statusLine = "stopped"
        }
    }
}
