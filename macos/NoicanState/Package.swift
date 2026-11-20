// swift-tools-version: 6.1

import PackageDescription

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
