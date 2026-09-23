import Foundation

/// The operation in flight, if any. At most one runs at a time; requests
/// made while one runs are ignored (the UI disables its buttons).
public enum LicenseActivity: Hashable, Sendable {
    case activating
    case verifying
    case deactivating
}

/// Everything the license UI renders.
public struct LicenseState: Hashable, Sendable {
    public var status: LicenseStatus
    public var activity: LicenseActivity?
    /// Outcome of the last operation that needs explaining (a refused
    /// activation, an unreachable server), cleared when the next one
    /// starts.
    public var notice: String?

    public init(status: LicenseStatus, activity: LicenseActivity? = nil, notice: String? = nil) {
        self.status = status
        self.activity = activity
        self.notice = notice
    }
}

/// Runs activation, validation, and deactivation against a
/// `LicenseBackend` and keeps the Keychain record and the published
/// `LicenseState` in step. Main-actor bound: it is driven by the UI and
/// its state feeds the menu directly.
@MainActor
public final class LicenseController {
    public private(set) var state: LicenseState {
        didSet {
            if state != oldValue {
                onChange?(state)
            }
        }
    }

    /// Called on every state change.
    public var onChange: ((LicenseState) -> Void)?

    private let backend: (any LicenseBackend)?
    private let store: any LicenseStore
    private let device: DeviceDescriptor
    private let policy: LicensePolicy
    private let now: () -> Date
    private var record: StoredLicense?
    private var verificationFailed = false
    /// When the server last rejected the stored record in this session.
    private var lastRejection: Date?

    /// `backend` is nil when the build has no license-server
    /// configuration; the status is then `.unconfigured`.
    public init(
        backend: (any LicenseBackend)?,
        store: any LicenseStore,
        device: DeviceDescriptor,
        policy: LicensePolicy = .standard,
        now: @escaping () -> Date = Date.init
    ) {
        self.backend = backend
        self.store = store
        self.device = device
        self.policy = policy
        self.now = now
        var notice: String?
        do {
            record = try store.load()
        } catch {
            notice = error.localizedDescription
        }
        state = LicenseState(status: .unlicensed, notice: notice)
        state.status = evaluate()
    }

    public var managementURL: URL? {
        backend?.managementURL
    }

    // MARK: - Operations

    /// Launch, hourly, and wake-from-sleep entry point: re-activates a
    /// record issued by another backend or for another Mac with its
    /// stored key, and re-validates one that is due — including a
    /// rejected record, so a mistaken rejection heals by itself.
    public func refreshIfDue() async {
        guard state.activity == nil, let backend, let record else {
            state.status = evaluate()
            return
        }
        if !isOwn(record, backend) {
            await activate(key: record.key, backend: backend)
        } else if isRecheckDue(record) {
            await validate(record, backend: backend)
        } else {
            state.status = evaluate()
        }
    }

    /// A rejected record is re-checked once per revalidation interval
    /// (counted from the rejection, since its last success is old);
    /// anything else when its last success is due for renewal.
    private func isRecheckDue(_ license: StoredLicense) -> Bool {
        guard license.rejection != nil else {
            return policy.isRevalidationDue(license, now: now())
        }
        guard let lastRejection else {
            return true
        }
        return now().timeIntervalSince(lastRejection) >= policy.revalidationInterval
    }

    /// The "Verify now" button.
    public func verifyNow() async {
        guard state.activity == nil, let backend, let record, isOwn(record, backend) else {
            return
        }
        await validate(record, backend: backend)
    }

    /// Activates a key the customer entered. When this Mac still holds an
    /// activation (for example after the customer rotated the key in the
    /// portal — activations survive a rotation), the new key is first
    /// tried against it, so no second device slot is spent.
    public func activate(key rawKey: String) async {
        let key = rawKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard state.activity == nil, let backend, !key.isEmpty else {
            return
        }
        if let record, isOwn(record, backend) {
            begin(.activating)
            do throws(LicenseBackendError) {
                let grant = try await backend.validate(key: key, activationID: record.activationID)
                finish(saving: newRecord(key: key, grant: grant, backend: backend))
                return
            } catch {
                if case let .unavailable(reason) = error {
                    finish(notice: reason)
                    return
                }
            }
        }
        await activate(key: key, backend: backend)
    }

    /// Releases this Mac's device slot and forgets the key. An activation
    /// issued for another Mac (a Keychain carried over by Migration
    /// Assistant) is forgotten locally only: releasing it on the server
    /// would deactivate the other Mac.
    public func deactivate() async {
        guard state.activity == nil, let record else {
            return
        }
        guard let backend, isOwn(record, backend) else {
            finishDeleting()
            return
        }
        begin(.deactivating)
        do throws(LicenseBackendError) {
            try await backend.deactivate(key: record.key, activationID: record.activationID)
            finishDeleting()
        } catch {
            switch error {
            case let .unavailable(reason):
                finish(notice: "This Mac is still activated. \(reason)")
            case let .rejected(rejection):
                finish(notice: rejection.message)
            }
        }
    }

    // MARK: - Steps

    private func activate(key: String, backend: any LicenseBackend) async {
        begin(.activating)
        do throws(LicenseBackendError) {
            let grant = try await backend.activate(key: key, device: device)
            finish(saving: newRecord(key: key, grant: grant, backend: backend))
        } catch {
            switch error {
            case let .unavailable(reason):
                finish(notice: reason)
            case let .rejected(rejection):
                finish(notice: rejection.message)
            }
        }
    }

    private func validate(_ license: StoredLicense, backend: any LicenseBackend) async {
        begin(.verifying)
        var updated = license
        do throws(LicenseBackendError) {
            let grant = try await backend.validate(key: license.key, activationID: license.activationID)
            updated.apply(grant, validatedAt: now())
            finish(saving: updated)
        } catch {
            switch error {
            case let .unavailable(reason):
                verificationFailed = true
                finish(notice: reason)
            case let .rejected(rejection):
                updated.rejection = rejection
                lastRejection = now()
                finish(saving: updated)
            }
        }
    }

    private func begin(_ activity: LicenseActivity) {
        state.activity = activity
        state.notice = nil
    }

    /// A definitive answer from the server (yes or no) was received:
    /// persist it. A Keychain failure is reported but does not undo the
    /// answer for this session.
    private func finish(saving license: StoredLicense) {
        record = license
        verificationFailed = false
        var notice: String?
        do {
            try store.save(license)
        } catch {
            notice = error.localizedDescription
        }
        state = LicenseState(status: evaluate(), notice: notice)
    }

    private func finishDeleting() {
        var notice: String?
        do {
            try store.delete()
        } catch {
            notice = error.localizedDescription
        }
        record = nil
        verificationFailed = false
        state = LicenseState(status: evaluate(), notice: notice)
    }

    private func finish(notice: String) {
        state = LicenseState(status: evaluate(), notice: notice)
    }

    private func newRecord(key: String, grant: LicenseGrant, backend: any LicenseBackend) -> StoredLicense {
        StoredLicense(backendID: backend.identifier, deviceID: device.id, key: key, grant: grant, lastValidatedAt: now())
    }

    private func isOwn(_ license: StoredLicense, _ backend: any LicenseBackend) -> Bool {
        license.belongs(toBackend: backend.identifier, device: device.id)
    }

    private func evaluate() -> LicenseStatus {
        guard let backend else {
            return .unconfigured
        }
        guard let record, isOwn(record, backend) else {
            return .unlicensed
        }
        return policy.status(of: record, verificationFailed: verificationFailed, now: now())
    }
}
