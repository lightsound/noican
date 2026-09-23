import Foundation
import Testing

@testable import NoicanLicensing

// Request and response shapes follow Polar's customer-portal license-key
// reference (API version 2026-10): POST /v1/customer-portal/license-keys/
// {activate,validate,deactivate}. The response bodies below are the
// documented examples, trimmed to the fields the backend reads plus a
// few it must ignore.

private let organizationID = "fda84e25-7b55-4d67-916d-60ead04ff61f"
private let benefitID = "32a8eda4-56cf-4a94-8228-792d324a519e"

private let configuration = PolarConfiguration(
    server: .production,
    organizationID: organizationID,
    benefitID: benefitID,
    organizationSlug: "noican"
)

private func backend(_ transport: FakeTransport, benefit: String? = benefitID) -> PolarLicenseBackend {
    var configuration = configuration
    configuration.benefitID = benefit
    return PolarLicenseBackend(configuration: configuration, transport: transport, userAgent: "Noican/1.0.0")
}

private func licenseKeyJSON(benefit: String = benefitID, expiresAt: String = "null", activation: String = "null") -> String {
    """
    {
      "id": "508176f7-065a-4b5d-b524-4e9c8a11ed63",
      "organization_id": "\(organizationID)",
      "customer_id": "d910050c-be66-4ca0-b4cc-34fde514f227",
      "benefit_id": "\(benefit)",
      "key": "1C285B2D-6CE6-4BC7-B8BE-ADB6A7E304DA",
      "display_key": "****-E304DA",
      "status": "granted",
      "limit_activations": 3,
      "usage": 0,
      "limit_usage": null,
      "validations": 5,
      "last_validated_at": "2024-09-02T13:57:00.977363Z",
      "expires_at": \(expiresAt),
      "activation": \(activation)
    }
    """
}

private func activationJSON(benefit: String = benefitID) -> String {
    """
    {
      "id": "b6724bc8-7ad9-4ca0-b143-7c896fcbb6fe",
      "license_key_id": "508176f7-065a-4b5d-b524-4e9c8a11ed63",
      "label": "Test MacBook",
      "meta": {"app_version": "1.0.0"},
      "created_at": "2024-09-02T13:48:13.251621Z",
      "modified_at": null,
      "license_key": \(licenseKeyJSON(benefit: benefit))
    }
    """
}

private let ownActivation = """
{"id": "b6724bc8-7ad9-4ca0-b143-7c896fcbb6fe", "license_key_id": "508176f7-065a-4b5d-b524-4e9c8a11ed63",
 "label": "Test MacBook", "meta": {}, "created_at": "2024-09-02T13:48:13.251621Z", "modified_at": null}
"""

private let notFound = #"{"error": "ResourceNotFound", "detail": "License key not found"}"#
private let limitReached = #"{"error": "NotPermitted", "detail": "License key only supports 3 activations"}"#

@Suite("Polar backend: requests")
struct PolarRequestTests {
    @Test("Activate posts the key, organization, label, and metadata without a token or version pin")
    func activateRequest() async throws {
        let transport = FakeTransport(.http(200, activationJSON()))
        _ = try await backend(transport).activate(key: "KEY-1", device: thisMac)
        let request = try #require(transport.requests.first)
        #expect(request.url?.absoluteString == "https://api.polar.sh/v1/customer-portal/license-keys/activate")
        #expect(request.httpMethod == "POST")
        #expect(request.value(forHTTPHeaderField: "Content-Type") == "application/json")
        #expect(request.value(forHTTPHeaderField: "Authorization") == nil, "the public endpoints take no secret")
        #expect(request.value(forHTTPHeaderField: "Polar-Version") == nil, "a pinned version would 404 once removed")
        let body = request.jsonBody
        #expect(body["key"] as? String == "KEY-1")
        #expect(body["organization_id"] as? String == organizationID)
        #expect(body["label"] as? String == "Test MacBook")
        #expect(body["meta"] as? [String: String] == ["app_version": "1.0.0"])
        #expect(body["conditions"] == nil, "no server-side conditions: a hardware change must not lock the key")
    }

