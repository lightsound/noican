import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif
@testable import NoicanLicensing

// MARK: - Fixtures

let thisMac = DeviceDescriptor(id: "device-a", label: "Test MacBook", metadata: ["app_version": "1.0.0"])
let otherMac = DeviceDescriptor(id: "device-b", label: "Old iMac")
let launchTime = Date(timeIntervalSince1970: 1_800_000_000)
let hour: TimeInterval = 60 * 60
let day: TimeInterval = 24 * hour

func grant(_ activationID: String = "act-1", expiresAt: Date? = nil) -> LicenseGrant {
    LicenseGrant(activationID: activationID, displayKey: "****-E304DA", expiresAt: expiresAt, activationLimit: 3)
}

func storedLicense(
    backendID: String = "mock",
    device: DeviceDescriptor = thisMac,
    key: String = "KEY-1",
    activationID: String = "act-1",
    validatedAt: Date = launchTime,
    rejection: LicenseRejection? = nil
) -> StoredLicense {
    var license = StoredLicense(
        backendID: backendID,
        deviceID: device.id,
        key: key,
        grant: grant(activationID),
        lastValidatedAt: validatedAt
    )
    license.rejection = rejection
    return license
}

// MARK: - Test doubles

/// A settable clock the controller reads through its `now` closure.
@MainActor
final class TestClock {
    var now = launchTime

    func advance(_ interval: TimeInterval) {
        now = now.addingTimeInterval(interval)
    }
}

final class InMemoryLicenseStore: LicenseStore {
    var license: StoredLicense?
    var failSaves = false
    var failLoads = false
    private(set) var loads = 0

    init(_ license: StoredLicense? = nil) {
        self.license = license
    }

    func load() throws -> StoredLicense? {
        loads += 1
        if failLoads {
            throw StoreFailure()
        }
        return license
    }

    func save(_ license: StoredLicense) throws {
        if failSaves {
            throw StoreFailure()
        }
        self.license = license
    }

    func delete() throws {
        license = nil
    }
}

struct StoreFailure: LocalizedError {
    var errorDescription: String? { "Keychain unavailable" }
}

/// Scripted backend: each operation pops its next result (or repeats
/// the last one) and records the call.
final class MockBackend: LicenseBackend, @unchecked Sendable {
    enum Call: Hashable {
        case activate(key: String, deviceID: String)
        case validate(key: String, activationID: String)
        case deactivate(key: String, activationID: String)
    }

    let identifier: String
    let managementURL: URL? = URL(string: "https://polar.sh/noican/portal")
    private let lock = NSLock()
    private var activateResults: [Result<LicenseGrant, LicenseBackendError>] = [.success(grant())]
    private var validateResults: [Result<LicenseGrant, LicenseBackendError>] = [.success(grant())]
    private var deactivateResults: [Result<Void, LicenseBackendError>] = [.success(())]
    private var recorded: [Call] = []

    init(identifier: String = "mock") {
        self.identifier = identifier
    }

    var calls: [Call] {
        lock.withLock { recorded }
    }

    func onActivate(_ results: Result<LicenseGrant, LicenseBackendError>...) {
        lock.withLock { activateResults = results }
    }

    func onValidate(_ results: Result<LicenseGrant, LicenseBackendError>...) {
        lock.withLock { validateResults = results }
    }

    func onDeactivate(_ results: Result<Void, LicenseBackendError>...) {
        lock.withLock { deactivateResults = results }
    }

    func activate(key: String, device: DeviceDescriptor) async throws(LicenseBackendError) -> LicenseGrant {
        try next(.activate(key: key, deviceID: device.id), &activateResults).get()
    }

    func validate(key: String, activationID: String) async throws(LicenseBackendError) -> LicenseGrant {
        try next(.validate(key: key, activationID: activationID), &validateResults).get()
    }

    func deactivate(key: String, activationID: String) async throws(LicenseBackendError) {
        try next(.deactivate(key: key, activationID: activationID), &deactivateResults).get()
    }

    private func next<T>(_ call: Call, _ results: inout [Result<T, LicenseBackendError>]) -> Result<T, LicenseBackendError> {
        lock.lock()
        defer { lock.unlock() }
        recorded.append(call)
        return results.count > 1 ? results.removeFirst() : results[0]
    }
}

/// Scripted HTTP transport for the Polar backend.
final class FakeTransport: HTTPTransport, @unchecked Sendable {
    enum Reply {
        case http(Int, String)
        case failure(URLError.Code)
    }

    private let lock = NSLock()
    private var replies: [Reply]
    private var sent: [URLRequest] = []

    init(_ replies: Reply...) {
        self.replies = replies
    }

    var requests: [URLRequest] {
        lock.withLock { sent }
    }

    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let reply = lock.withLock {
            sent.append(request)
            return replies.count > 1 ? replies.removeFirst() : replies[0]
        }
        switch reply {
        case let .http(status, body):
            let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: "HTTP/1.1", headerFields: nil)!
            return (Data(body.utf8), response)
        case let .failure(code):
            throw URLError(code)
        }
    }
}

extension URLRequest {
    /// The JSON body as a dictionary, for asserting on request fields.
    var jsonBody: [String: Any] {
        guard let httpBody, let object = try? JSONSerialization.jsonObject(with: httpBody) as? [String: Any] else {
            return [:]
        }
        return object
    }
}
