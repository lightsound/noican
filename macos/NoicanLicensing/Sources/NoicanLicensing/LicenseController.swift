import Foundation

public enum LicenseActivity: Hashable, Sendable {
    case activating
    case verifying
    case deactivating
}

public struct LicenseState: Hashable, Sendable {
    public var status: LicenseStatus
    public var activity: LicenseActivity?
    public var notice: String?

    public init(status: LicenseStatus, activity: LicenseActivity? = nil, notice: String? = nil) {
        self.status = status
        self.activity = activity
        self.notice = notice
    }
}

@MainActor
public final class LicenseController {
    public private(set) var state: LicenseState {
        didSet {
            if state != oldValue {
                onChange?(state)
            }
        }
    }

    public var onChange: ((LicenseState) -> Void)?

    private let backend: (any LicenseBackend)?
    private let store: any LicenseStore
    private let device: DeviceDescriptor
    private let policy: LicensePolicy
    private let now: () -> Date
    private var record: StoredLicense?
    private var needsReload = false
    private var verificationFailed = false
    private var lastRejection: Date?

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
        state = LicenseState(status: .unlicensed)
        needsReload = true
        reloadIfNeeded()
    }

    private func reloadIfNeeded() {
        guard needsReload else {
            return
        }
        do {
            record = try store.load()
            needsReload = false
            state = LicenseState(status: evaluate(), activity: state.activity)
        } catch {
            state = LicenseState(status: evaluate(), activity: state.activity, notice: error.localizedDescription)
        }
    }

    public var managementURL: URL? {
        backend?.managementURL
    }

    public func refreshIfDue() async {
        guard state.activity == nil else {
            return
        }
        reloadIfNeeded()
        guard let backend, let record, isRecheckDue(record, backend) else {
            state.status = evaluate()
            return
        }
        if isOwn(record, backend) {
            await validate(record, backend: backend)
        } else {
            await activate(key: record.key, backend: backend)
        }
    }

    private func isRecheckDue(_ license: StoredLicense, _ backend: any LicenseBackend) -> Bool {
        if isOwn(license, backend), license.rejection == nil {
            return policy.isRevalidationDue(license, now: now())
        }
        guard let lastRejection else {
            return true
        }
        let elapsed = now().timeIntervalSince(lastRejection)
        return elapsed < 0 || elapsed >= policy.revalidationInterval
    }

    public func verifyNow() async {
        guard state.activity == nil else {
            return
        }
        reloadIfNeeded()
        guard let backend, let record, isOwn(record, backend) else {
            return
        }
        await validate(record, backend: backend)
    }

    public func activate(key rawKey: String) async {
        let key = rawKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard state.activity == nil, let backend, !key.isEmpty else {
            return
        }
        reloadIfNeeded()
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

    public func deactivate() async {
        guard state.activity == nil else {
            return
        }
        reloadIfNeeded()
        guard let record else {
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
                lastRejection = now()
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

    private func finish(saving license: StoredLicense) {
        record = license
        needsReload = false
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
        needsReload = false
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
