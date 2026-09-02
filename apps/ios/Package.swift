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
                "Location/LocationKeychainCredentialStore.swift",
                "Location/LocationReportEnricher.swift",
                "Location/LocationReporter.swift",
                "Location/LocationSettingsView.swift",
            ],
            sources: [
                "API",
                "Domain",
                "Location/LocationCredentialCapabilities.swift",
                "Location/LocationDiskQueue.swift",
                "Location/LocationPresence.swift",
                "Location/LocationReport.swift",
                "Location/LocationStatusStore.swift",
            ]
        ),
        .testTarget(
            name: "BrunnCoreTests",
            dependencies: ["BrunnCore"],
            path: "BrunnCoreTests"
        ),
    ]
)
