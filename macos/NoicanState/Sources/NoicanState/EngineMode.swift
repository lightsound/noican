/// The single top-level control: Off, Preview (engine + self-monitor on
/// the default output), or On (engine only, for meetings). Preview and On
/// both feed the virtual microphone; the only difference is the monitor
/// tee, so switching between them is instant and click-free.
///
/// The mode is the *user's intent* and only the user changes it: the
/// system never moves the selection. When the selected mode is not
/// actually delivering, the failure renders as the warning tint plus the
/// reason under the control, and re-tapping the same segment retries.
public enum EngineMode: String, CaseIterable, Identifiable, Sendable {
    case off
    case preview
    case on

    public var id: String { rawValue }

    /// Segment title in the mode control.
    public var label: String {
        switch self {
        case .off: "Off"
        case .preview: "Preview"
        case .on: "On"
        }
    }

    /// SF Symbol name shown next to the segment title.
    public var symbolName: String {
        switch self {
        case .off: "power"
        case .preview: "headphones"
        case .on: "waveform"
        }
    }
}
