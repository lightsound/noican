import CryptoKit
import Foundation
import IOKit
import NoicanLicensing
import SystemConfiguration

/// The app's license shell: builds the `LicenseController` from
/// `LicenseConfiguration`, publishes its state to the menu, forwards
/// whether noise cancellation may start to the engine reducer, and
/// re-checks the license hourly (the continuous clock keeps counting
/// through sleep, so the first tick after a long sleep comes right away).
@MainActor
final class LicenseModel: ObservableObject {
    @Published private(set) var state: LicenseState

    /// Whether this build talks to Polar's sandbox (shown next to the
    /// status so a sandbox build is never mistaken for a release).
    let isSandbox: Bool
    let purchaseURL = LicenseConfiguration.purchaseURL

    private let controller: LicenseController
    private var refreshTask: Task<Void, Never>?

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
        refreshTask = Task { [weak self] in
            await self?.controller.refreshIfDue()
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(60 * 60))
                await self?.controller.refreshIfDue()
            }
        }
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
    private static func hardwareID() -> String {
        let platform = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("IOPlatformExpertDevice"))
        defer { IOObjectRelease(platform) }
        let uuid = IORegistryEntryCreateCFProperty(platform, "IOPlatformUUID" as CFString, kCFAllocatorDefault, 0)?
            .takeRetainedValue() as? String ?? "unknown"
        let digest = SHA256.hash(data: Data("com.lightsound.noican.license:\(uuid)".utf8))
        return digest.map { String(format: "%02x", $0) }.joined()
    }
}
