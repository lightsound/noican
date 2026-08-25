// swift-tools-version: 5.9
import PackageDescription

// The Rust engine is linked as a static library. `build.sh` builds it with
// cargo and passes the search path in; building this package directly without
// that step will fail at the link stage, which is the intended behaviour —
// there is no useful app without the engine.
let rustLibraryPath = Context.environment["NOICAN_RUST_LIB_DIR"] ?? "../../target/release"

let package = Package(
    name: "NoicanMenuBar",
    platforms: [
        // Core Audio process taps and the current aggregate-device behaviour
        // both assume a recent system; 14 is the floor the research settled on.
        .macOS(.v14)
    ],
    targets: [
        .systemLibrary(
            name: "CNoican",
            path: "Sources/CNoican"
        ),
        .executableTarget(
            name: "NoicanMenuBar",
            dependencies: ["CNoican"],
            path: "Sources/NoicanMenuBar",
            linkerSettings: [
                .unsafeFlags([
                    "-L\(rustLibraryPath)",
                    "-lnoican_ffi",
                ]),
                .linkedFramework("CoreAudio"),
                .linkedFramework("AudioToolbox"),
                .linkedFramework("CoreFoundation"),
                // ONNX Runtime, which the Rust engine statically links, is C++.
                .linkedLibrary("c++"),
            ]
        ),
    ]
)
