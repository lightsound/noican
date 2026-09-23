import Foundation
import NoicanLicensing

/// The license server this build talks to. Compiled in rather than read
/// from Info.plist: an unconfigured build runs unrestricted, and a plist
/// edit must not be able to turn a shipping build into one.
///
/// Placeholders until the Polar organization exists — see
/// docs/licensing.md ("Polar setup") for where each value comes from.
/// While `organizationID` is not a UUID the app reports "License not
/// configured" and noise cancellation is not gated.
enum LicenseConfiguration {
    static let polar = PolarConfiguration(
        // Switch to .production for release builds; sandbox keys do not
        // exist on production and vice versa.
        server: .sandbox,
        organizationID: "",
        benefitID: "",
        organizationSlug: ""
    )

    /// Checkout or product page for the "Buy Noican" link; nil hides it.
    static let purchaseURL: URL? = nil
}
