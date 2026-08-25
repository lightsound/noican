import CNoican
import Foundation
import Observation

/// The Swift side of the Rust engine.
///
/// Everything the UI touches lives here, on the main actor. The Rust handle is
/// not thread-safe by design — the engine's own control plane assumes a single
/// caller — so confining it to the main actor is what makes that safe rather
/// than merely convenient.
@Observable
@MainActor
final class EngineController {
    /// A model the user can pick.
    struct Model: Identifiable, Hashable {
        let id: String
        let displayName: String
        let sampleRate: UInt32
        var downloaded: Bool
        /// False for models that can only run a block at a time.
        let liveCapable: Bool

        /// Label for the picker, marking anything the user should know before
        /// choosing it. A block-stage model buys several seconds of latency, so
        /// picking one unwarned looks exactly like the app hanging.
        var menuLabel: String {
            var label = displayName
            if !liveCapable {
                label += " — offline only, seconds of latency"
            }
            if !downloaded {
                label += " — not downloaded"
            }
            return label
        }
    }

    /// An audio device the user can pick.
    struct Device: Identifiable, Hashable {
        /// The persistent UID, which is what gets remembered.
        let id: String
        let name: String
        let sampleRate: UInt32
        let isVirtual: Bool
    }

    /// What the engine is doing, refreshed on a timer while running.
    ///
    /// Deliberately excludes the level meters. This value is observed by the
    /// menu bar label, so anything that changes at meter rate would re-render
    /// the label ten times a second even while the popover is closed.
    struct Status: Equatable {
        var running = false
        var bypassed = false
        var switching = false
        var dropouts: UInt64 = 0
        var latencyMilliseconds: Float = 0
    }

    private(set) var models: [Model] = []
    private(set) var inputDevices: [Device] = []
    private(set) var outputDevices: [Device] = []
    private(set) var status = Status()
    private(set) var lastError: String?

    /// Peak levels, kept apart from `status` so only the meter views observe
    /// them. See `Status` for why that separation matters.
    let meters = MeterModel()

    /// UID of the microphone to capture. Persisted across launches.
    var selectedInputUID: String? {
        didSet { defaults.set(selectedInputUID, forKey: Keys.inputUID) }
    }

    /// UID of the virtual device to feed. Persisted across launches.
    var selectedOutputUID: String? {
        didSet { defaults.set(selectedOutputUID, forKey: Keys.outputUID) }
    }

    /// The active model. Changing it while running switches without a click.
    var selectedModelID: String {
        didSet {
            defaults.set(selectedModelID, forKey: Keys.modelID)
            guard status.running, selectedModelID != oldValue else { return }
            applyModel(selectedModelID)
        }
    }

    private enum Keys {
        static let inputUID = "noican.inputUID"
        static let outputUID = "noican.outputUID"
        static let modelID = "noican.modelID"
    }

    private let defaults = UserDefaults.standard
    // `nonisolated(unsafe)` so `deinit`, which is not main-actor isolated, can
    // release it. The pointer is immutable and only ever passed back to Rust.
    nonisolated(unsafe) private let engine: OpaquePointer?
    private var refreshTask: Task<Void, Never>?

    init() {
        noican_init_logging()
        engine = noican_engine_new()
        selectedModelID = defaults.string(forKey: Keys.modelID) ?? ""
        selectedInputUID = defaults.string(forKey: Keys.inputUID)
        selectedOutputUID = defaults.string(forKey: Keys.outputUID)
        reloadCatalog()
        reloadDevices()
    }

    deinit {
        // `deinit` cannot touch main-actor state, so the handle is released
        // through the nonisolated free function.
        if let engine {
            noican_engine_free(engine)
        }
    }

    // MARK: - Catalog and devices

    /// Re-reads the model catalog and which weights are present.
    func reloadCatalog() {
        let count = noican_models(nil, 0)
        guard count > 0 else {
            models = []
            return
        }
        var buffer = [NoicanModel](repeating: NoicanModel(), count: count)
        let written = noican_models(&buffer, count)
        models = buffer.prefix(written).map { entry in
            var entry = entry
            return Model(
                id: readFixedString(&entry.id),
                displayName: readFixedString(&entry.display_name),
                sampleRate: entry.sample_rate,
                downloaded: entry.downloaded,
                liveCapable: entry.live_capable
            )
        }
        if selectedModelID.isEmpty || !models.contains(where: { $0.id == selectedModelID }) {
            // Prefer something already downloaded so the first launch works.
            selectedModelID = models.first(where: \.downloaded)?.id ?? models.first?.id ?? ""
        }
    }

    /// Re-reads the device lists and fills in any missing selection.
    func reloadDevices() {
        inputDevices = readDevices(noican_input_devices)
        outputDevices = readDevices(noican_output_devices)

        // A remembered device can disappear between launches, so a selection
        // that no longer matches anything has to be replaced rather than kept.
        let inputIsStale = !inputDevices.contains { $0.id == selectedInputUID }
        if selectedInputUID == nil || inputIsStale {
            selectedInputUID = readString(noican_default_input_uid) ?? inputDevices.first?.id
        }
        let outputIsStale = !outputDevices.contains { $0.id == selectedOutputUID }
        if selectedOutputUID == nil || outputIsStale {
            selectedOutputUID = readString(noican_suggested_output_uid)
        }
    }

