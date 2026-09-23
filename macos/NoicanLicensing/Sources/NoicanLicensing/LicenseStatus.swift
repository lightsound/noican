import Foundation

/// What the popover shows about the activation.
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

/// The license as it stands on this Mac right now.
public enum LicenseStatus: Hashable, Sendable {
    /// The build carries no license-server configuration (development
    /// builds before the Polar organization exists).
    case unconfigured
    /// No activation on this Mac.
    case unlicensed
    /// Verified within the revalidation interval, or not yet due.
    case active(LicenseSummary)
    /// The last verification attempt could not reach the server; the
    /// license keeps working offline until `graceEndsAt`.
    case offline(LicenseSummary, graceEndsAt: Date)
    /// The grace period ran out without a successful verification.
    case verificationRequired(LicenseSummary)
    /// The server's last definitive answer was no.
    case rejected(LicenseSummary, LicenseRejection)

    /// The one switch for unlicensed behavior: whether noise cancellation
    /// (Preview and On) may be started. Everything else in the app —
    /// Off, the microphone and model pickers, the driver itself — works
    /// regardless of the license.
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

/// How often the license is re-checked and how long it works offline.
public struct LicensePolicy: Hashable, Sendable {
    /// A validation older than this is repeated at the next opportunity
    /// (launch or the hourly check).
    public var revalidationInterval: TimeInterval
    /// How long a license keeps working without a successful validation,
    /// counted from the last one.
    public var offlineGracePeriod: TimeInterval

    public static let standard = LicensePolicy(
        revalidationInterval: 24 * 60 * 60,
        offlineGracePeriod: 30 * 24 * 60 * 60
    )

    public init(revalidationInterval: TimeInterval, offlineGracePeriod: TimeInterval) {
        self.revalidationInterval = revalidationInterval
        self.offlineGracePeriod = offlineGracePeriod
    }

    /// Classifies a stored activation that belongs to this backend and
    /// Mac. `verificationFailed` is whether the latest attempt to reach
    /// the server failed.
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

    /// Whether the activation should be validated again. A validation
    /// time in the future (the clock was set back) is due as well.
    public func isRevalidationDue(_ license: StoredLicense, now: Date) -> Bool {
        let elapsed = now.timeIntervalSince(license.lastValidatedAt)
        return elapsed < 0 || elapsed >= revalidationInterval
    }
}
