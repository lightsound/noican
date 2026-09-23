import Foundation
import Testing

@testable import NoicanLicensing

@MainActor
private func controller(
    backend: MockBackend? = MockBackend(),
    store: InMemoryLicenseStore = InMemoryLicenseStore(),
    clock: TestClock = TestClock()
) -> LicenseController {
    LicenseController(backend: backend, store: store, device: thisMac, now: { clock.now })
}

@MainActor
private func isActive(_ controller: LicenseController) -> Bool {
    if case .active = controller.state.status {
        return true
    }
    return false
}

@Suite("License controller: activation")
@MainActor
struct LicenseActivationTests {
    @Test("A new key is activated, stored, and allows processing")
    func activateNewKey() async {
        let backend = MockBackend()
        let store = InMemoryLicenseStore()
        let license = controller(backend: backend, store: store)
        #expect(license.state.status == .unlicensed)
        await license.activate(key: "  KEY-1\n")
        #expect(backend.calls == [.activate(key: "KEY-1", deviceID: thisMac.id)], "whitespace from pasting is trimmed")
        #expect(isActive(license))
        #expect(license.state.status.allowsProcessing)
        #expect(store.license?.key == "KEY-1")
        #expect(store.license?.activationID == "act-1")
        #expect(store.license?.backendID == "mock")
        #expect(store.license?.deviceID == thisMac.id)
    }

    @Test("A refused activation explains itself and stores nothing")
    func activationRefused() async {
        let backend = MockBackend()
        backend.onActivate(.failure(.rejected(.refused(detail: "License key only supports 3 activations"))))
        let store = InMemoryLicenseStore()
        let license = controller(backend: backend, store: store)
        await license.activate(key: "KEY-1")
        #expect(license.state.status == .unlicensed)
        #expect(license.state.notice?.contains("only supports 3 activations") == true)
        #expect(license.state.activity == nil)
        #expect(store.license == nil)
    }

    @Test("An unreachable server during activation keeps the Mac unlicensed with the reason")
    func activationOffline() async {
        let backend = MockBackend()
        backend.onActivate(.failure(.unavailable(reason: "This Mac is offline.")))
        let license = controller(backend: backend)
        await license.activate(key: "KEY-1")
        #expect(license.state.status == .unlicensed)
        #expect(license.state.notice == "This Mac is offline.")
    }

    @Test("An empty key does nothing")
    func emptyKey() async {
        let backend = MockBackend()
        let license = controller(backend: backend)
        await license.activate(key: "   ")
        #expect(backend.calls.isEmpty)
    }

    @Test("A rotated key re-uses this Mac's activation instead of spending a slot")
    func rotatedKeyReusesActivation() async {
        let backend = MockBackend()
        let store = InMemoryLicenseStore(storedLicense(key: "OLD", rejection: .activationRevoked))
        let license = controller(backend: backend, store: store)
        await license.activate(key: "NEW")
        #expect(backend.calls == [.validate(key: "NEW", activationID: "act-1")])
        #expect(store.license?.key == "NEW")
        #expect(store.license?.rejection == nil)
        #expect(isActive(license))
    }

    @Test("When the old activation is gone too, the key is activated afresh")
    func releasedActivationFallsBackToActivate() async {
        let backend = MockBackend()
        backend.onValidate(.failure(.rejected(.activationRevoked)))
        backend.onActivate(.success(grant("act-2")))
        let store = InMemoryLicenseStore(storedLicense(rejection: .activationRevoked))
        let license = controller(backend: backend, store: store)
        await license.activate(key: "KEY-1")
        #expect(backend.calls == [
            .validate(key: "KEY-1", activationID: "act-1"),
            .activate(key: "KEY-1", deviceID: thisMac.id)
        ])
        #expect(store.license?.activationID == "act-2")
        #expect(isActive(license))
    }

    @Test("A Keychain failure is reported but the activation holds for the session")
    func keychainSaveFailure() async {
        let store = InMemoryLicenseStore()
        store.failSaves = true
        let license = controller(store: store)
        await license.activate(key: "KEY-1")
        #expect(isActive(license))
        #expect(license.state.notice == "Keychain unavailable")
    }

    @Test("Without a backend configuration the controller is inert and allows processing")
    func unconfigured() async {
        let license = controller(backend: nil)
        #expect(license.state.status == .unconfigured)
        #expect(license.state.status.allowsProcessing)
        await license.activate(key: "KEY-1")
        await license.refreshIfDue()
        #expect(license.state.status == .unconfigured)
    }

    @Test("State changes are reported through onChange")
    func onChangeFires() async {
        let license = controller()
        var activities: [LicenseActivity?] = []
        license.onChange = { activities.append($0.activity) }
        await license.activate(key: "KEY-1")
        #expect(activities == [.activating, nil])
    }
}

