import Foundation

public struct LicenseSummary: Hashable, Sendable {
    public var displayKey: String
    public var lastValidatedAt: Date
    public var expiresAt: Date?
    public var activationLimit: Int?

    init(_ license: StoredLicense) {
        displayKey = license.displayKey
        lastValidatedAt = license.lastValidatedAt
        expiresAt = license.expiresAt
        activationLimit = license.activationLimit
    }
}

public enum LicenseStatus: Hashable, Sendable {
    case unconfigured
    case unlicensed
    case active(LicenseSummary)
    case offline(LicenseSummary, graceEndsAt: Date)
    case verificationRequired(LicenseSummary)
    case rejected(LicenseSummary, LicenseRejection)

    public var allowsProcessing: Bool {
        switch self {
        case .unconfigured, .active, .offline:
            true
        case .unlicensed, .verificationRequired, .rejected:
            false
        }
    }

    public var summary: LicenseSummary? {
        switch self {
        case .unconfigured, .unlicensed:
            nil
        case let .active(summary), let .offline(summary, _),
             let .verificationRequired(summary), let .rejected(summary, _):
            summary
        }
    }
}

public struct LicensePolicy: Hashable, Sendable {
    public var revalidationInterval: TimeInterval
    public var offlineGracePeriod: TimeInterval

    public static let standard = LicensePolicy(
        revalidationInterval: 24 * 60 * 60,
        offlineGracePeriod: 30 * 24 * 60 * 60
    )

    public init(revalidationInterval: TimeInterval, offlineGracePeriod: TimeInterval) {
        self.revalidationInterval = revalidationInterval
        self.offlineGracePeriod = offlineGracePeriod
    }

    public func status(of license: StoredLicense, verificationFailed: Bool, now: Date) -> LicenseStatus {
        let summary = LicenseSummary(license)
        if let rejection = license.rejection {
            return .rejected(summary, rejection)
        }
        if let expiry = license.expiresAt, expiry <= now {
            return .rejected(summary, .expired(expiry))
        }
        let graceEnd = license.lastValidatedAt.addingTimeInterval(offlineGracePeriod)
        guard now < graceEnd else {
            return .verificationRequired(summary)
        }
        return verificationFailed ? .offline(summary, graceEndsAt: graceEnd) : .active(summary)
    }

    public func isRevalidationDue(_ license: StoredLicense, now: Date) -> Bool {
        let elapsed = now.timeIntervalSince(license.lastValidatedAt)
        return elapsed < 0 || elapsed >= revalidationInterval
    }
}
