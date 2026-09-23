import Foundation

/// The activation as persisted on this Mac (in the Keychain). The key
/// itself is kept, not just the activation: it is what lets a backend
/// swap or a migrated Mac re-activate without asking the customer, and
/// what a definitive rejection can later be re-checked with.
public struct StoredLicense: Codable, Hashable, Sendable {
    /// Bumped when the stored shape changes incompatibly.
    public static let currentFormat = 1

    public var format: Int
    /// `LicenseBackend.identifier` of the backend that issued
    /// `activationID`.
    public var backendID: String
    /// `DeviceDescriptor.id` of the Mac the activation belongs to.
    public var deviceID: String
    public var key: String
    public var activationID: String
    public var displayKey: String
    public var expiresAt: Date?
    public var activationLimit: Int?
    /// Local clock time of the last successful activation or validation;
    /// the offline grace period counts from here.
    public var lastValidatedAt: Date
    /// Set when the server's last definitive answer was no. The record is
    /// kept (and re-checked) rather than deleted, so a mistaken rejection
    /// heals on the next successful validation.
    public var rejection: LicenseRejection?

    public init(
        backendID: String,
        deviceID: String,
        key: String,
        grant: LicenseGrant,
        lastValidatedAt: Date
    ) {
        format = Self.currentFormat
        self.backendID = backendID
        self.deviceID = deviceID
        self.key = key
        activationID = grant.activationID
        displayKey = grant.displayKey
        expiresAt = grant.expiresAt
        activationLimit = grant.activationLimit
        self.lastValidatedAt = lastValidatedAt
        rejection = nil
    }

    /// Whether the activation was issued for this backend and this Mac.
    /// Anything else must be activated again (with the stored key) before
    /// it can be validated.
    public func belongs(toBackend backendID: String, device deviceID: String) -> Bool {
        self.backendID == backendID && self.deviceID == deviceID
    }

    /// Records a successful validation.
    mutating func apply(_ grant: LicenseGrant, validatedAt date: Date) {
        activationID = grant.activationID
        displayKey = grant.displayKey
        expiresAt = grant.expiresAt
        activationLimit = grant.activationLimit
        lastValidatedAt = date
        rejection = nil
    }

    public func encoded() throws -> Data {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .secondsSince1970
        return try encoder.encode(self)
    }

    public static func decode(_ data: Data) throws -> StoredLicense {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .secondsSince1970
        return try decoder.decode(StoredLicense.self, from: data)
    }
}

/// Persistence for the one activation this Mac holds.
public protocol LicenseStore {
    func load() throws -> StoredLicense?
    func save(_ license: StoredLicense) throws
    func delete() throws
}
