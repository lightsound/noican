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
    targets: [
        .target(
            name: "CNoican",
            path: "Sources/CNoican",
            publicHeadersPath: "include"
        ),
        .executableTarget(
            name: "NoicanMenuBar",
            dependencies: ["CNoican"],
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
