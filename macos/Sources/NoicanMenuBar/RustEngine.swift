import CNoican
import CoreAudio
import Foundation
import NoicanState

struct ModelRatings: Hashable {
    let noiseRemoval: Int
    let voiceQuality: Int
    let responsiveness: Int
    let efficiency: Int

    init(_ ratings: NoicanModelRatings) {
        noiseRemoval = Int(ratings.noise_removal)
        voiceQuality = Int(ratings.voice_quality)
        responsiveness = Int(ratings.responsiveness)
        efficiency = Int(ratings.efficiency)
    }
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

    var health: NoicanHealth {
        noican_engine_health(handle)
    }

    var levels: NoicanLevels {
        noican_engine_levels(handle)
    }

    func modelStatus(_ model: String) -> NoicanModelStatus {
        model.withCString { id in
            noican_engine_model_status(handle, id)
        }
    }

    static var monitorTargetError: String? {
        copyString { buffer, capacity in
            noican_monitor_target_error(buffer, capacity)
        }
    }

    static func monitorDeviceError(_ deviceID: UInt32) -> String? {
        copyString { buffer, capacity in
            noican_monitor_device_error(deviceID, buffer, capacity)
        }
    }

    func setIntensity(_ value: Float) {
        _ = noican_engine_set_intensity(handle, value)
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
                ratings: ModelRatings(noican_model_ratings(index))
            )
        }
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

enum RustEngineError: LocalizedError {
    case message(String)

    var errorDescription: String? {
        switch self {
        case let .message(message):
            message
        }
    }
}
