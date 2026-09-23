import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// Where the app's Polar integration points. Every value is public
/// (the customer-portal license endpoints take no secret), so it is
/// compiled into the app.
public struct PolarConfiguration: Hashable, Sendable {
    public enum Server: String, Hashable, Sendable {
        case production
        case sandbox

        var apiBaseURL: URL {
            switch self {
            case .production: URL(string: "https://api.polar.sh")!
            case .sandbox: URL(string: "https://sandbox-api.polar.sh")!
            }
        }

        var siteBaseURL: URL {
            switch self {
            case .production: URL(string: "https://polar.sh")!
            case .sandbox: URL(string: "https://sandbox.polar.sh")!
            }
        }
    }

    public var server: Server
    /// Polar organization ID (Settings › General). Required: Polar scopes
    /// every key lookup to it.
    public var organizationID: String
    /// ID of the license-key benefit attached to the Noican product. When
    /// set, keys from the seller's other products are refused.
    public var benefitID: String?
    /// Organization slug, for the customer-portal link
    /// (`https://polar.sh/<slug>/portal`).
    public var organizationSlug: String?

    public init(server: Server, organizationID: String, benefitID: String? = nil, organizationSlug: String? = nil) {
        self.server = server
        self.organizationID = organizationID
        self.benefitID = benefitID.flatMap(Self.nonEmpty)
        self.organizationSlug = organizationSlug.flatMap(Self.nonEmpty)
    }

    /// Whether the organization ID has been filled in with a UUID.
    public var isComplete: Bool {
        UUID(uuidString: organizationID) != nil
    }

    public var customerPortalURL: URL? {
        organizationSlug.map {
            server.siteBaseURL.appendingPathComponent($0).appendingPathComponent("portal")
        }
    }

    private static func nonEmpty(_ value: String) -> String? {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}

/// One HTTP round trip; `URLSession` in the app, a scripted fake in tests.
public protocol HTTPTransport: Sendable {
    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse)
}

public struct URLSessionTransport: HTTPTransport {
    private let session: URLSession

    public init(timeout: TimeInterval = 15) {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = timeout
        configuration.timeoutIntervalForResource = timeout * 2
        session = URLSession(configuration: configuration)
    }

    public func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw URLError(.badServerResponse)
        }
        return (data, http)
    }
}

/// Polar's public customer-portal license-key API
/// (`POST /v1/customer-portal/license-keys/{activate,validate,deactivate}`),
/// which needs no access token and is meant for desktop clients.
///
/// No `Polar-Version` header is sent, deliberately: Polar removes each
/// dated API version about nine months after release and then answers
/// requests pinned to it with 404, which a shipped app could not tell
/// from "key not found" — every customer on an old build would lose
/// their license on the removal date. Unpinned requests use Polar's
/// Current version; the decoding below reads only the handful of fields
/// every version has carried, and anything it cannot read is
/// `.unavailable` (grace period), never a rejection.
public struct PolarLicenseBackend: LicenseBackend {
    public let identifier = "polar"
    private let configuration: PolarConfiguration
    private let transport: any HTTPTransport
    private let userAgent: String

    public init(configuration: PolarConfiguration, transport: any HTTPTransport, userAgent: String) {
        self.configuration = configuration
        self.transport = transport
        self.userAgent = userAgent
    }

    public var managementURL: URL? {
        configuration.customerPortalURL
    }

    public func activate(key: String, device: DeviceDescriptor) async throws(LicenseBackendError) -> LicenseGrant {
        let body = ActivateRequest(
            key: key,
            organizationID: configuration.organizationID,
            label: Self.clamped(device.label.isEmpty ? "Mac" : device.label),
            meta: Self.meta(device.metadata)
        )
        let (data, response) = try await post("activate", body)
        let error = ErrorResponse.parse(data)
        switch response.statusCode {
        case 200:
            let activation: ActivationResponse = try decode(data)
            guard activation.licenseKey.isForBenefit(configuration.benefitID) else {
                // Activation takes no benefit filter, so a key for another
                // product got a slot; give it back before refusing.
                try? await deactivate(key: key, activationID: activation.id)
                throw .rejected(.wrongProduct)
            }
            return activation.licenseKey.grant(activationID: activation.id, key: key)
        case 404 where error?.error == "ResourceNotFound":
            throw .rejected(.unknownKey)
        case 403 where error?.error == "NotPermitted":
            throw .rejected(.refused(detail: error?.detail ?? ""))
        default:
            throw Self.unavailable(response)
        }
    }

    public func validate(key: String, activationID: String) async throws(LicenseBackendError) -> LicenseGrant {
        let body = ValidateRequest(
            key: key,
            organizationID: configuration.organizationID,
            activationID: activationID,
            benefitID: configuration.benefitID
        )
        let (data, response) = try await post("validate", body)
        switch response.statusCode {
        case 200:
            let license: LicenseKeyResponse = try decode(data)
            guard license.isForBenefit(configuration.benefitID) else {
                throw .rejected(.wrongProduct)
            }
            guard (license.activation?.id ?? activationID) == activationID else {
                throw .rejected(.activationRevoked)
            }
            return license.grant(activationID: activationID, key: key)
        case 404 where ErrorResponse.parse(data)?.error == "ResourceNotFound":
            throw .rejected(.activationRevoked)
        default:
            throw Self.unavailable(response)
        }
    }

    public func deactivate(key: String, activationID: String) async throws(LicenseBackendError) {
        let body = DeactivateRequest(
            key: key,
            organizationID: configuration.organizationID,
            activationID: activationID
        )
        let (data, response) = try await post("deactivate", body)
        switch response.statusCode {
        case 200 ..< 300:
            return
        case 404 where ErrorResponse.parse(data)?.error == "ResourceNotFound":
            return
        default:
            throw Self.unavailable(response)
        }
    }

