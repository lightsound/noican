// swift-tools-version: 6.1

import PackageDescription

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
