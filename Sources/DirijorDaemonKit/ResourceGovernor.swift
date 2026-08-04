import Darwin
import DirijorCore
import DirijorProtocol
import Foundation

/// Governor policy knobs. Static defaults for now; real Settings wiring later.
public struct GovernorConfig: Sendable {
    /// Sustained-idle time before an unattached idle session is frozen. 0 = never.
    public var idleThresholdSeconds: TimeInterval = 900
    /// Above this footprint the record just carries the number (UI badges it).
    public var softMemoryBytes: UInt64 = 2 << 30
    /// Above this footprint the session is frozen (never silently killed).
    public var hardMemoryBytes: UInt64 = 6 << 30
    /// All sessions together may use this fraction of physical RAM before the
    /// governor starts hibernating idle sessions oldest-first. Deliberately
    /// generous: this machine's whole job is running agents, and — because
    /// hibernation is SIGSTOP, which doesn't return pages — freezing under
    /// budget reclaims little, so a low fraction just churns idle tabs.
    public var globalBudgetFraction: Double = 0.75
    /// Minimum idle stretch before the *budget* path may freeze a session. The
    /// sustained-idle path has its own (longer) threshold; this stops the budget
    /// sweep from freezing a tab you glanced away from seconds ago while the
    /// machine sits chronically over a tight budget.
    public var budgetMinIdleSeconds: TimeInterval = 300
    /// Main scan cadence.
    public var scanInterval: Duration = .seconds(30)
    /// Hibernated sessions get a cheap footprint sample every Nth tick only.
    public var hibernatedSampleEvery: Int = 5
    /// Port scanning (lsof shell-out) can be disabled wholesale.
    public var portScanEnabled: Bool = true

    public init() {}
}