    // MARK: - Transport

    private func post(_ action: String, _ body: some Encodable) async throws(LicenseBackendError) -> (Data, HTTPURLResponse) {
        let url = configuration.server.apiBaseURL
            .appendingPathComponent("v1/customer-portal/license-keys")
            .appendingPathComponent(action)
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue(userAgent, forHTTPHeaderField: "User-Agent")
        do {
            request.httpBody = try JSONEncoder().encode(body)
            return try await transport.send(request)
        } catch {
            throw .unavailable(reason: Self.describe(error))
        }
    }

    private func decode<T: Decodable>(_ data: Data) throws(LicenseBackendError) -> T {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom { decoder in
            let text = try decoder.singleValueContainer().decode(String.self)
            guard let date = Self.parseDate(text) else {
                throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: text))
            }
            return date
        }
        do {
            return try decoder.decode(T.self, from: data)
        } catch {
            throw .unavailable(reason: "The license server sent a response this version of Noican can't read.")
        }
    }

    // MARK: - Helpers

    /// Polar timestamps carry microseconds (`2024-09-02T13:48:13.251621Z`);
    /// the fraction is dropped rather than trusted to a formatter.
    static func parseDate(_ text: String) -> Date? {
        var trimmed = text
        if let dot = trimmed.firstIndex(of: "."),
           let zone = trimmed[dot...].firstIndex(where: { $0 == "Z" || $0 == "+" || $0 == "-" }) {
            trimmed.removeSubrange(dot ..< zone)
        }
        return try? Date(trimmed, strategy: .iso8601)
    }

    private static func unavailable(_ response: HTTPURLResponse) -> LicenseBackendError {
        let code = response.statusCode
        let reason = switch code {
        case 429:
            "The license server is busy (rate limited). Noican will try again later."
        case 500...:
            "The license server had a problem (HTTP \(code)). Noican will try again later."
        default:
            "The license server answered unexpectedly (HTTP \(code))."
        }
        return .unavailable(reason: reason)
    }

    private static func describe(_ error: any Error) -> String {
        if let urlError = error as? URLError {
            switch urlError.code {
            case .notConnectedToInternet, .networkConnectionLost, .dataNotAllowed:
                return "This Mac is offline."
            case .timedOut:
                return "The license server didn't respond in time."
            case .cannotFindHost, .cannotConnectToHost, .dnsLookupFailed:
                return "The license server can't be reached."
            default:
                break
            }
        }
        return "Couldn't reach the license server: \(error.localizedDescription)"
    }

    /// Polar limits labels and metadata values to 500 characters and
    /// rejects empty metadata values; keys are limited to 40.
    private static func clamped(_ text: String) -> String {
        String(text.prefix(500))
    }

    private static func meta(_ metadata: [String: String]) -> [String: String] {
        metadata.reduce(into: [:]) { result, entry in
            let key = String(entry.key.prefix(40))
            let value = clamped(entry.value)
            if !key.isEmpty, !value.isEmpty {
                result[key] = value
            }
        }
    }
}

// MARK: - Wire format

private struct ActivateRequest: Encodable {
    var key: String
    var organizationID: String
    var label: String
    var meta: [String: String]

    enum CodingKeys: String, CodingKey {
        case key
        case organizationID = "organization_id"
        case label
        case meta
    }
}

private struct ValidateRequest: Encodable {
    var key: String
    var organizationID: String
    var activationID: String
    var benefitID: String?

    enum CodingKeys: String, CodingKey {
        case key
        case organizationID = "organization_id"
        case activationID = "activation_id"
        case benefitID = "benefit_id"
    }
}

private struct DeactivateRequest: Encodable {
    var key: String
    var organizationID: String
    var activationID: String

    enum CodingKeys: String, CodingKey {
        case key
        case organizationID = "organization_id"
        case activationID = "activation_id"
    }
}

/// Only the fields the app acts on, and all but the key's own ID
/// optional: a field a later API version drops must not turn a valid
/// license into an unreadable one.
private struct LicenseKeyResponse: Decodable {
    struct Activation: Decodable {
        var id: String
    }

    var id: String
    var benefitID: String?
    var displayKey: String?
    var limitActivations: Int?
    var expiresAt: Date?
    var activation: Activation?

    enum CodingKeys: String, CodingKey {
        case id
        case benefitID = "benefit_id"
        case displayKey = "display_key"
        case limitActivations = "limit_activations"
        case expiresAt = "expires_at"
        case activation
    }

    func isForBenefit(_ expected: String?) -> Bool {
        guard let expected, let benefitID else {
            return true
        }
        return benefitID == expected
    }

    func grant(activationID: String, key: String) -> LicenseGrant {
        LicenseGrant(
            activationID: activationID,
            displayKey: displayKey ?? "****-" + String(key.suffix(6)),
            expiresAt: expiresAt,
            activationLimit: limitActivations
        )
    }
}

private struct ActivationResponse: Decodable {
    var id: String
    var licenseKey: LicenseKeyResponse

    enum CodingKeys: String, CodingKey {
        case id
        case licenseKey = "license_key"
    }
}

/// Polar's error body: `{"error": "ResourceNotFound", "detail": "…"}`.
/// Validation errors (422) carry an array in `detail`, read as nil.
private struct ErrorResponse: Decodable {
    var error: String?
    var detail: String?

    enum CodingKeys: String, CodingKey {
        case error
        case detail
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        error = try? container.decode(String.self, forKey: .error)
        detail = try? container.decode(String.self, forKey: .detail)
    }

    static func parse(_ data: Data) -> ErrorResponse? {
        try? JSONDecoder().decode(ErrorResponse.self, from: data)
    }
}
