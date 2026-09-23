#if canImport(Security)
import Foundation
import Security

/// Keeps the activation as one generic-password item in the user's login
/// Keychain. The item's access control list admits the app that created
/// it, so a Developer ID build reads it back silently across updates
/// (same designated requirement); an ad-hoc development build is a new
/// code identity after every rebuild and gets a Keychain access prompt.
public struct KeychainLicenseStore: LicenseStore {
    private let service: String
    private let account: String

    public init(service: String = "com.lightsound.noican.license", account: String = "activation") {
        self.service = service
        self.account = account
    }

    public func load() throws -> StoredLicense? {
        var query = baseQuery
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound {
            return nil
        }
        guard status == errSecSuccess, let data = result as? Data else {
            throw KeychainError(operation: "read", status: status)
        }
        return try StoredLicense.decode(data)
    }

    public func save(_ license: StoredLicense) throws {
        let data = try license.encoded()
        let update = SecItemUpdate(
            baseQuery as CFDictionary,
            [kSecValueData as String: data] as CFDictionary
        )
        if update == errSecSuccess {
            return
        }
        guard update == errSecItemNotFound else {
            throw KeychainError(operation: "update", status: update)
        }
        var item = baseQuery
        item[kSecValueData as String] = data
        item[kSecAttrLabel as String] = "Noican license"
        let add = SecItemAdd(item as CFDictionary, nil)
        guard add == errSecSuccess else {
            throw KeychainError(operation: "save", status: add)
        }
    }

    public func delete() throws {
        let status = SecItemDelete(baseQuery as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw KeychainError(operation: "delete", status: status)
        }
    }

    private var baseQuery: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account
        ]
    }
}

public struct KeychainError: LocalizedError, Hashable {
    public var operation: String
    public var status: OSStatus

    public var errorDescription: String? {
        let reason = SecCopyErrorMessageString(status, nil) as String? ?? "status \(status)"
        return "Couldn't \(operation) the license in the Keychain: \(reason)"
    }
}
#endif
