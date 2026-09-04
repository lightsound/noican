/// How a microphone can feed the 48 kHz engine, decided from the Core
/// Audio snapshot at device-refresh time. Carrying this here lets the
/// reducer pre-flight selections without touching Core Audio.
public enum CaptureSupport: Hashable, Sendable {
    /// The device can run at the 48 kHz engine rate: captured through
    /// the private Aggregate Device (the original, unchanged path).
    case engineRate
    /// The device is fixed to a native rate other than 48 kHz —
    /// Bluetooth telephony profiles (HFP/SCO at 8/16/24 kHz), the
    /// 44.1 kHz family (44.1/22.05/11.025 kHz), 88.2/96 kHz interfaces:
    /// captured natively through the split transport and resampled to
    /// 48 kHz inside it by the exact rational ratio (issue #7). The
    /// associated value is the snapshot's nominal rate in Hz; the
    /// transport re-reads the live rate at start time (Bluetooth
    /// profiles renegotiate).
    case nativeRate(hertz: Int)
    /// Neither 48 kHz-capable nor at a rate inside `nativeRateRange`
    /// (unreadable rate, or an exotic device): not selectable as the
    /// engine input.
    case unsupported(hertz: Int)

    /// Native rates the split transport's resampler converts, in Hz.
    /// Mirrors `noican_core::capture::{MIN,MAX}_NATIVE_RATE` (the Rust
    /// side re-validates at start); keep the two in sync.
    public static let nativeRateRange = 8_000...192_000

    /// Classifies a device from the two Core Audio facts the shell
    /// samples: whether it advertises 48 kHz support, and its current
    /// nominal rate. Pure, so the policy stays reducer-testable.
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

    /// Whether a native rate is a Bluetooth telephony profile (HFP/SCO
    /// at 8/12/16/24 kHz) — narrow-band capture with the headset-wide
    /// playback trade-off — as opposed to a full-band rate that is
    /// merely not 48 kHz (44.1 kHz and up). Drives the selection notice.
    public static func isTelephonyRate(_ hertz: Int) -> Bool {
        hertz <= 24_000
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
    /// microphone list ("16 kHz", "44.1 kHz"), or nil for 48 kHz-capable
    /// devices (the normal case needs no annotation).
    public var rateLabel: String? {
        switch capture {
        case .engineRate:
            return nil
        case let .nativeRate(hertz), let .unsupported(hertz):
            return InputDevice.rateLabel(hertz: hertz)
        }
    }

    /// Formats a rate in Hz the way audio users read it: "16 kHz",
    /// "44.1 kHz", "22.05 kHz", "11.025 kHz" — whole kilohertz plus the
    /// remaining hertz as up to three decimals with trailing zeros
    /// trimmed. Sub-kilohertz rates (never real devices) stay in Hz.
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
