import Foundation
import Testing

@testable import NoicanLicensing

@Suite("License status and grace period")
struct LicensePolicyTests {
    let policy = LicensePolicy.standard

    @Test("A fresh validation is active and allows processing")
    func freshIsActive() {
        let status = policy.status(of: storedLicense(), verificationFailed: false, now: launchTime + hour)
        guard case .active = status else {
            Issue.record("expected active, got \(status)")
            return
        }
        #expect(status.allowsProcessing)
    }

    @Test("A failed verification inside the grace period keeps working offline")
    func offlineGrace() {
        let status = policy.status(of: storedLicense(), verificationFailed: true, now: launchTime + 10 * day)
        #expect(status == .offline(LicenseSummary(storedLicense()), graceEndsAt: launchTime + 30 * day))
        #expect(status.allowsProcessing)
    }

    @Test("The grace period ends exactly 30 days after the last success")
    func graceBoundary() {
        let license = storedLicense()
        let justInside = policy.status(of: license, verificationFailed: true, now: launchTime + 30 * day - 1)
        #expect(justInside.allowsProcessing)
        let atEnd = policy.status(of: license, verificationFailed: true, now: launchTime + 30 * day)
        #expect(atEnd == .verificationRequired(LicenseSummary(license)))
        #expect(!atEnd.allowsProcessing)
    }

    @Test("Rejections and passed expiry dates block processing")
    func rejectionsBlock() {
        let revoked = storedLicense(rejection: .activationRevoked)
        let revokedStatus = policy.status(of: revoked, verificationFailed: false, now: launchTime)
        #expect(revokedStatus == .rejected(LicenseSummary(revoked), .activationRevoked))
        var expiring = storedLicense()
        expiring.expiresAt = launchTime + day
        #expect(policy.status(of: expiring, verificationFailed: false, now: launchTime).allowsProcessing)
        let expired = policy.status(of: expiring, verificationFailed: false, now: launchTime + day)
        #expect(expired == .rejected(LicenseSummary(expiring), .expired(launchTime + day)))
        #expect(!expired.allowsProcessing)
    }

    @Test("Unconfigured builds allow processing; no license does not")
    func edgeStatuses() {
        #expect(LicenseStatus.unconfigured.allowsProcessing)
        #expect(!LicenseStatus.unlicensed.allowsProcessing)
    }

    @Test("Revalidation is due after 24 hours, or when the clock went backwards")
    func revalidationDue() {
        let license = storedLicense()
        #expect(!policy.isRevalidationDue(license, now: launchTime + 23 * hour))
        #expect(policy.isRevalidationDue(license, now: launchTime + day))
        #expect(policy.isRevalidationDue(license, now: launchTime - hour))
    }

    @Test("The stored record round-trips through its Keychain encoding")
    func storedLicenseRoundTrip() throws {
        var license = storedLicense(rejection: .refused(detail: "limit"))
        license.expiresAt = launchTime + day
        #expect(try StoredLicense.decode(license.encoded()) == license)
    }

    @Test("Rejection messages read as sentences")
    func rejectionMessages() {
        #expect(LicenseRejection.refused(detail: "License key only supports 3 activations").message.hasPrefix(
            "Activation refused: License key only supports 3 activations. To move"
        ))
        #expect(LicenseRejection.refused(detail: "").message.contains("gave no reason."))
    }
}
