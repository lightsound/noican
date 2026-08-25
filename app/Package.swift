// swift-tools-version:5.9
// SwiftUI menu-bar app for the noican engine.
//
// Build order matters: the Rust FFI static library must exist first.
// Use scripts/build-macos-app.sh from the repository root, which builds
// the Rust workspace, then this package, then assembles Noican.app.
import PackageDescription

let package = Package(
    name: "NoicanApp",
    platforms: [.macOS(.v14)],
    targets: [
        .target(name: "CNoican"),
        .executableTarget(
            name: "NoicanApp",
            dependencies: ["CNoican"],
            linkerSettings: [
                .unsafeFlags(["-L../target/release"]),
                .linkedLibrary("noican_ffi"),
                .linkedFramework("CoreAudio"),
                .linkedFramework("CoreFoundation"),
            ]
        ),
    ]
)
