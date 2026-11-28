import CNoican
import Foundation
import NoicanState
import os

@MainActor
final class EngineDiagnostics {
    private static let log = Logger(
        subsystem: "com.lightsound.noican", category: "engine-diagnostics"
    )

    private var lastUnderrunCount: UInt64 = 0

    private var lastInputOverrunCount: UInt64 = 0

    private var startupLogged = false

    private var lastVirtualOutputLevel = VirtualOutputLevel.nominal

    private static let isTranslated: Bool = {
        var flag: Int32 = 0
        var size = MemoryLayout<Int32>.size
        let status = sysctlbyname("sysctl.proc_translated", &flag, &size, nil, 0)
        return status == 0 && flag == 1
    }()

    func reset() {
        lastUnderrunCount = 0
        lastInputOverrunCount = 0
        startupLogged = false
    }

    func sample(
        _ health: NoicanHealth,
        of engine: RustEngine,
        activeModelID: String,
        splitTransport: Bool
    ) {
        if !startupLogged {
            startupLogged = true
            Self.log.info(
                """
                Engine transport diagnostics: worker realtime scheduling \
                \(health.worker_realtime, privacy: .public), \
                Rosetta-translated process \
                \(Self.isTranslated, privacy: .public)
                """
            )
            if let routing = engine.routingDescription {
                let prefix = splitTransport ? "Split output routing" : "Aggregate output routing"
                Self.log.info("\(prefix, privacy: .public): \(routing, privacy: .public)")
            }
        }
        let underruns = health.output_underruns
        defer { lastUnderrunCount = underruns }
        if underruns > lastUnderrunCount {
            let overBudget = health.worker_blocks_over_budget
            let blocks = health.worker_blocks
            let maxMs = Double(health.worker_block_max_ns) / 1_000_000
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
        let overruns = health.input_overruns
        defer { lastInputOverrunCount = overruns }
        if overruns > lastInputOverrunCount {
            Self.log.warning(
                """
                Input overruns: \(overruns, privacy: .public) samples dropped \
                (model: \(activeModelID, privacy: .public)) — \
                the capture ring filled faster than the worker drained it
                """
            )
        }
    }

    func recordVirtualOutputLevel(_ level: VirtualOutputLevel, deviceName: String) {
        guard level != lastVirtualOutputLevel else {
            return
        }
        lastVirtualOutputLevel = level
        switch level {
        case let .turnedDown(scalar):
            let percent = Int(scalar * 100)
            Self.log.warning(
                """
                Virtual output level: "\(deviceName, privacy: .public)" volume at \
                \(percent, privacy: .public)% (scalar \
                \(scalar, format: .fixed(precision: 3), privacy: .public), below unity) — \
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
