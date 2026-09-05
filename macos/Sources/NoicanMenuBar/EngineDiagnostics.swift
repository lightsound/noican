import Foundation
import NoicanState
import os

/// Reports the engine's real-time budget diagnostics (output-ring
/// underruns plus the worker's block-time statistics) to the unified
/// log. Underruns are a hardware-diagnosis tool, not a user-facing
/// control, so nothing renders in the popover by design: one warning
/// line per growth tick — sampled by the 1 Hz health poll — keeps the
/// UI quiet while the counter, the active model, and the block
/// statistics stay retrievable from Console.app (subsystem
/// `com.lightsound.noican`, category `engine-diagnostics`).
@MainActor
final class EngineDiagnostics {
    private static let log = Logger(
        subsystem: "com.lightsound.noican", category: "engine-diagnostics"
    )

    /// Last underrun count already reported. The baseline resets with
    /// the engine counters: on engine start (via `reset()`) and on the
    /// post-switch stats reset (a counter that moved backwards only
    /// rebases silently).
    private var lastUnderrunCount: UInt64 = 0

    /// Whether the one-time transport line was written for this start.
    private var startupLogged = false

    /// Last virtual-output level reading written to the log. Deliberately
    /// *not* reset with the engine counters: the Noican Microphone slider
    /// belongs to the device, not to a transport, so a restart with the
    /// slider still down must not log a fresh "detection" of a slider
    /// that never moved — the log is meant as evidence of how often and
    /// how far it actually moves.
    private var lastVirtualOutputLevel = VirtualOutputLevel.nominal

    /// True when the process runs translated under Rosetta 2 — inference
    /// would be silently slower, so hardware budget numbers must carry
    /// this bit.
    private static let isTranslated: Bool = {
        var flag: Int32 = 0
        var size = MemoryLayout<Int32>.size
        let status = sysctlbyname("sysctl.proc_translated", &flag, &size, nil, 0)
        return status == 0 && flag == 1
    }()

    /// Rebases the baseline for a fresh transport (engine start).
    func reset() {
        lastUnderrunCount = 0
        startupLogged = false
    }

    /// Samples the engine's counters and logs one warning line when the
    /// underrun count grew since the previous sample, carrying the
    /// active model id and the worker block statistics — enough to
    /// attribute dropouts to a model on hardware without any UI.
    func sample(_ engine: RustEngine, activeModelID: String) {
        if !startupLogged {
            startupLogged = true
            Self.log.info(
                """
                Engine transport diagnostics: worker realtime scheduling \
                \(engine.workerRealtime, privacy: .public), \
                Rosetta-translated process \
                \(Self.isTranslated, privacy: .public)
                """
            )
            // Aggregate path only: where the engine output lands inside
            // the aggregate, as AUHAL reports it. The map's effect is
            // invisible otherwise, and this is the line to read when the
            // virtual microphone records silence.
            if let routing = engine.routingDescription {
                Self.log.info("Aggregate output routing: \(routing, privacy: .public)")
            }
        }
        let underruns = engine.outputUnderruns
        defer { lastUnderrunCount = underruns }
        guard underruns > lastUnderrunCount else {
            return
        }
        let overBudget = engine.workerBlocksOverBudget
        let blocks = engine.workerBlocks
        let maxMs = Double(engine.workerBlockMaxNs) / 1_000_000
        Self.log.warning(
            """
            Output underruns: \(underruns, privacy: .public) \
            (model: \(activeModelID, privacy: .public)); \
            worker blocks over 10 ms budget: \(overBudget, privacy: .public)\
            /\(blocks, privacy: .public), \
            max \(maxMs, format: .fixed(precision: 1), privacy: .public) ms
            """
        )
    }

    /// Records a reading of the virtual output device's own volume/mute
    /// controls: one line per *distinct* reading (a turned-down slider
    /// logs its scalar again whenever it moves; the return to unity logs
    /// once), nothing while the reading is unchanged. Detection only —
    /// the level is never written (see `VirtualOutputLevel`).
    func recordVirtualOutputLevel(_ level: VirtualOutputLevel, deviceName: String) {
        guard level != lastVirtualOutputLevel else {
            return
        }
        lastVirtualOutputLevel = level
        switch level {
        case let .turnedDown(scalar):
            Self.log.warning(
                """
                Virtual output level: "\(deviceName, privacy: .public)" volume scalar \
                \(scalar, format: .fixed(precision: 3), privacy: .public) (below unity) — \
                consumers hear the engine output attenuated; not changed by Noican
                """
            )
        case .muted:
            Self.log.warning(
                "Virtual output level: \"\(deviceName, privacy: .public)\" is muted; not changed by Noican"
            )
        case .nominal:
            Self.log.info(
                "Virtual output level: \"\(deviceName, privacy: .public)\" back at unity and unmuted"
            )
        }
    }
}