    @Test("Validate scopes the lookup to the configured benefit and this Mac's activation")
    func validateRequest() async throws {
        let transport = FakeTransport(.http(200, licenseKeyJSON(activation: ownActivation)))
        _ = try await backend(transport).validate(key: "KEY-1", activationID: "b6724bc8-7ad9-4ca0-b143-7c896fcbb6fe")
        let request = try #require(transport.requests.first)
        #expect(request.url?.path == "/v1/customer-portal/license-keys/validate")
        let body = request.jsonBody
        #expect(body["activation_id"] as? String == "b6724bc8-7ad9-4ca0-b143-7c896fcbb6fe")
        #expect(body["benefit_id"] as? String == benefitID)
        #expect(body["organization_id"] as? String == organizationID)
    }

    @Test("Without a benefit ID the validate body omits the field")
    func validateWithoutBenefit() async throws {
        let transport = FakeTransport(.http(200, licenseKeyJSON(activation: ownActivation)))
        _ = try await backend(transport, benefit: nil).validate(key: "KEY-1", activationID: "b6724bc8-7ad9-4ca0-b143-7c896fcbb6fe")
        #expect(transport.requests.first?.jsonBody["benefit_id"] == nil)
    }

    @Test("Deactivate posts the key and activation")
    func deactivateRequest() async throws {
        let transport = FakeTransport(.http(204, ""))
        try await backend(transport).deactivate(key: "KEY-1", activationID: "act-1")
        let request = try #require(transport.requests.first)
        #expect(request.url?.path == "/v1/customer-portal/license-keys/deactivate")
        #expect(request.jsonBody["activation_id"] as? String == "act-1")
    }

    @Test("The sandbox server and the customer portal URL follow the configuration")
    func sandboxURLs() async throws {
        let transport = FakeTransport(.http(204, ""))
        let sandbox = PolarConfiguration(server: .sandbox, organizationID: organizationID, organizationSlug: "noican")
        let backend = PolarLicenseBackend(configuration: sandbox, transport: transport, userAgent: "Noican")
        try await backend.deactivate(key: "KEY-1", activationID: "act-1")
        #expect(transport.requests.first?.url?.host == "sandbox-api.polar.sh")
        #expect(backend.managementURL?.absoluteString == "https://sandbox.polar.sh/noican/portal")
        #expect(PolarConfiguration(server: .production, organizationID: organizationID).customerPortalURL == nil)
    }

    @Test("Metadata values Polar would refuse are dropped or clamped")
    func metadataIsSanitized() async throws {
        let transport = FakeTransport(.http(200, activationJSON()))
        let device = DeviceDescriptor(
            id: "d",
            label: "",
            metadata: ["empty": "", "long": String(repeating: "x", count: 600)]
        )
        _ = try await backend(transport).activate(key: "KEY-1", device: device)
        let body = try #require(transport.requests.first).jsonBody
        #expect(body["label"] as? String == "Mac", "the label is required and must not be empty")
        let meta = try #require(body["meta"] as? [String: String])
        #expect(meta["empty"] == nil)
        #expect(meta["long"]?.count == 500)
    }

    @Test("Placeholder configuration is incomplete")
    func placeholderIsIncomplete() {
        #expect(!PolarConfiguration(server: .production, organizationID: "").isComplete)
        #expect(!PolarConfiguration(server: .production, organizationID: "YOUR-ORG-ID").isComplete)
        #expect(configuration.isComplete)
        #expect(PolarConfiguration(server: .production, organizationID: organizationID, benefitID: " ").benefitID == nil)
    }
}

@Suite("Polar backend: responses")
struct PolarResponseTests {
    @Test("A documented activation response becomes a grant")
    func activationGrant() async throws {
        let transport = FakeTransport(.http(200, activationJSON()))
        let grant = try await backend(transport).activate(key: "KEY-1", device: thisMac)
        #expect(grant.activationID == "b6724bc8-7ad9-4ca0-b143-7c896fcbb6fe")
        #expect(grant.displayKey == "****-E304DA")
        #expect(grant.activationLimit == 3)
        #expect(grant.expiresAt == nil)
    }

