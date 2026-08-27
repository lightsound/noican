import CNoican
import CoreAudio
import Foundation

/// A model entry exposed by the Rust registry (never hardcoded here).
struct ModelInfo: Hashable, Identifiable {
    let id: String
    let displayName: String
    let needsEnrollment: Bool
}

/// Thin wrapper over the Rust C ABI. The handle is internally synchronized
/// (a mutex-guarded control state on the Rust side), so calls may come from
/// any thread; long operations (weight download, model construction) are
/// expected to run off the main actor.
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

    func start(aggregateDevice: AudioObjectID, model: String) throws {
        let result = model.withCString { id in
            noican_engine_start(handle, aggregateDevice, id)
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
    }

    /// Toggles the preview self-monitor (processed voice on the system
    /// default output). Fails when the engine is stopped or the default
    /// output is a virtual loopback; the meeting-facing path is never
    /// affected. The monitor does not survive an engine stop/start, so
    /// callers re-enable it after `start`.
    func setMonitor(_ enabled: Bool) throws {
        try requireSuccess(noican_engine_set_monitor(handle, enabled ? 1 : 0))
    }

    /// Preview monitor state, one lock-free read — never waits on the
    /// engine's control lock, so it is safe to poll at 20 Hz even while
    /// a monitor start is in progress. The tripped protocol (AUHAL still
    /// up but silenced; cleared by the next toggle) is defined on the
    /// enum itself.
    var monitorState: MonitorState {
        MonitorState(rawValue: noican_engine_monitor_state(handle)) ?? .off
    }

    /// Reason the current system default output must not receive the
    /// preview (loopback, aggregate, or built-in speakers), or nil when
    /// preview may start. A pure inspection — a few Core Audio property
    /// reads, no audio objects — so it is cheap to poll.
    static var monitorTargetError: String? {
        copyString { buffer, capacity in
            noican_monitor_target_error(buffer, capacity)
        }
    }

    /// Device the running preview monitor plays on (resolved on the Rust
    /// side at enable time), or 0 while no monitor is up. Reads the
    /// control mutex — meant for event-driven and 1 Hz callers, not the
    /// 20 Hz poll path.
    var monitorDeviceID: UInt32 {
        noican_engine_monitor_device(handle)
    }

    /// Reason a *specific* device must not receive the preview — the same
    /// vetting as `monitorTargetError`, applied to the device the monitor
    /// actually plays on rather than the current default output (the two
    /// diverge once the default output moves after enable time). Catches
    /// the same-device data-source flip from the headphone jack to the
    /// internal speakers; a vanished device reads as unclassifiable and
    /// is the caller's device-list check instead.
    static func monitorDeviceError(_ deviceID: UInt32) -> String? {
        copyString { buffer, capacity in
            noican_monitor_device_error(deviceID, buffer, capacity)
        }
    }

    /// Decayed linear peak (0–1) of the model input, measured per 10 ms
    /// block by the inference worker; 0 while stopped. Reads one atomic —
    /// never blocks, so it is safe to poll from the UI.
    var inputLevel: Float {
        noican_engine_input_level(handle)
    }

    /// Decayed linear peak (0–1) of the model output; see `inputLevel`.
    var outputLevel: Float {
        noican_engine_output_level(handle)
    }

    var isRunning: Bool {
        noican_engine_is_running(handle) != 0
    }

    var isFaulted: Bool {
        noican_engine_is_faulted(handle) != 0
    }

    /// Heartbeat: input frames delivered by the audio device since start.
    /// Stops advancing when the device stops calling back.
    var framesProcessed: UInt64 {
        noican_engine_frames_processed(handle)
    }

    /// The selectable model catalog, read from the Rust registry.
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
                needsEnrollment: noican_model_needs_enrollment(index) != 0
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

/// Preview monitor state as reported by `noican_engine_monitor_state`.
/// Raw values mirror the C `NoicanMonitorState` enum (noican.h) and the
/// Rust `MonitorState`; they are frozen.
enum MonitorState: Int32 {
    /// No monitor AUHAL is up (engine stopped or preview disabled).
    case off = 0
    /// The monitor plays the processed microphone signal.
    case playing = 1
    /// The feedback guard silenced the preview: the monitor AUHAL is
    /// still up but renders silence. Cleared by the next monitor toggle
    /// in either direction — enabling re-arms, disabling tears down.
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