@Suite("License controller: revalidation and grace")
@MainActor
struct LicenseRevalidationTests {
    @Test("Launch with a recent validation makes no request")
    func recentValidationIsTrusted() async {
        let backend = MockBackend()
        let clock = TestClock()
        clock.advance(2 * hour)
        let license = controller(backend: backend, store: InMemoryLicenseStore(storedLicense()), clock: clock)
        #expect(isActive(license), "the stored record is trusted before any network round trip")
        await license.refreshIfDue()
        #expect(backend.calls.isEmpty)
    }

    @Test("A due validation succeeds and restarts the clock")
    func dueValidationRenews() async {
        let backend = MockBackend()
        let store = InMemoryLicenseStore(storedLicense())
        let clock = TestClock()
        clock.advance(2 * day)
        let license = controller(backend: backend, store: store, clock: clock)
        await license.refreshIfDue()
        #expect(backend.calls == [.validate(key: "KEY-1", activationID: "act-1")])
        #expect(store.license?.lastValidatedAt == launchTime + 2 * day)
        #expect(isActive(license))
    }

    @Test("Offline, the license works through the grace period and then requires verification")
    func graceLapses() async {
        let backend = MockBackend()
        backend.onValidate(.failure(.unavailable(reason: "This Mac is offline.")))
        let store = InMemoryLicenseStore(storedLicense())
        let clock = TestClock()
        clock.advance(2 * day)
        let license = controller(backend: backend, store: store, clock: clock)
        await license.refreshIfDue()
        #expect(license.state.status == .offline(LicenseSummary(storedLicense()), graceEndsAt: launchTime + 30 * day))
        #expect(license.state.status.allowsProcessing)
        #expect(license.state.notice == "This Mac is offline.")
        #expect(store.license == storedLicense(), "a transient failure changes nothing on disk")

        clock.advance(28 * day)
        await license.refreshIfDue()
        #expect(license.state.status == .verificationRequired(LicenseSummary(storedLicense())))
        #expect(!license.state.status.allowsProcessing)

        backend.onValidate(.success(grant()))
        await license.verifyNow()
        #expect(isActive(license), "coming back online restores the license")
    }

    @Test("A definitive rejection blocks, is persisted, and heals when the server says yes again")
    func rejectionHeals() async {
        let backend = MockBackend()
        backend.onValidate(.failure(.rejected(.activationRevoked)))
        let store = InMemoryLicenseStore(storedLicense())
        let clock = TestClock()
        clock.advance(2 * day)
        let license = controller(backend: backend, store: store, clock: clock)
        await license.refreshIfDue()
        #expect(license.state.status == .rejected(LicenseSummary(storedLicense()), .activationRevoked))
        #expect(!license.state.status.allowsProcessing)
        #expect(store.license?.rejection == .activationRevoked)
        #expect(store.license?.key == "KEY-1", "the key is kept for the re-check")

        clock.advance(hour)
        await license.refreshIfDue()
        #expect(backend.calls.count == 1, "a rejection is re-checked once per revalidation interval, not hourly")

        backend.onValidate(.success(grant()))
        clock.advance(day)
        await license.refreshIfDue()
        #expect(backend.calls.count == 2)
        #expect(isActive(license))
        #expect(store.license?.rejection == nil)
    }

