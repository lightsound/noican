import Foundation

/// The license server as the app sees it: activate a key on this Mac,
/// re-validate that activation, release it. `PolarLicenseBackend` is the
/// shipping implementation; another provider (Paddle with a small
/// activation server, Keygen, a self-hosted server holding keys exported
/// from Polar) replaces it by conforming to this protocol and changing
/// `identifier` — `LicenseController` then re-activates every stored key
/// against the new backend on the next launch, so customers never
/// re-enter their key.
public protocol LicenseBackend: Sendable {
    /// Stable name of the backend, persisted with each activation. A
    /// stored activation whose identifier differs was issued by another
    /// backend and is re-activated rather than validated.
    var identifier: String { get }
    /// Where customers manage their devices and keys (Polar's customer
    /// portal), or nil when the backend has no such page.
    var managementURL: URL? { get }

    /// Reserves one device slot of `key` for `device`.
    func activate(key: String, device: DeviceDescriptor) async throws(LicenseBackendError) -> LicenseGrant
    /// Confirms that `activationID` of `key` is still valid.
    func validate(key: String, activationID: String) async throws(LicenseBackendError) -> LicenseGrant
    /// Releases the device slot. An activation the server no longer
    /// knows counts as released.
    func deactivate(key: String, activationID: String) async throws(LicenseBackendError)
}

/// What the app tells the server about this Mac.
public struct DeviceDescriptor: Hashable, Sendable {
    /// Opaque per-Mac identifier (a salted hash of the hardware UUID).
    /// Kept locally with the activation to notice a Keychain that moved
    /// to another Mac (Migration Assistant); never used as a server-side
    /// condition, so a logic-board swap cannot lock the customer out.
    public var id: String
    /// Human-readable name shown next to the activation in the customer
    /// portal, so the customer can tell which Mac to release.
    public var label: String
    /// Extra facts stored with the activation for support (app and macOS
    /// versions). Values must be non-empty.
    public var metadata: [String: String]

    public init(id: String, label: String, metadata: [String: String] = [:]) {
        self.id = id
        self.label = label
        self.metadata = metadata
    }
}

/// A successful activation or validation.
public struct LicenseGrant: Hashable, Sendable {
    public var activationID: String
    /// Masked key for display (Polar: `****-E304DA`).
    public var displayKey: String
    public var expiresAt: Date?
    /// Device limit of the key, when the server reports one.
    public var activationLimit: Int?

    public init(activationID: String, displayKey: String, expiresAt: Date? = nil, activationLimit: Int? = nil) {
        self.activationID = activationID
        self.displayKey = displayKey
        self.expiresAt = expiresAt
        self.activationLimit = activationLimit
    }
}

/// Backend failures, split by what they mean for a customer who already
/// paid: only a definitive answer from the license server may take a
/// license away; anything else keeps the offline grace period running.
public enum LicenseBackendError: Error, Hashable, Sendable {
    /// The server answered, and the answer is no.
    case rejected(LicenseRejection)
    /// No usable answer: offline, timeout, rate limit, server error, or a
    /// response this build does not understand. `reason` is for the UI.
    case unavailable(reason: String)
}

/// Why the license server refused a key.
public enum LicenseRejection: Hashable, Sendable, Codable {
    /// Activation: no such key.
    case unknownKey
    /// Validation: the key or this Mac's activation is gone — released
    /// from the customer portal, key rotated, refunded, or revoked.
    case activationRevoked
    /// Activation refused with the server's explanation: the device limit
    /// is reached, or the key is revoked, disabled, or expired.
    case refused(detail: String)
    /// The key belongs to another product of the same seller.
    case wrongProduct
    /// The key carries an expiry date that has passed.
    case expired(Date)

    /// One user-facing sentence, in the voice of the app's other
    /// message slots.
    public var message: String {
        switch self {
        case .unknownKey:
            "License key not found — check the key and try again."
        case .activationRevoked:
            "This Mac's activation is no longer valid (released, key rotated, or refunded). Enter your license key again."
        case let .refused(detail):
            "Activation refused: \(Self.sentence(detail)) To move the license, deactivate it on the other Mac "
                + "or release that Mac in the customer portal."
        case .wrongProduct:
            "This license key is for a different product."
        case let .expired(date):
            "This license expired on \(date.formatted(date: .abbreviated, time: .omitted))."
        }
    }

    private static func sentence(_ text: String) -> String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let last = trimmed.last else {
            return "the license server gave no reason."
        }
        return ".!?".contains(last) ? trimmed : trimmed + "."
    }
}
