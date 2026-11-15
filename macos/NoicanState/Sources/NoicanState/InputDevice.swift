public enum CaptureSupport: Hashable, Sendable {
    case engineRate
    case nativeRate(hertz: Int)
    case unsupported(hertz: Int)

    public static let nativeRateRange = 8_000...192_000

    public static func classify(supports48kHz: Bool, nominalRate: Double) -> CaptureSupport {
        guard !supports48kHz else {
            return .engineRate
        }
        let hertz = Int(nominalRate.rounded())
        guard nativeRateRange.contains(hertz) else {
            return .unsupported(hertz: max(hertz, 0))
        }
        return .nativeRate(hertz: hertz)
    }

    public static func isTelephonyRate(_ hertz: Int) -> Bool {
        hertz % 4_000 == 0 && hertz <= 32_000
    }
}

public struct InputDevice: Hashable, Identifiable, Sendable {
    public let uid: String
    public let name: String
    public let capture: CaptureSupport

    public var id: String { uid }

    public init(uid: String, name: String, capture: CaptureSupport) {
        self.uid = uid
        self.name = name
        self.capture = capture
    }

    public var rateLabel: String? {
        switch capture {
        case .engineRate:
            return nil
        case let .nativeRate(hertz), let .unsupported(hertz):
            return InputDevice.rateLabel(hertz: hertz)
        }
    }

    static func rateLabel(hertz: Int) -> String {
        guard hertz >= 1_000 else {
            return "\(hertz) Hz"
        }
        let whole = hertz / 1_000
        let remainder = hertz % 1_000
        guard remainder != 0 else {
            return "\(whole) kHz"
        }
        var fraction = String(remainder)
        fraction = String(repeating: "0", count: 3 - fraction.count) + fraction
        while fraction.hasSuffix("0") {
            fraction.removeLast()
        }
        return "\(whole).\(fraction) kHz"
    }
}
