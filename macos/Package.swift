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
        // The pure state machine (reducer + projections). A standalone
        // package so its tests run without the Rust staticlib this
        // executable links; see macos/NoicanState/Package.swift.
        .package(path: "NoicanState"),
        // License activation against Polar (or a replacement backend);
        // standalone for the same reason as NoicanState.
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
                // scripts/build-macos-app.sh reads this "-L" line back to
                // decide where cargo must put libnoican_ffi.a, and fails
                // the build if the path does not match its TARGET. Keep
                // the flag and its path on one line in this shape.
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
