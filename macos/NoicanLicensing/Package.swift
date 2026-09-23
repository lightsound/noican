// swift-tools-version: 6.1

import PackageDescription

// Standalone like NoicanState: the license logic depends on Foundation
// only (the Keychain store compiles where Security exists), so its tests
// run without the Rust staticlib and without a signed app bundle.
let package = Package(
    name: "NoicanLicensing",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .library(name: "NoicanLicensing", targets: ["NoicanLicensing"]),
    ],
    targets: [
        .target(
            name: "NoicanLicensing",
            path: "Sources/NoicanLicensing"
        ),
        .testTarget(
            name: "NoicanLicensingTests",
            dependencies: ["NoicanLicensing"],
            path: "Tests/NoicanLicensingTests"
        ),
    ]
)