    /// Downloads the weights for `modelID` off the main thread.
    func fetchModel(_ modelID: String) async {
        let ok = await Task.detached(priority: .utility) {
            modelID.withCString { noican_fetch_model($0) }
        }.value
        if ok {
            lastError = nil
        } else {
            lastError = Self.lastEngineError()
        }
        reloadCatalog()
    }

    // MARK: - Lifecycle

    /// Whether the engine has everything it needs to start.
    var canStart: Bool {
        guard let input = selectedInputUID, let output = selectedOutputUID else { return false }
        guard !input.isEmpty, !output.isEmpty, input != output else { return false }
        return models.first { $0.id == selectedModelID }?.downloaded == true
    }

    /// Starts or stops the audio path.
    func setRunning(_ shouldRun: Bool) {
        if shouldRun {
            start()
        } else {
            stop()
        }
    }

    private func start() {
        guard let engine,
              let input = selectedInputUID,
              let output = selectedOutputUID
        else { return }

        let ok = input.withCString { inputPointer in
            output.withCString { outputPointer in
                selectedModelID.withCString { modelPointer in
                    noican_engine_start(
                        engine,
                        inputPointer,
                        outputPointer,
                        modelPointer
                    )
                }
            }
        }
        if ok {
            lastError = nil
            startRefreshing()
        } else {
            lastError = Self.lastEngineError()
        }
        refreshStatus()
    }

    private func stop() {
        guard let engine else { return }
        noican_engine_stop(engine)
        refreshTask?.cancel()
        refreshTask = nil
        refreshStatus()
    }

    /// Bypasses or re-enables the active model.
    func setBypass(_ bypassed: Bool) {
        guard let engine else { return }
        noican_engine_set_bypass(engine, bypassed)
        refreshStatus()
    }

    private func applyModel(_ modelID: String) {
        guard let engine else { return }
        let ok = modelID.withCString { pointer in
            noican_engine_set_model(engine, pointer)
        }
        lastError = ok ? nil : Self.lastEngineError()
    }

    // MARK: - Status

    /// Polls the engine while it runs.
    ///
    /// Ten hertz: fast enough for level meters to look alive, slow enough that
    /// the peak-and-clear meters in the engine still report something useful.
    private func startRefreshing() {
        refreshTask?.cancel()
        refreshTask = Task { [weak self] in
            while !Task.isCancelled {
                self?.refreshStatus()
                try? await Task.sleep(for: .milliseconds(100))
            }
        }
    }

    private func refreshStatus() {
        guard let engine else { return }
        var raw = NoicanStatus()
        noican_engine_status(engine, &raw)

        meters.inputPeak = raw.input_peak
        meters.outputPeak = raw.output_peak

        // Assign only on a real change: `status` drives the menu bar label, and
        // an unconditional write would invalidate it on every poll.
        let updated = Status(
            running: raw.running,
            bypassed: raw.bypassed,
            switching: raw.switching,
            dropouts: raw.dropouts,
            latencyMilliseconds: raw.latency_ms
        )
        if updated != status {
            status = updated
        }
    }

    // MARK: - Bridging helpers

    private func readDevices(
        _ list: (UnsafeMutablePointer<NoicanDevice>?, Int) -> Int
    ) -> [Device] {
        let count = list(nil, 0)
        guard count > 0 else { return [] }
        var buffer = [NoicanDevice](repeating: NoicanDevice(), count: count)
        let written = list(&buffer, count)
        return buffer.prefix(written).map { entry in
            var entry = entry
            return Device(
                id: readFixedString(&entry.uid),
                name: readFixedString(&entry.name),
                sampleRate: entry.sample_rate,
                isVirtual: entry.is_virtual
            )
        }
    }

    private func readString(_ read: (UnsafeMutablePointer<CChar>?) -> Bool) -> String? {
        var buffer = [CChar](repeating: 0, count: Int(NOICAN_STRING_CAPACITY))
        guard read(&buffer) else { return nil }
        let value = String(cString: buffer)
        return value.isEmpty ? nil : value
    }

    private static func lastEngineError() -> String {
        let message = String(cString: noican_last_error())
        return message.isEmpty ? "unknown error" : message
    }
}

/// Reads a fixed-size C string field out of an ABI struct.
///
/// The fields arrive as tuples of `CChar` because that is how Swift imports a C
/// array, so they have to be read through a pointer to the whole tuple.
private func readFixedString<T>(_ field: inout T) -> String {
    withUnsafePointer(to: &field) { pointer in
        pointer.withMemoryRebound(
            to: CChar.self,
            capacity: Int(NOICAN_STRING_CAPACITY)
        ) { rebound in
            String(cString: rebound)
        }
    }
}
