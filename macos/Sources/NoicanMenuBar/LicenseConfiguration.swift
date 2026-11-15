import Foundation
import NoicanLicensing

enum LicenseConfiguration {
    static let polar = PolarConfiguration(
        server: .sandbox,
        organizationID: "",
        benefitID: "",
        organizationSlug: ""
    )

    static let purchaseURL: URL? = nil
}
