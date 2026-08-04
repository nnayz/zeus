// swift-tools-version: 6.0
import PackageDescription

// The engine behind `diri`, the Rust + GPUI desktop app in `diri/`. This
// package builds the daemon (`dirijord`), the PTY holder, and the `dirijor`
// CLI; `diri/scripts/package.sh` copies all three into `diri.app`.
let package = Package(
    name: "dirijor",
    platforms: [.macOS(.v15)],
    products: [
        .library(name: "DirijorProtocol", targets: ["DirijorProtocol"]),
        .library(name: "DirijorCore", targets: ["DirijorCore"]),
        .library(name: "DirijorClient", targets: ["DirijorClient"]),
        .library(name: "DirijorDetection", targets: ["DirijorDetection"]),
        .executable(name: "dirijord", targets: ["dirijord"]),
        .executable(name: "dirijord-holder", targets: ["dirijord-holder"]),
        .executable(name: "dirijor", targets: ["dirijor-cli"]),
    ],
    dependencies: [
        .package(url: "https://github.com/migueldeicaza/SwiftTerm", from: "1.13.0"),
        .package(url: "https://github.com/apple/swift-argument-parser", from: "1.5.0"),
    ],
    targets: [
        // MARK: Shared
        // Agent manifests live here (not in DirijorDetection) because every
        // layer needs the `agent` descriptor half — the CLI and the protocol
        // depend on DirijorCore but not on the detection engine. Detection
        // reads the `rules` half out of the same files.
        .target(name: "DirijorCore", resources: [.copy("Resources/manifests")]),
        .target(name: "DirijorProtocol", dependencies: ["DirijorCore"]),
        .target(name: "DirijorClient", dependencies: ["DirijorProtocol", "DirijorCore"]),
        .target(name: "DirijorDetection", dependencies: ["DirijorCore"]),

        // MARK: Daemon side
        .target(name: "CDirijorPTY"),
        .target(name: "DirijorHolderKit", dependencies: ["CDirijorPTY"]),
        .target(name: "DirijorGit", dependencies: ["DirijorCore"]),
        .target(name: "DirijorMCP", dependencies: ["DirijorProtocol", "DirijorCore"]),
        .target(
            name: "DirijorDaemonKit",
            dependencies: [
                "DirijorProtocol", "DirijorCore", "DirijorDetection", "DirijorGit",
                "CDirijorPTY", "DirijorHolderKit",
                .product(name: "SwiftTerm", package: "SwiftTerm"),
            ],
            linkerSettings: [.linkedLibrary("sqlite3")]
        ),

        // MARK: Executables
        .executableTarget(name: "dirijord", dependencies: ["DirijorDaemonKit"]),
        .executableTarget(name: "dirijord-holder", dependencies: ["DirijorHolderKit"]),
        .executableTarget(
            name: "dirijor-cli",
            dependencies: [
                "DirijorProtocol", "DirijorCore", "DirijorMCP",
                .product(name: "ArgumentParser", package: "swift-argument-parser"),
            ]
        ),

        // MARK: Tests
        .testTarget(name: "DirijorProtocolTests", dependencies: ["DirijorProtocol"]),
        .testTarget(name: "DirijorCoreTests", dependencies: ["DirijorCore"]),
        .testTarget(name: "DirijorDetectionTests", dependencies: ["DirijorDetection"]),
        .testTarget(
            name: "DirijorDaemonKitTests",
            dependencies: ["DirijorDaemonKit", "DirijorHolderKit", "DirijorClient", "dirijord-holder"]
        ),
        .testTarget(
            name: "DirijorCLITests",
            dependencies: ["dirijor-cli", "DirijorCore", "DirijorProtocol"]
        ),
    ]
)
