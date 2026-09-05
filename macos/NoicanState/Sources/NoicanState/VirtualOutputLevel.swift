/// What the Noican virtual output device's own volume and mute controls
/// say about the level consumers will receive, classified from the raw
/// Core Audio readings by the shell.
///
/// The BlackHole-derived driver registers a volume control (one master
/// value, −64…0 dB, applied to every sample it loops) and a mute control,
/// both exposed in System Settings › Sound › Input as the "Noican
/// Microphone" slider. A 2026-09-05 hardware investigation traced "Noican
/// sounds quiet in meetings" to that slider sitting at about −35 dB —
/// nothing in the engine or the transport was attenuating. Who moved it
/// is unknown (the user, or a meeting app's "automatically adjust
/// microphone volume" writing the selected input device's system volume),
/// which is why the reducer only *reports* the condition and never
/// restores the level: writing it back would fight such an app and take
/// away the user's own adjustment.
///
/// Devices without these controls (or whose controls cannot be read) are
/// classified as `.nominal` — fail open: no message is better than a
/// message about a control that does not exist.
public enum VirtualOutputLevel: Hashable, Sendable {
    /// Volume at unity (or unreadable) and not muted.
    case nominal
    /// The volume control is below unity; `scalar` is the Core Audio
    /// scalar reading (0…1), kept for the log rather than the UI.
    case turnedDown(scalar: Float)
    /// The mute control is on. Takes precedence over a low volume: muted
    /// is the more actionable finding.
    case muted

    /// Scalar readings at or above this count as unity. Core Audio
    /// reports the slider position as a `Float32`, and a slider parked
    /// at the top can read a hair below 1.0 depending on the driver's
    /// dB → scalar conversion; anything closer than this is not a level
    /// the user (or an app) turned down.
    public static let unityThreshold: Float = 0.999

    /// Classifies the raw readings. `volumeScalar` and `isMuted` are nil
    /// when the corresponding control is absent or unreadable, which
    /// never produces a finding on its own.
    public static func classify(volumeScalar: Float?, isMuted: Bool?) -> VirtualOutputLevel {
        if isMuted == true {
            return .muted
        }
        if let volumeScalar, volumeScalar.isFinite, volumeScalar < unityThreshold {
            return .turnedDown(scalar: volumeScalar)
        }
        return .nominal
    }

    /// The one-line notice for the popover, or nil when nothing needs
    /// saying. Always a single sentence naming the place to fix it, in
    /// the voice of the other message slots.
    public var notice: String? {
        switch self {
        case .nominal:
            nil
        case .turnedDown:
            "Noican Microphone volume is turned down in System Settings › Sound › Input — apps will hear you quietly."
        case .muted:
            "Noican Microphone is muted in System Settings › Sound › Input."
        }
    }
}
