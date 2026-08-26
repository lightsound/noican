// swift-tools-version: 6.1

import PackageDescription

// Deliberately a standalone package (not a target of the app package):
// it depends on nothing but the standard library, so `swift test` here
// never needs the Rust staticlib the app's executable target links, and
// the reducer tests run on any platform — including the Linux CI runners.
let package = Package(
    name: "NoicanState",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .library(name: "NoicanState", targets: ["NoicanState"]),
    ],
    targets: [
        .target(
            name: "NoicanState",
            path: "Sources/NoicanState"
        ),
        .testTarget(
            name: "NoicanStateTests",
            dependencies: ["NoicanState"],
            path: "Tests/NoicanStateTests"
        ),
    ]
)
