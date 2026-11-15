import CryptoKit
import Foundation
import IOKit
import Network
import NoicanLicensing
import SystemConfiguration

/// The app's license shell: builds the `LicenseController` from
/// `LicenseConfiguration`, publishes its state to the menu, forwards
/// whether noise cancellation may start to the engine reducer, and
/// re-checks the license hourly (the continuous clock keeps counting
/// through sleep, so the first tick after a long sleep comes right away)
/// and whenever the network comes back.
@MainActor
final class LicenseModel: ObservableObject {
    @Published private(set) var state: LicenseState

    /// Whether this build talks to Polar's sandbox (shown next to the
    /// status so a sandbox build is never mistaken for a release).
    let isSandbox: Bool
    let purchaseURL = LicenseConfiguration.purchaseURL

    private let controller: LicenseController
    private let pathMonitor = NWPathMonitor()
    private var isNetworkSatisfied: Bool?

    init(onAllowanceChange: @escaping (Bool) -> Void) {
        let configuration = LicenseConfiguration.polar
        let backend: PolarLicenseBackend? = configuration.isComplete
            ? PolarLicenseBackend(
                configuration: configuration,
                transport: URLSessionTransport(),
                userAgent: "Noican/\(Self.appVersion) (macOS \(Self.macOSVersion))"
            )
            : nil
        isSandbox = backend != nil && configuration.server == .sandbox
        controller = LicenseController(backend: backend, store: KeychainLicenseStore(), device: Self.thisMac())
        state = controller.state
        onAllowanceChange(controller.state.status.allowsProcessing)
        controller.onChange = { [weak self] newState in
            self?.state = newState
            onAllowanceChange(newState.status.allowsProcessing)
        }
        guard backend != nil else {
            return
        }
        Task { [weak self] in
            while !Task.isCancelled, let controller = self?.controller {
                await controller.refreshIfDue()
                try? await Task.sleep(for: .seconds(60 * 60))
            }
        }
        pathMonitor.pathUpdateHandler = { [weak self] path in
            let isSatisfied = path.status == .satisfied
            // Delivered on the main queue (see `start`), which is the
            // main actor.
            MainActor.assumeIsolated {
                self?.networkChanged(isSatisfied: isSatisfied)
            }
        }
        pathMonitor.start(queue: .main)
    }

    var managementURL: URL? {
        controller.managementURL
    }

    func activate(key: String) {
        Task { await controller.activate(key: key) }
    }

    func verifyNow() {
        Task { await controller.verifyNow() }
    }

    func deactivate() {
        Task { await controller.deactivate() }
    }

    /// Re-checks when connectivity returns (not on the monitor's first
    /// report, which the launch check already covers). `refreshIfDue`
    /// only asks the server when a check is actually due.
    private func networkChanged(isSatisfied: Bool) {
        defer { isNetworkSatisfied = isSatisfied }
        guard isSatisfied, isNetworkSatisfied == false else {
            return
        }
        Task { await controller.refreshIfDue() }
    }

    // MARK: - This Mac

    private static var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unknown"
    }

    private static var macOSVersion: String {
        let version = ProcessInfo.processInfo.operatingSystemVersion
        return "\(version.majorVersion).\(version.minorVersion).\(version.patchVersion)"
    }

    private static func thisMac() -> DeviceDescriptor {
        DeviceDescriptor(
            id: hardwareID(),
            label: SCDynamicStoreCopyComputerName(nil, nil) as String? ?? "Mac",
            metadata: ["app_version": appVersion, "macos_version": macOSVersion]
        )
    }

    /// A salted hash of the hardware UUID: identifies this Mac to the
    /// local record without sending the UUID anywhere.
    ///
    /// When the UUID cannot be read, the ID is a random value minted
    /// for this launch — never a constant. A constant fallback would be
    /// shared by every Mac in that state, so a license record carried
    /// to another such Mac (Migration Assistant moves the Keychain)
    /// would be mistaken for that Mac's own, including by
    /// `LicenseController.deactivate`, which would then release the
    /// first Mac's activation on the server. A per-launch ID makes any
    /// stored record read as another Mac's — forgotten locally, never
    /// released. The price on a Mac whose UUID stays unreadable is
    /// real: `refreshIfDue` re-activates the stored key automatically
    /// at every launch, spending a fresh activation slot each time
    /// (orphaned slots are releasable only in the customer portal)
    /// until the key's activation limit refuses further launches, and
    /// no launch can release a previous launch's slot. That state
    /// means IOKit is not reporting a platform UUID at all — broken
    /// IOKit or a VM, not a healthy Mac — and failing closed there is
    /// the point. Persisting the random ID would defeat this:
    /// Migration Assistant would carry it along with the record it is
    /// meant to disown.
    private static func hardwareID() -> String {
        let platform = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("IOPlatformExpertDevice"))
        defer { IOObjectRelease(platform) }
        guard let uuid = IORegistryEntryCreateCFProperty(platform, "IOPlatformUUID" as CFString, kCFAllocatorDefault, 0)?
            .takeRetainedValue() as? String
        else {
            return UUID().uuidString
        }
        let digest = SHA256.hash(data: Data("com.lightsound.noican.license:\(uuid)".utf8))
        return digest.map { String(format: "%02x", $0) }.joined()
    }
}
