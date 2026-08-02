// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "StraylightCore",
    platforms: [
        .macOS(.v14),
        .iOS(.v17),
    ],
    products: [
        .library(name: "StraylightCore", targets: ["StraylightCore"]),
    ],
    targets: [
        .target(
            name: "StraylightCore",
            path: "Straylight",
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
            name: "StraylightCoreTests",
            dependencies: ["StraylightCore"],
            path: "StraylightCoreTests"
        ),
    ]
)
