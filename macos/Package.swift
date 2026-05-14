// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "NitpickAgentMacOS",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .library(name: "NitpickAgentMacOSCore", targets: ["NitpickAgentMacOSCore"]),
        .executable(name: "NitpickAgentApp", targets: ["NitpickAgentApp"]),
    ],
    dependencies: [
        .package(url: "https://github.com/sparkle-project/Sparkle", from: "2.9.1")
    ],
    targets: [
        .target(name: "NitpickAgentMacOSCore"),
        .executableTarget(
            name: "NitpickAgentApp",
            dependencies: [
                "NitpickAgentMacOSCore",
                .product(name: "Sparkle", package: "Sparkle"),
            ]
        ),
        .testTarget(
            name: "NitpickAgentMacOSCoreTests",
            dependencies: ["NitpickAgentMacOSCore"]
        ),
        .testTarget(
            name: "NitpickAgentAppTests",
            dependencies: ["NitpickAgentApp"]
        ),
    ]
)