    @Test("A rejected record is re-checked at the next launch")
    func rejectedRecordRecheckedAtLaunch() async {
        let backend = MockBackend()
        let license = controller(backend: backend, store: InMemoryLicenseStore(storedLicense(rejection: .activationRevoked)))
        #expect(!license.state.status.allowsProcessing)
        await license.refreshIfDue()
        #expect(backend.calls == [.validate(key: "KEY-1", activationID: "act-1")])
        #expect(isActive(license))
    }
}

@Suite("License controller: backend swap, migrated Macs, deactivation")
@MainActor
struct LicenseMigrationTests {
    @Test("A record from another backend is re-activated with the stored key — no re-entry")
    func backendSwapReactivates() async {
        let backend = MockBackend(identifier: "keygen")
        backend.onActivate(.success(grant("kg-1")))
        let store = InMemoryLicenseStore(storedLicense(backendID: "polar"))
        let license = controller(backend: backend, store: store)
        #expect(license.state.status == .unlicensed, "the old backend's activation is not trusted")
        await license.refreshIfDue()
        #expect(backend.calls == [.activate(key: "KEY-1", deviceID: thisMac.id)])
        #expect(store.license?.backendID == "keygen")
        #expect(store.license?.activationID == "kg-1")
        #expect(isActive(license))
    }

    @Test("A Keychain carried over from another Mac is activated for this Mac")
    func migratedMacReactivates() async {
        let backend = MockBackend()
        backend.onActivate(.success(grant("act-2")))
        let store = InMemoryLicenseStore(storedLicense(device: otherMac))
        let license = controller(backend: backend, store: store)
        await license.refreshIfDue()
        #expect(backend.calls == [.activate(key: "KEY-1", deviceID: thisMac.id)])
        #expect(store.license?.deviceID == thisMac.id)
    }

    @Test("A refused re-activation is retried daily, an unreachable server hourly")
    func migrationRetryCadence() async {
        let backend = MockBackend()
        backend.onActivate(.failure(.unavailable(reason: "This Mac is offline.")))
        let clock = TestClock()
        let license = controller(backend: backend, store: InMemoryLicenseStore(storedLicense(device: otherMac)), clock: clock)
        await license.refreshIfDue()
        clock.advance(hour)
        await license.refreshIfDue()
        #expect(backend.calls.count == 2, "offline: the next hourly tick tries again")

        backend.onActivate(.failure(.rejected(.refused(detail: "License key only supports 3 activations"))))
        clock.advance(hour)
        await license.refreshIfDue()
        clock.advance(hour)
        await license.refreshIfDue()
        #expect(backend.calls.count == 3, "after a definitive no, not again within the day")
        clock.advance(day)
        await license.refreshIfDue()
        #expect(backend.calls.count == 4)
    }

    @Test("Deactivation releases the slot and forgets the key")
    func deactivateReleases() async {
        let backend = MockBackend()
        let store = InMemoryLicenseStore(storedLicense())
        let license = controller(backend: backend, store: store)
        await license.deactivate()
        #expect(backend.calls == [.deactivate(key: "KEY-1", activationID: "act-1")])
        #expect(store.license == nil)
        #expect(license.state.status == .unlicensed)
    }

    @Test("A failed deactivation keeps the Mac activated")
    func deactivateOffline() async {
        let backend = MockBackend()
        backend.onDeactivate(.failure(.unavailable(reason: "This Mac is offline.")))
        let store = InMemoryLicenseStore(storedLicense())
        let license = controller(backend: backend, store: store)
        await license.deactivate()
        #expect(store.license == storedLicense())
        #expect(isActive(license))
        #expect(license.state.notice == "This Mac is still activated. This Mac is offline.")
    }

    @Test("Removing another Mac's activation never deactivates it on the server")
    func deactivateForeignRecordLocally() async {
        let backend = MockBackend()
        let store = InMemoryLicenseStore(storedLicense(device: otherMac))
        let license = controller(backend: backend, store: store)
        await license.deactivate()
        #expect(backend.calls.isEmpty)
        #expect(store.license == nil)
    }
}
