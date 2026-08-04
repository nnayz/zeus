// swift-tools-version: 6.0
import PackageDescription

// The engine behind `zeus`, the Rust + GPUI desktop app in `zeus/`. This
// package builds the daemon (`zeusd`), the PTY holder, and the `zeus`
// CLI; `zeus/scripts/package.sh` copies all three into `zeus.app`.
let package = Package(
    name: "zeus",
    platforms: [.macOS(.v15)],
    products: [
        .library(name: "ZeusProtocol", targets: ["ZeusProtocol"]),
        .library(name: "ZeusCore", targets: ["ZeusCore"]),
        .library(name: "ZeusClient", targets: ["ZeusClient"]),
        .library(name: "ZeusDetection", targets: ["ZeusDetection"]),
        .executable(name: "zeusd", targets: ["zeusd"]),
        .executable(name: "zeusd-holder", targets: ["zeusd-holder"]),
        .executable(name: "zeus", targets: ["zeus-cli"]),
    ],
    dependencies: [
        .package(url: "https://github.com/migueldeicaza/SwiftTerm", from: "1.13.0"),
        .package(url: "https://github.com/apple/swift-argument-parser", from: "1.5.0"),
    ],
    targets: [
        // MARK: Shared
        // Agent manifests live here (not in ZeusDetection) because every
        // layer needs the `agent` descriptor half — the CLI and the protocol
        // depend on ZeusCore but not on the detection engine. Detection
        // reads the `rules` half out of the same files.
        .target(name: "ZeusCore", resources: [.copy("Resources/manifests")]),
        .target(name: "ZeusProtocol", dependencies: ["ZeusCore"]),
        .target(name: "ZeusClient", dependencies: ["ZeusProtocol", "ZeusCore"]),
        .target(name: "ZeusDetection", dependencies: ["ZeusCore"]),

        // MARK: Daemon side
        .target(name: "CZeusPTY"),
        .target(name: "ZeusHolderKit", dependencies: ["CZeusPTY"]),
        .target(name: "ZeusGit", dependencies: ["ZeusCore"]),
        .target(name: "ZeusMCP", dependencies: ["ZeusProtocol", "ZeusCore"]),
        .target(
            name: "ZeusDaemonKit",
            dependencies: [
                "ZeusProtocol", "ZeusCore", "ZeusDetection", "ZeusGit",
                "CZeusPTY", "ZeusHolderKit",
                .product(name: "SwiftTerm", package: "SwiftTerm"),
            ],
            linkerSettings: [.linkedLibrary("sqlite3")]
        ),

        // MARK: Executables
        .executableTarget(name: "zeusd", dependencies: ["ZeusDaemonKit"]),
        .executableTarget(name: "zeusd-holder", dependencies: ["ZeusHolderKit"]),
        .executableTarget(
            name: "zeus-cli",
            dependencies: [
                "ZeusProtocol", "ZeusCore", "ZeusMCP",
                .product(name: "ArgumentParser", package: "swift-argument-parser"),
            ]
        ),

        // MARK: Tests
        .testTarget(name: "ZeusProtocolTests", dependencies: ["ZeusProtocol"]),
        .testTarget(name: "ZeusCoreTests", dependencies: ["ZeusCore"]),
        .testTarget(name: "ZeusDetectionTests", dependencies: ["ZeusDetection"]),
        .testTarget(
            name: "ZeusDaemonKitTests",
            dependencies: ["ZeusDaemonKit", "ZeusHolderKit", "ZeusClient", "zeusd-holder"]
        ),
        .testTarget(
            name: "ZeusCLITests",
            dependencies: ["zeus-cli", "ZeusCore", "ZeusProtocol"]
        ),
    ]
)
