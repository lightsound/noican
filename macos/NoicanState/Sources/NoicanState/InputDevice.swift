/// One selectable microphone as the state machine sees it: a pure value
/// snapshot taken by the control plane at device-refresh time. Carrying
/// the 48 kHz capability here lets the reducer pre-flight selections
/// without touching Core Audio (the capability of a given device does not
/// change; the list itself follows hot-plug via `AppEvent.devicesChanged`).
public struct InputDevice: Hashable, Identifiable, Sendable {
    /// Core Audio device UID (stable across reconnects).
    public let uid: String
    /// Human-readable device name, shown in the microphone list.
    public let name: String
    /// Whether the device advertises 48 kHz support. Bluetooth headset
    /// microphones advertise telephony rates only and are the common
    /// refusal (issue #7).
    public let supports48kHz: Bool

    public var id: String { uid }

    public init(uid: String, name: String, supports48kHz: Bool) {
        self.uid = uid
        self.name = name
        self.supports48kHz = supports48kHz
    }
}