/// Periodic resource sweep over live sessions: memory footprint, listening
/// ports, artifact pull — plus the idle-hibernation and RAM-watchdog policies.
/// Bound in Daemon.start(); everything here runs at 30s cadence, never hot.
public actor ResourceGovernor {
    private let registry: SessionRegistry
    private var config: GovernorConfig
    private var loopTask: Task<Void, Never>?
    private var tickCount = 0
    private let physicalMemory: UInt64

    public init(registry: SessionRegistry, config: GovernorConfig = GovernorConfig()) {
        self.registry = registry
        self.config = config
        self.physicalMemory = ProcessInfo.processInfo.physicalMemory
    }

    public func start() {
        guard loopTask == nil else { return }
        loopTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                try? await Task.sleep(for: self.config.scanInterval)
                guard !Task.isCancelled else { return }
                await self.scan()
            }
        }
        DaemonLog.shared.log("resource governor started (scan \(config.scanInterval))")
    }

    public func stop() {
        loopTask?.cancel()
        loopTask = nil
    }

    public func configure(_ settings: GovernorSettingsParams) {
        config.idleThresholdSeconds = settings.idleThresholdSeconds
        config.hardMemoryBytes = settings.hardMemoryBytes
        DaemonLog.shared.log(
            "resource policy updated (idle=\(settings.idleThresholdSeconds)s hard=\(settings.hardMemoryBytes))")
    }

    func currentConfig() -> GovernorConfig { config }

    /// Port badges only matter for a terminal the user can currently see.
    /// `lsof` is a subprocess and process-tree walk, so run it every fourth
    /// governor pass (two minutes at the default cadence), never for detached
    /// background sessions.
    static func shouldScanPorts(enabled: Bool, attached: Bool, tick: Int) -> Bool {
        enabled && attached && tick.isMultiple(of: 4)
    }

    // MARK: Scan

    /// One full sweep. Internal so tests can drive it without the timer.
    func scan() async {
        tickCount += 1
        let sessions = await registry.liveSessionsSnapshot()
        var totalFootprint: UInt64 = 0
        var idleCandidates: [(id: SessionID, idleSince: Date, footprint: UInt64)] = []

        for (id, session) in sessions {
            guard await session.isRunning else { continue }
            guard let record = await registry.record(id) else { continue }

            if await session.isHibernated {
                // Frozen trees barely change; sample footprint occasionally so
                // the badge stays honest, skip everything else.
                if tickCount % config.hibernatedSampleEvery == 0,
                    let pids = record.hibernation?.treePids
                {
                    let footprint = ProcessTree.footprint(pids.map { pid_t($0) })
                    totalFootprint &+= footprint
                    await registry.applyResourceSample(
                        sessionID: id, memoryBytes: footprint, ports: nil, artifacts: nil)
                } else {
                    totalFootprint &+= record.memoryBytes ?? 0
                }
                continue
            }

            let rootPid = await session.pid
            guard rootPid > 0 else { continue }
            let tree = ProcessTree.enumerate(root: rootPid)
            let pids = tree.map(\.pid)
            let footprint = ProcessTree.footprint(pids)
            totalFootprint &+= footprint

            let attached = await session.sinkCount > 0
            let ports =
                Self.shouldScanPorts(
                    enabled: config.portScanEnabled, attached: attached, tick: tickCount)
                ? Self.listeningPorts(of: pids) : nil
            let artifacts = await session.artifacts

            await registry.applyResourceSample(
                sessionID: id,
                memoryBytes: footprint,
                ports: ports,
                artifacts: artifacts.isEmpty ? nil : artifacts
            )

            // Eligibility for ANY auto-hibernation: idle and unattended. A
            // working / needs-input session, or one a client is viewing, is
            // never frozen out from under the user — computed once and reused
            // by every policy below.
            let idle = await idleSince(record: record, session: session)

            // RAM watchdog, hard limit: reclaim a runaway — but only an idle,
            // unattended one. A heavy agent (large context + build/browser
            // subprocesses) legitimately crosses this mid-task; freezing it
            // there was the "keeps hibernating while working" bug. An active
            // session over the limit stays up and is badged by the soft-limit
            // path instead; the user can hibernate it by hand.
            if footprint > config.hardMemoryBytes, idle != nil {
                DaemonLog.shared.log(
                    "session \(id) over hard memory limit (\(footprint) bytes) — freezing")
                try? await registry.hibernate(sessionID: id, reason: .memoryPressure)
                continue
            }

            // Idle-hibernation policy.
            if let idleSince = idle {
                idleCandidates.append((id, idleSince, footprint))
            }
        }

        // Sustained-idle freeze.
        if config.idleThresholdSeconds > 0 {
            let now = Date()
            for candidate in idleCandidates
            where now.timeIntervalSince(candidate.idleSince) > config.idleThresholdSeconds {
                DaemonLog.shared.log(
                    "session \(candidate.id) idle since \(candidate.idleSince) — hibernating")
                try? await registry.hibernate(sessionID: candidate.id, reason: .idle)
            }
        }

        // Global budget: over → freeze idle sessions oldest-first until under.
        let budget = UInt64(Double(physicalMemory) * config.globalBudgetFraction)
        if totalFootprint > budget {
            DaemonLog.shared.log(
                "sessions over global budget (\(totalFootprint)/\(budget) bytes)")
            var excess = totalFootprint - budget
            let now = Date()
            for candidate in idleCandidates.sorted(by: { $0.idleSince < $1.idleSince })
            where now.timeIntervalSince(candidate.idleSince) > config.budgetMinIdleSeconds {
                guard excess > 0 else { break }
                DaemonLog.shared.log("budget: hibernating idle session \(candidate.id)")
                try? await registry.hibernate(sessionID: candidate.id, reason: .memoryPressure)
                excess = excess > candidate.footprint ? excess - candidate.footprint : 0
            }
        }
    }

    /// Non-nil when the session is eligible for idle hibernation; the date is
    /// the start of its idle stretch (last meaningful activity).
    private func idleSince(record: SessionRecord, session: AgentSession) async -> Date? {
        guard record.hibernation == nil, !record.pinned else { return nil }
        // Only committed-idle sessions. needsInput / working / starting never
        // hibernate; shells reach .idle via the processOnly reducer when quiet.
        guard case .idle = record.status else { return nil }
        guard await session.sinkCount == 0 else { return nil }
        let lastDetachAt = await session.lastDetachAt
        let recency = [
            record.lastTurnCompletedAt, record.updatedAt, lastDetachAt, record.lastSeenAt,
        ]
        .compactMap { $0 }
        .max()
        return recency ?? record.createdAt
    }

    // MARK: Ports

    /// `lsof -a -iTCP -sTCP:LISTEN -p <pids> -Fpcn` over the tree — simple and
    /// off the hot path. -F machine format: `p<pid>` `c<command>` `n<host:port>`.
    /// Bounded by a watchdog so a wedged lsof can't stall the scan.
    static func listeningPorts(of pids: [pid_t], timeout: TimeInterval = 3.0) -> [PortInfo]? {
        guard !pids.isEmpty else { return [] }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/sbin/lsof")
        process.arguments = [
            "-a", "-iTCP", "-sTCP:LISTEN",
            "-p", pids.map(String.init).joined(separator: ","),
            "-Fpcn", "-n", "-P",
        ]
        let stdout = Pipe()
        process.standardOutput = stdout
        process.standardError = Pipe()

        do {
            try process.run()
        } catch {
            return nil
        }
        let watchdog = DispatchWorkItem { [weak process] in
            if process?.isRunning == true { process?.terminate() }
        }
        DispatchQueue.global().asyncAfter(deadline: .now() + timeout, execute: watchdog)
        let data = stdout.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        watchdog.cancel()

        // lsof exits 1 when nothing matches; only treat launch/kill as failure.
        guard process.terminationReason == .exit else { return nil }
        return parseLsofOutput(String(decoding: data, as: UTF8.self))
    }

    /// Parses `-Fpcn` output into unique ports. `n` lines look like
    /// `*:3000`, `127.0.0.1:8123`, or `[::1]:5173`.
    static func parseLsofOutput(_ output: String) -> [PortInfo] {
        var results: [PortInfo] = []
        var seenPorts: Set<Int> = []
        var currentCommand = "?"

        for line in output.split(separator: "\n") {
            guard let field = line.first else { continue }
            let value = String(line.dropFirst())
            switch field {
            case "c":
                currentCommand = value
            case "n":
                guard let colon = value.lastIndex(of: ":"),
                    let port = Int(value[value.index(after: colon)...]),
                    port > 0, !seenPorts.contains(port)
                else { continue }
                seenPorts.insert(port)
                results.append(PortInfo(port: port, processName: currentCommand))
            default:
                break
            }
        }
        return results.sorted { $0.port < $1.port }
    }
}
