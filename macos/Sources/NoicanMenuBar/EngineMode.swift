import Foundation

/// Settled engine lifecycle: the last known outcome, never a transition.
/// In-flight transitions are expressed solely by `AppState.isBusy` (the
/// spinner), so every surface rendered from this moves exactly once per
/// transition, settled state to settled state.
enum EnginePhase: Equatable {
    case off
    case running
    case failed(String)
}

/// The single top-level control: Off, Preview (engine + self-monitor on
/// the default output), or On (engine only, for meetings). Preview and On
/// both feed the virtual microphone; the only difference is the monitor
/// tee, so switching between them is instant and click-free.
enum EngineMode: String, CaseIterable, Identifiable {
    case off
    case preview
    case on

    var id: String { rawValue }

    var label: String {
        switch self {
        case .off: "Off"
        case .preview: "Preview"
        case .on: "On"
        }
    }

    var symbolName: String {
        switch self {
        case .off: "power"
        case .preview: "headphones"
        case .on: "waveform"
        }
    }
}
