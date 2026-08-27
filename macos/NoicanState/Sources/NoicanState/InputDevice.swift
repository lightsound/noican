/// How a microphone can feed the 48 kHz engine, decided from the Core
/// Audio snapshot at device-refresh time. Carrying this here lets the
/// reducer pre-flight selections without touching Core Audio.
public enum CaptureSupport: Hashable, Sendable {
    /// The device can run at the 48 kHz engine rate: captured through
    /// the private Aggregate Device (the original, unchanged path).
    case engineRate
    /// The device is fixed to a native rate that divides 48 kHz —
    /// Bluetooth telephony profiles (HFP/SCO at 8/16/24 kHz): captured
    /// natively through the split transport and resampled to 48 kHz
    /// inside it (issue #7). The associated value is the snapshot's
    /// nominal rate in Hz; the transport re-reads the live rate at
    /// start time (Bluetooth profiles renegotiate).
    case nativeRate(hertz: Int)
    /// Neither 48 kHz-capable nor an integer divisor of 48 kHz (e.g. a
    /// 44.1 kHz-only interface): not selectable as the engine input.
    case unsupported(hertz: Int)

    /// Classifies a device from the two Core Audio facts the shell
    /// samples: whether it advertises 48 kHz support, and its current
    /// nominal rate. Pure, so the policy stays reducer-testable.
    public static func classify(supports48kHz: Bool, nominalRate: Double) -> CaptureSupport {
        guard !supports48kHz else {
            return .engineRate
        }
        let hertz = Int(nominalRate.rounded())
        guard hertz > 0, hertz < 48_000, 48_000 % hertz == 0 else {
            return .unsupported(hertz: max(hertz, 0))
        }
        return .nativeRate(hertz: hertz)
    }
}

/// One selectable microphone as the state machine sees it: a pure value
/// snapshot taken by the control plane at device-refresh time. Carrying
/// the capture capability here lets the reducer pre-flight selections
/// without touching Core Audio (the list itself follows hot-plug via
/// `AppEvent.devicesChanged`; a running device's live rate change is
/// observed separately via `AppEvent.inputSampleRateChanged`).
public struct InputDevice: Hashable, Identifiable, Sendable {
    /// Core Audio device UID (stable across reconnects).
    public let uid: String
    /// Human-readable device name, shown in the microphone list.
    public let name: String
    /// How the engine can capture from this device.
    public let capture: CaptureSupport

    public var id: String { uid }

    public init(uid: String, name: String, capture: CaptureSupport) {
        self.uid = uid
        self.name = name
        self.capture = capture
    }

    /// Short rate caption shown next to non-48 kHz devices in the
    /// microphone list ("16 kHz"), or nil for 48 kHz-capable devices
    /// (the normal case needs no annotation).
    public var rateLabel: String? {
        switch capture {
        case .engineRate:
            return nil
        case let .nativeRate(hertz), let .unsupported(hertz):
            if hertz % 1_000 == 0 {
                return "\(hertz / 1_000) kHz"
            }
            return "\(hertz) Hz"
        }
    }
}
