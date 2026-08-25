import CNoican
import CoreAudio
import Foundation

final class RustEngine {
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

    func start(aggregateDevice: AudioObjectID, model: String) throws {
        let result = model.withCString { slug in
            noican_engine_start(handle, aggregateDevice, slug)
        }
        try requireSuccess(result)
    }

    func stop() {
        noican_engine_stop(handle)
    }

    func setModel(_ model: String) throws {
        let result = model.withCString { slug in
            noican_engine_set_model(handle, slug)
        }
        try requireSuccess(result)
    }

    var isRunning: Bool {
        noican_engine_is_running(handle) != 0
    }

    var isFaulted: Bool {
        noican_engine_is_faulted(handle) != 0
    }

    static func modelSlugs() -> [String] {
        (0..<noican_model_count()).compactMap { index in
            copyString { buffer, capacity in
                noican_model_slug(index, buffer, capacity)
            }
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
