import Foundation

public struct StoredLicense: Codable, Hashable, Sendable {
    public static let currentFormat = 1

    public var format: Int
    public var backendID: String
    public var deviceID: String
    public var key: String
    public var activationID: String
    public var displayKey: String
    public var expiresAt: Date?
    public var activationLimit: Int?
    public var lastValidatedAt: Date
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

    public func belongs(toBackend backendID: String, device deviceID: String) -> Bool {
        self.backendID == backendID && self.deviceID == deviceID
    }

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

public protocol LicenseStore {
    func load() throws -> StoredLicense?
    func save(_ license: StoredLicense) throws
    func delete() throws
}
