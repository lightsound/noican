// swift-tools-version: 6.1

import PackageDescription

let package = Package(
    name: "NoicanMenuBar",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .executable(name: "NoicanMenuBar", targets: ["NoicanMenuBar"]),
    ],
    dependencies: [
        .package(path: "NoicanState"),
        .package(path: "NoicanLicensing"),
    ],
    targets: [
        .target(
            name: "CNoican",
            path: "Sources/CNoican",
            publicHeadersPath: "include"
        ),
        .executableTarget(
            name: "NoicanMenuBar",
            dependencies: [
                "CNoican",
                .product(name: "NoicanState", package: "NoicanState"),
                .product(name: "NoicanLicensing", package: "NoicanLicensing"),
            ],
            path: "Sources/NoicanMenuBar",
            linkerSettings: [
                .unsafeFlags([
                    "-L", "../target/aarch64-apple-darwin/release",
                ]),
                .linkedLibrary("noican_ffi"),
                .linkedLibrary("c++"),
                .linkedFramework("Accelerate"),
                .linkedFramework("AudioToolbox"),
                .linkedFramework("AudioUnit"),
                .linkedFramework("CoreAudio"),
                .linkedFramework("CoreFoundation"),
                .linkedFramework("Foundation"),
                .linkedFramework("Security"),
                .linkedFramework("SystemConfiguration"),
            ]
        ),
    ]
)
