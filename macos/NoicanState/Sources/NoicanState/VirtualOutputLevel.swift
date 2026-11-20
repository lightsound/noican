public enum VirtualOutputLevel: Hashable, Sendable {
    case nominal
    case turnedDown(scalar: Float)
    case muted

    public static let unityThreshold: Float = 0.999

    public static func classify(volumeScalar: Float?, isMuted: Bool?) -> VirtualOutputLevel {
        if isMuted == true {
            return .muted
        }
        if let volumeScalar, volumeScalar.isFinite, volumeScalar < unityThreshold {
            return .turnedDown(scalar: volumeScalar)
        }
        return .nominal
    }

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
