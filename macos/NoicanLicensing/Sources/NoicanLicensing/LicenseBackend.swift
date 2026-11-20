import Foundation

public protocol LicenseBackend: Sendable {
    var identifier: String { get }
    var managementURL: URL? { get }

    func activate(key: String, device: DeviceDescriptor) async throws(LicenseBackendError) -> LicenseGrant
    func validate(key: String, activationID: String) async throws(LicenseBackendError) -> LicenseGrant
    func deactivate(key: String, activationID: String) async throws(LicenseBackendError)
}

public struct DeviceDescriptor: Hashable, Sendable {
    public var id: String
    public var label: String
    public var metadata: [String: String]

    public init(id: String, label: String, metadata: [String: String] = [:]) {
        self.id = id
        self.label = label
        self.metadata = metadata
    }
}

public struct LicenseGrant: Hashable, Sendable {
    public var activationID: String
    public var displayKey: String
    public var expiresAt: Date?
    public var activationLimit: Int?

    public init(activationID: String, displayKey: String, expiresAt: Date? = nil, activationLimit: Int? = nil) {
        self.activationID = activationID
        self.displayKey = displayKey
        self.expiresAt = expiresAt
        self.activationLimit = activationLimit
    }
}

public enum LicenseBackendError: Error, Hashable, Sendable {
    case rejected(LicenseRejection)
    case unavailable(reason: String)
}

public enum LicenseRejection: Hashable, Sendable, Codable {
    case unknownKey
    case activationRevoked
    case refused(detail: String)
    case wrongProduct
    case expired(Date)

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
