import Foundation

/// Engine lifecycle as shown in the menu: drives the status dot and text.
enum EnginePhase: Equatable {
    case off
    case busy(String)
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
