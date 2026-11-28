import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

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
    public var organizationID: String
    public var benefitID: String
    public var organizationSlug: String?

    public init(server: Server, organizationID: String, benefitID: String, organizationSlug: String? = nil) {
        self.server = server
        self.organizationID = organizationID.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        self.benefitID = benefitID.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        self.organizationSlug = organizationSlug
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .flatMap { $0.isEmpty ? nil : $0 }
    }

    public var isComplete: Bool {
        UUID(uuidString: organizationID) != nil && UUID(uuidString: benefitID) != nil
    }

    public var customerPortalURL: URL? {
        organizationSlug.map {
            server.siteBaseURL.appendingPathComponent($0).appendingPathComponent("portal")
        }
    }
}

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

public struct PolarLicenseBackend: LicenseBackend {
    public let identifier = "polar"

    private static let unreadableResponseReason = "The license server sent a response this version of Noican can't read."

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
                try? await deactivate(key: key, activationID: activation.id)
                throw .rejected(.wrongProduct)
            }
            guard activation.licenseKey.isGranted else {
                try? await deactivate(key: key, activationID: activation.id)
                throw .rejected(.refused(detail: "the license key is not active"))
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
            if let benefitID = license.benefitID, benefitID != configuration.benefitID {
                throw .rejected(.wrongProduct)
            }
            if let status = license.status, status != "granted" {
                throw .rejected(.activationRevoked)
            }
            if let activation = license.activation, activation.id != activationID {
                throw .rejected(.activationRevoked)
            }
            guard license.benefitID != nil, license.status != nil, license.activation != nil else {
                throw .unavailable(reason: Self.unreadableResponseReason)
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
            throw .unavailable(reason: Self.unreadableResponseReason)
        }
    }

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
    var benefitID: String

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

private struct LicenseKeyResponse: Decodable {
    struct Activation: Decodable {
        var id: String
    }

    var id: String
    var status: String?
    var benefitID: String?
    var displayKey: String?
    var limitActivations: Int?
    var expiresAt: Date?
    var activation: Activation?

    enum CodingKeys: String, CodingKey {
        case id
        case status
        case benefitID = "benefit_id"
        case displayKey = "display_key"
        case limitActivations = "limit_activations"
        case expiresAt = "expires_at"
        case activation
    }

    var isGranted: Bool {
        status == "granted"
    }

    func isForBenefit(_ expected: String) -> Bool {
        benefitID == expected
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