    @Test("Expiry timestamps with microseconds are read")
    func expiryIsParsed() async throws {
        let transport = FakeTransport(.http(200, licenseKeyJSON(expiresAt: #""2026-08-30T08:40:34.769148Z""#, activation: ownActivation)))
        let grant = try await backend(transport).validate(key: "KEY-1", activationID: "b6724bc8-7ad9-4ca0-b143-7c896fcbb6fe")
        #expect(grant.expiresAt == Date(timeIntervalSince1970: 1_788_079_234))
    }

    @Test("Activation 404 means an unknown key; 403 carries Polar's reason")
    func activationRejections() async {
        await #expect(throws: LicenseBackendError.rejected(.unknownKey)) {
            try await backend(FakeTransport(.http(404, notFound))).activate(key: "NOPE", device: thisMac)
        }
        await #expect(throws: LicenseBackendError.rejected(.refused(detail: "License key only supports 3 activations"))) {
            try await backend(FakeTransport(.http(403, limitReached))).activate(key: "KEY-1", device: thisMac)
        }
    }

    @Test("A key for another product is refused and its fresh activation released")
    func activationWrongProduct() async throws {
        let transport = FakeTransport(.http(200, activationJSON(benefit: "00000000-0000-4000-8000-000000000000")), .http(204, ""))
        await #expect(throws: LicenseBackendError.rejected(.wrongProduct)) {
            try await backend(transport).activate(key: "KEY-1", device: thisMac)
        }
        #expect(transport.requests.map(\.url?.lastPathComponent) == ["activate", "deactivate"])
    }

    @Test("Validation 404 means this Mac's activation is gone")
    func validationRevoked() async {
        await #expect(throws: LicenseBackendError.rejected(.activationRevoked)) {
            try await backend(FakeTransport(.http(404, notFound))).validate(key: "KEY-1", activationID: "act-1")
        }
    }

    @Test("A validation answering for another activation or benefit is a rejection")
    func validationMismatch() async {
        let other = licenseKeyJSON(activation: #"{"id": "other", "license_key_id": "x", "label": "", "meta": {}}"#)
        await #expect(throws: LicenseBackendError.rejected(.activationRevoked)) {
            try await backend(FakeTransport(.http(200, other))).validate(key: "KEY-1", activationID: "act-1")
        }
        let foreign = licenseKeyJSON(benefit: "00000000-0000-4000-8000-000000000000")
        await #expect(throws: LicenseBackendError.rejected(.wrongProduct)) {
            try await backend(FakeTransport(.http(200, foreign))).validate(key: "KEY-1", activationID: "act-1")
        }
    }

    @Test("Only a well-formed Polar error is definitive; everything else is unavailable")
    func transientFailures() async {
        let cases: [FakeTransport.Reply] = [
            .http(404, "<html>captive portal</html>"),
            .http(403, "Forbidden"),
            .http(429, #"{"error": "TooManyRequests"}"#),
            .http(500, ""),
            .http(422, #"{"detail": [{"loc": ["body", "key"], "msg": "field required", "type": "missing"}]}"#),
            .http(200, #"{"unexpected": true}"#),
            .failure(.notConnectedToInternet),
            .failure(.timedOut)
        ]
        for reply in cases {
            await #expect {
                try await backend(FakeTransport(reply)).validate(key: "KEY-1", activationID: "act-1")
            } throws: { error in
                guard case .unavailable = error as? LicenseBackendError else {
                    return false
                }
                return true
            }
        }
    }

    @Test("Deactivating an activation the server no longer knows succeeds")
    func deactivateMissing() async throws {
        try await backend(FakeTransport(.http(404, notFound))).deactivate(key: "KEY-1", activationID: "act-1")
        await #expect(throws: LicenseBackendError.unavailable(reason: "This Mac is offline.")) {
            try await backend(FakeTransport(.failure(.notConnectedToInternet))).deactivate(key: "KEY-1", activationID: "act-1")
        }
    }

    @Test("Fields a later API version drops do not invalidate a license")
    func minimalResponse() async throws {
        let transport = FakeTransport(.http(200, #"{"id": "act-9", "license_key": {"id": "lk-1"}}"#))
        let grant = try await backend(transport).activate(key: "1C285B2D-6CE6-4BC7-B8BE-ADB6A7E304DA", device: thisMac)
        #expect(grant.activationID == "act-9")
        #expect(grant.displayKey == "****-E304DA", "masked locally when the server sends no display key")
    }
}
