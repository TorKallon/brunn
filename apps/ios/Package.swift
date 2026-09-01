// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "BrunnCore",
    platforms: [
        .macOS(.v14),
        .iOS(.v17),
    ],
    products: [
        .library(name: "BrunnCore", targets: ["BrunnCore"]),
    ],
    targets: [
        .target(
            name: "BrunnCore",
            path: "Brunn",
            exclude: [
                "App",
                "Features",
                "Resources",
                "Services",
                "Shared",
            ],
            sources: ["API", "Domain"]
        ),
        .testTarget(
            name: "BrunnCoreTests",
            dependencies: ["BrunnCore"],
            path: "BrunnCoreTests"
        ),
    ]
)
