public enum EngineMode: String, CaseIterable, Identifiable, Sendable {
    case off
    case preview
    case on

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .off: "Off"
        case .preview: "Preview"
        case .on: "On"
        }
    }

    public var symbolName: String {
        switch self {
        case .off: "power"
        case .preview: "headphones"
        case .on: "waveform"
        }
    }
}
