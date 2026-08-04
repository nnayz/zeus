import CryptoKit
import DirijorCore
import DirijorProtocol
import Foundation

/// Assembles and runs the whole daemon. `dirijord`'s main is a thin shell
/// around this so tests can drive a Daemon in-process against a temp config.
public final class Daemon: Sendable {
    public let registry: SessionRegistry
    public let events: EventBus
    let statusEngine: StatusEngine
    let hub: ConnectionHub
    public let governor: ResourceGovernor
    public let prMonitor: PullRequestMonitor
    public let browserPool: BrowserPool
    let titleWatcher: TitleWatcher
    let shutdownRequest = ShutdownRequestBox()
    public static let build = "0.1.0"

    public init(config: DaemonConfig) {
        let events = EventBus()
        let registry = SessionRegistry(config: config, events: events)
        let statusEngine = StatusEngine()
        let browserPool = BrowserPool(config: config)
        let governor = ResourceGovernor(registry: registry)
        self.events = events
        self.registry = registry
        self.statusEngine = statusEngine
        self.governor = governor
        self.prMonitor = PullRequestMonitor(registry: registry)
        self.browserPool = browserPool
        self.titleWatcher = TitleWatcher(registry: registry)
        self.hub = ConnectionHub(
            socketPath: config.socketPath,
            services: DaemonServices(
                registry: registry, events: events, statusEngine: statusEngine,
                browserPool: browserPool, governor: governor, build: Self.build,
                executableHash: Self.executableHash(),
                shutdownRequest: shutdownRequest,
                remote: config.remote
            )
        )
    }

    public func start() async throws {
        // Step-by-step boot logging so a wedged/crashing start is attributable
        // from the (truncated-per-launch) daemon log: the last "start: …" line
        // before silence names the culprit. See `dirijord … ready` in main.swift
        // for the terminal success marker — if that never appears but the CLI
        // helper line does, one of these steps hung or hard-crashed.
        DaemonLog.shared.log("start: begin")
        InjectionBuilder.scrubProcessEnvironment()
        // daemon.shutdown → persist state and exit. The app uses this to swap
        // in a newer daemon binary (hello's executableHash differs from the
        // bundle's): holder-owned sessions stay live and the replacement daemon
        // adopts them from disk. The brief delay lets the RPC ack flush first.
        shutdownRequest.bind { [weak self] in
            Task {
                try? await Task.sleep(for: .milliseconds(150))
                await self?.shutdown()
                DaemonLog.shared.log("exiting on daemon.shutdown request")
                // This RPC always means "replace me" (app update or settings
                // requiring a restart). A non-zero exit asks launchd's
                // SuccessfulExit=false policy to relaunch the embedded agent.
                exit(1)
            }
        }
        await statusEngine.bind(registry: registry)
        DaemonLog.shared.log("start: statusEngine bound")
        await registry.bind(statusEngine: statusEngine)
        DaemonLog.shared.log("start: registry bound")
        await registry.restoreFromDisk()
        DaemonLog.shared.log("start: restoreFromDisk done")
        try await hub.start()
        DaemonLog.shared.log("start: hub listening")
        await governor.start()
        DaemonLog.shared.log("start: governor started")
        await prMonitor.start()
        DaemonLog.shared.log("start: prMonitor started")
        await browserPool.start()
        DaemonLog.shared.log("start: browserPool started")
        await titleWatcher.start()
        DaemonLog.shared.log("start: titleWatcher started")
        await registry.startBranchMonitor()
        DaemonLog.shared.log("start: branchMonitor started")
    }

    public func shutdown() async {
        await registry.stopBranchMonitor()
        await titleWatcher.stop()
        await browserPool.stop()
        await prMonitor.stop()
        await governor.stop()
        await registry.prepareShutdown()
        await hub.stop()
    }

    /// SHA-256 of our own executable — the daemon's true version identity.
    /// Build strings go stale; the binary hash can't.
    static func executableHash() -> String? {
        let path = URL(fileURLWithPath: CommandLine.arguments[0])
            .resolvingSymlinksInPath().path
        guard let data = FileManager.default.contents(atPath: path) else { return nil }
        return SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }
}
