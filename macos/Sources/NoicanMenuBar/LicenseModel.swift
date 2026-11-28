import CryptoKit
import Foundation
import IOKit
import Network
import NoicanLicensing
import SystemConfiguration

@MainActor
final class LicenseModel: ObservableObject {
    @Published private(set) var state: LicenseState

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

    private func networkChanged(isSatisfied: Bool) {
        defer { isNetworkSatisfied = isSatisfied }
        guard isSatisfied, isNetworkSatisfied == false else {
            return
        }
        Task { await controller.refreshIfDue() }
    }

    private static var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unknown"
    }

    private static var macOSVersion: String {
        let version = ProcessInfo.processInfo.operatingSystemVersion
        return "\(version.majorVersion).\(version.minorVersion).\(version.patchVersion)"
    }

    private static func thisMac() -> DeviceDescriptor? {
        guard let id = hardwareID() else {
            return nil
        }
        return DeviceDescriptor(
            id: id,
            label: SCDynamicStoreCopyComputerName(nil, nil) as String? ?? "Mac",
            metadata: ["app_version": appVersion, "macos_version": macOSVersion]
        )
    }

    private static func hardwareID() -> String? {
        let platform = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("IOPlatformExpertDevice"))
        defer { IOObjectRelease(platform) }
        guard let uuid = IORegistryEntryCreateCFProperty(platform, "IOPlatformUUID" as CFString, kCFAllocatorDefault, 0)?
            .takeRetainedValue() as? String
        else {
            return nil
        }
        let digest = SHA256.hash(data: Data("com.lightsound.noican.license:\(uuid)".utf8))
        return digest.map { String(format: "%02x", $0) }.joined()
    }
}
