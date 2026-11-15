import CNoican
import CoreAudio
import Foundation
import NoicanState

struct ModelRatings: Hashable {
    let noiseRemoval: Int
    let voiceQuality: Int
    let responsiveness: Int
    let efficiency: Int
}

struct ModelInfo: Hashable, Identifiable {
    let id: String
    let displayName: String
    let tagline: String
    let details: String
    let ratings: ModelRatings
}

final class RustEngine: @unchecked Sendable {
    private let handle: UnsafeMutableRawPointer

    init() throws {
        guard let handle = noican_engine_create(nil) else {
            throw RustEngineError.message("Could not create the Rust engine")
        }
        self.handle = handle
    }

    deinit {
        noican_engine_destroy(handle)
    }

    func start(_ aggregate: AggregateComposition, model: String) throws {
        let layout = aggregate.virtualOutputChannels
        let result = model.withCString { id in
            noican_engine_start(
                handle, aggregate.deviceID, layout.firstChannel, layout.channelCount, id
            )
        }
        try requireSuccess(result)
    }

    func startNative(
        inputDevice: AudioObjectID,
        virtualOutput: AudioObjectID,
        captureRate: Double,
        model: String
    ) throws {
        let result = model.withCString { id in
            noican_engine_start_native(handle, inputDevice, virtualOutput, captureRate, id)
        }
        try requireSuccess(result)
    }

    func stop() {
        noican_engine_stop(handle)
    }

    func setModel(_ model: String) throws {
        let result = model.withCString { id in
            noican_engine_set_model(handle, id)
        }
        try requireSuccess(result)
        resetDebugStats()
    }

    func setMonitor(_ enabled: Bool) throws {
        try requireSuccess(noican_engine_set_monitor(handle, enabled ? 1 : 0))
    }

    var monitorState: MonitorState {
        MonitorState(rawValue: noican_engine_monitor_state(handle)) ?? .off
    }

    static var monitorTargetError: String? {
        copyString { buffer, capacity in
            noican_monitor_target_error(buffer, capacity)
        }
    }

    var monitorDeviceID: UInt32 {
        noican_engine_monitor_device(handle)
    }

    static func monitorDeviceError(_ deviceID: UInt32) -> String? {
        copyString { buffer, capacity in
            noican_monitor_device_error(deviceID, buffer, capacity)
        }
    }

    var inputLevel: Float {
        noican_engine_input_level(handle)
    }

    var outputLevel: Float {
        noican_engine_output_level(handle)
    }

    func setIntensity(_ value: Float) {
        _ = noican_engine_set_intensity(handle, value)
    }

    var isRunning: Bool {
        noican_engine_is_running(handle) != 0
    }

    var isFaulted: Bool {
        noican_engine_is_faulted(handle) != 0
    }

    var framesProcessed: UInt64 {
        noican_engine_frames_processed(handle)
    }

    var outputUnderruns: UInt64 {
        noican_engine_output_underruns(handle)
    }

    var inputOverruns: UInt64 {
        noican_engine_input_overruns(handle)
    }

    var workerBlocks: UInt64 {
        noican_engine_worker_blocks(handle)
    }

    var workerBlocksOverBudget: UInt64 {
        noican_engine_worker_blocks_over_budget(handle)
    }

    var workerBlockMaxNs: UInt64 {
        noican_engine_worker_block_max_ns(handle)
    }

    var workerRealtime: Bool {
        noican_engine_worker_realtime(handle) != 0
    }

    func resetDebugStats() {
        noican_engine_reset_debug_stats(handle)
    }

    var routingDescription: String? {
        Self.copyString { buffer, capacity in
            noican_engine_routing_description(handle, buffer, capacity)
        }
    }

    static func models() -> [ModelInfo] {
        (0..<noican_model_count()).compactMap { index in
            guard
                let id = copyString({ buffer, capacity in
                    noican_model_id(index, buffer, capacity)
                }),
                let displayName = copyString({ buffer, capacity in
                    noican_model_display_name(index, buffer, capacity)
                })
            else {
                return nil
            }
            return ModelInfo(
                id: id,
                displayName: displayName,
                tagline: copyString { buffer, capacity in
                    noican_model_tagline(index, buffer, capacity)
                } ?? "",
                details: copyString { buffer, capacity in
                    noican_model_details(index, buffer, capacity)
                } ?? "",
                ratings: ModelRatings(
                    noiseRemoval: rating(index, NOICAN_TRAIT_NOISE_REMOVAL),
                    voiceQuality: rating(index, NOICAN_TRAIT_VOICE_QUALITY),
                    responsiveness: rating(index, NOICAN_TRAIT_RESPONSIVENESS),
                    efficiency: rating(index, NOICAN_TRAIT_EFFICIENCY)
                )
            )
        }
    }

    private static func rating(_ index: Int, _ trait: NoicanModelTrait) -> Int {
        max(0, Int(noican_model_rating(index, Int32(trait.rawValue))))
    }

    private var lastError: String {
        Self.copyString { buffer, capacity in
            noican_engine_last_error(handle, buffer, capacity)
        } ?? "Unknown Rust engine error"
    }

    private func requireSuccess(_ result: Int32) throws {
        guard result == 0 else {
            throw RustEngineError.message(lastError)
        }
    }

    private static func copyString(
        _ copy: (UnsafeMutablePointer<CChar>?, Int) -> Int
    ) -> String? {
        let required = copy(nil, 0)
        guard required > 1 else {
            return nil
        }
        var bytes = [CChar](repeating: 0, count: required)
        let reported = bytes.withUnsafeMutableBufferPointer { buffer in
            copy(buffer.baseAddress, buffer.count)
        }
        guard reported == required else {
            return nil
        }
        return bytes.withUnsafeBufferPointer { buffer in
            guard let baseAddress = buffer.baseAddress else {
                return nil
            }
            return String(cString: baseAddress)
        }
    }
}

enum MonitorState: Int32 {
    case off = 0
    case playing = 1
    case tripped = 2
}

enum RustEngineError: LocalizedError {
    case message(String)

    var errorDescription: String? {
        switch self {
        case let .message(message):
            message
        }
    }
}
