import DirijorCore
import DirijorDetection
import DirijorProtocol
import Foundation

/// Drives one StatusReducer per session: 200ms screen scans (skipped when
/// content hasn't changed) + hook/notify ingestion. Outcomes flow into the
/// SessionRegistry, which owns records and event publishing.
actor StatusEngine {
    private struct Tracked {
        var reducer: StatusReducer
        var session: AgentSession
        var lastScannedSeq: UInt64 = 0
        /// Last time we ran the expensive snapshot + manifest evaluation.
        var lastEval: Date = .distantPast
        /// Last time this session's output activity reached the event bus.
        var lastOutputEvent: Date = .distantPast
        /// What the session was spawned as (never changes).
        var baseKind: AgentKind = .shell
        /// Adopted foreground agent when a shell tab is running `claude`/`codex`.
        var foregroundAgent: AgentKind?
        var effectiveKind: AgentKind { foregroundAgent ?? baseKind }
    }

    private var tracked: [SessionID: Tracked] = [:]
    private let manifests: ManifestEngine?
    private var registry: SessionRegistry?
    private var tickTask: Task<Void, Never>?
    /// Foreground-agent adoption runs every Nth tick (~2s at either cadence).
    private var fgTickCounter = 0
    /// Unattached sessions need status notifications, not foreground animation
    /// latency. This counter lets them share the loop at a bounded cadence.
    private var scanTickCounter = 0
    /// Number of clients that have declared themselves frontmost. While zero,
    /// nobody is watching the UI, so the scan tick backs off from 200ms to 1s.
    /// Terminal grid delivery is independent and remains 20fps; status regexes
    /// were already throttled to 5Hz, so a 10Hz coordinator was pure wakeup
    /// overhead. One-second background detection is within notification latency.
    private var activeClientCount = 0
    private var appActive: Bool { activeClientCount > 0 }
    static let activeTickInterval: Duration = .milliseconds(200)
    static let backgroundTickInterval: Duration = .seconds(1)
    private var tickInterval: Duration {
        appActive ? Self.activeTickInterval : Self.backgroundTickInterval
    }
    /// Minimum spacing between `session.output` events for one session.
    static let outputEventInterval: TimeInterval = 1.0

    /// A visible terminal keeps the responsive cadence. Unattached sessions
    /// are sampled at 600ms while the desktop is active and 1s while it is not,
    /// instead of multiplying foreground cost by every restored tab.
    static func shouldInspectSession(
        attached: Bool, appActive: Bool, tick: Int
    ) -> Bool {
        attached || tick.isMultiple(of: appActive ? 3 : 1)
    }

    init() {
        self.manifests = try? ManifestEngine(overridesDir: DirijorPaths.manifestOverridesDir)
    }

    func bind(registry: SessionRegistry) {
        self.registry = registry
    }

    func track(sessionID: SessionID, kind: AgentKind, session: AgentSession) {
        tracked[sessionID] = Tracked(
            reducer: StatusReducer(authority: Self.authority(for: kind, adopted: false)),
            session: session,
            baseKind: kind)
        startTickingIfNeeded()
    }

    /// Reducer authority for a kind, declared by its manifest. An ADOPTED agent
    /// (typed into a shell tab) never went through `InjectionBuilder`, so it has
    /// no hook wiring — a `hooks` manifest degrades to screen-primary rather
    /// than waiting forever for events that will never arrive.
    private static func authority(for kind: AgentKind, adopted: Bool) -> StatusReducer.Authority {
        switch kind.descriptor.statusAuthority {
        case .hooks: return adopted ? .screenPrimary : .hooksPrimary
        case .screen: return .screenPrimary
        case .process: return .processOnly
        }
    }

    func untrack(sessionID: SessionID) {
        tracked.removeValue(forKey: sessionID)
        if tracked.isEmpty {
            tickTask?.cancel()
            tickTask = nil
        }
    }

    // MARK: Signals

    func ingest(sessionID: SessionID, signal: StatusSignal) async {
        guard tracked[sessionID] != nil else { return }
        let outcome = tracked[sessionID]!.reducer.reduce(signal)
        await apply(outcome, currentStatus: tracked[sessionID]!.reducer.status, to: sessionID)
    }

    func ingestHookReport(_ params: HookReportParams) async {
        guard let sessionID = params.dirijorSessionID else { return }
        let parsed: (StatusSignal, HookMetadata)?
        switch params.kind {
        case "claude-hook":
            guard let event = params.event else { return }
            parsed = HookParsing.parseClaudeHook(event: event, payload: params.payload)
        case "codex-notify":
            parsed = HookParsing.parseCodexNotify(payload: params.payload)
        default:
            parsed = nil
        }
        guard let (signal, meta) = parsed else { return }
        await registry?.applyHookMetadata(sessionID: sessionID, meta: meta)
        await ingest(sessionID: sessionID, signal: signal)
    }

    func processExited(sessionID: SessionID, info: ExitInfo) async {
        await ingest(sessionID: sessionID, signal: .processExit(code: info.code, signal: info.signal))
        untrack(sessionID: sessionID)
    }

    // MARK: Tick loop

    private func startTickingIfNeeded() {
        guard tickTask == nil else { return }
        tickTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let interval = await self?.tickInterval else { break }
                try? await Task.sleep(for: interval)
                await self?.tickAll()
            }
        }
    }

    /// The desktop app reports frontmost/backgrounded transitions. Reference-
    /// counted so a stale "inactive" from one client can't slow ticks while
    /// another client is watching; a dropped connection releases its count via
    /// `clientResignedActive` on close.
    func clientBecameActive() {
        activeClientCount += 1
    }

    func clientResignedActive() {
        activeClientCount = max(0, activeClientCount - 1)
    }

    private func tickAll() async {
        fgTickCounter += 1
        scanTickCounter &+= 1
        let active = appActive
        let checkForeground = fgTickCounter.isMultiple(of: active ? 10 : 2)  // ~every 2s

        for (id, entry) in tracked {
            guard tracked[id] != nil else { continue }
            // A hibernated tree is SIGSTOPped: the screen cannot change and the
            // status must stay exactly as frozen (wake restores it untouched).
            if await entry.session.isHibernated { continue }
            let attached = await entry.session.sinkCount > 0
            guard Self.shouldInspectSession(
                attached: attached, appActive: active, tick: scanTickCounter)
            else { continue }

            // Foreground-agent adoption: a shell tab that starts `claude` (or
            // `codex`) becomes that agent — display, manifest, and reducer
            // authority follow; when the agent exits it reverts to a shell.
            if checkForeground, !entry.baseKind.isFirstClass {
                let fg = await entry.session.foregroundAgentKind()
                if fg != entry.foregroundAgent, tracked[id] != nil {
                    tracked[id]!.foregroundAgent = fg
                    let effective = tracked[id]!.effectiveKind
                    tracked[id]!.reducer = StatusReducer(
                        authority: Self.authority(for: effective, adopted: fg != nil))
                    tracked[id]!.lastScannedSeq = 0  // rescan under the new manifest
                    await registry?.applyForegroundAgent(sessionID: id, agent: fg)
                }
            }
            let seq = await entry.session.contentSeq
            if seq != entry.lastScannedSeq {
                // Screen content changed = PTY output activity. This is what
                // moves processOnly sessions to running and refreshes working
                // recency for full-model agents. Cheap — do it every change.
                guard tracked[id] != nil else { continue }
                let activity = tracked[id]!.reducer.reduce(.ptyOutputActivity)
                await apply(activity, currentStatus: tracked[id]!.reducer.status, to: id)

                // Same signal, published for automation. Coalesced hard: this
                // branch fires at up to 10Hz per busy session, and a subscriber
                // wants "it is producing output", not a byte-accurate firehose
                // it would have to rate-limit itself.
                if let last = tracked[id]?.lastOutputEvent,
                    Date().timeIntervalSince(last) >= Self.outputEventInterval
                {
                    tracked[id]?.lastOutputEvent = Date()
                    await registry?.publishOutputActivity(sessionID: id, contentSeq: seq)
                }

                // The snapshot + manifest regex is the expensive part. A busy
                // agent (spinner) bumps contentSeq every 100ms tick; evaluating
                // that fast just burns CPU — needs-input/turn detection is fine
                // at ~5Hz. Throttle it while keeping the cheap activity above.
                // Re-fetch after the await above: the session can be untracked
                // while apply() is suspended (mass kill on daemon swap).
                let now = Date()
                if let manifests, let lastEval = tracked[id]?.lastEval,
                    now.timeIntervalSince(lastEval) >= 0.2
                {
                    tracked[id]?.lastEval = now
                    let snapshot = await entry.session.makeSnapshot()
                    tracked[id]?.lastScannedSeq = seq
                    if let observation = manifests.evaluate(
                        snapshot, manifestID: kindManifestID(for: id))
                    {
                        guard tracked[id] != nil else { continue }
                        let outcome = tracked[id]!.reducer.reduce(.screen(observation))
                        await apply(outcome, currentStatus: tracked[id]!.reducer.status, to: id)
                    }
                }
            }
            guard tracked[id] != nil else { continue }
            let outcome = tracked[id]!.reducer.reduce(.tick)
            await apply(outcome, currentStatus: tracked[id]!.reducer.status, to: id)
        }
    }

    private func kindManifestID(for id: SessionID) -> String {
        guard let entry = tracked[id] else { return "generic" }
        return entry.effectiveKind.manifestID
    }

    private func apply(
        _ outcome: ReducerOutcome, currentStatus: SessionStatus, to sessionID: SessionID
    ) async {
        guard outcome.statusChange != nil || outcome.needsInput != nil || outcome.turnCompleted
        else { return }
        await registry?.applyStatusOutcome(
            sessionID: sessionID,
            status: outcome.statusChange,
            needsInput: outcome.needsInput,
            turnCompleted: outcome.turnCompleted
        )
    }
}
