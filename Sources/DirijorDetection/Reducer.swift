import DirijorCore
import Foundation

// MARK: - Internal reducer state

/// All the mutable belief/debounce tracking a `StatusReducer` needs. Pure value
/// type; one instance per session, mutated only through `StatusReducer.reduce`.
struct InternalState: Sendable {
    var spawnedAt: Date
    /// Recency of the most recent meaningful signal — drives staleness → unknown.
    var lastSignalAt: Date

    /// A turn is in flight (prompt submitted / agent working) — gates turnCompleted.
    var turnInFlight: Bool = false
    /// Subagent ids currently running. Bookkeeping only — never canonical state.
    var activeSubagents: Set<String> = []

    // Working → idle anti-flicker.
    var idleCandidateSince: Date?
    var idleConfirms: Int = 0
    /// A strong idle signal (Claude Stop / codex turn-complete) lowers the
    /// confirmation requirement to 1.
    var idleStrong: Bool = false
    /// Fire `turnCompleted` exactly once on the next committed working→idle.
    var pendingTurnCompleted: Bool = false

    // On-screen blocker tracking.
    var screenBlockerActive: Bool = false
    var blockerMissScans: Int = 0

    // Screen belief.
    var screenBelief: ManifestState?
    var lastScreenSeq: UInt64?

    /// A `.skip` screen (transcript viewer / picker) is being held.
    var skipActive: Bool = false
    /// The user started typing a response to a needs-input prompt.
    var respondingSince: Date?
    /// Last produced needs-input detail (for re-emission / dedupe upstream).
    var pendingNeedsInput: NeedsInputDetail?

    init(spawnedAt: Date) {
        self.spawnedAt = spawnedAt
        self.lastSignalAt = spawnedAt
    }
}

// MARK: - Reduction

extension StatusReducer {
    mutating func reduceImpl(_ signal: StatusSignal, now: Date) -> ReducerOutcome {
        var outcome = ReducerOutcome()

        // `.exited` is absorbing: once dead, nothing changes.
        if case .exited = status { return outcome }

        // Process exit is authoritative for every authority mode.
        if case .processExit(let code, let sig) = signal {
            let reason: ExitReason = sig != nil ? .signaled : .exited
            setStatus(.exited(ExitInfo(reason: reason, code: code, signal: sig)), &outcome)
            return outcome
        }

        // processOnly: starting → working on first output; only exit changes it after.
        if authority == .processOnly {
            if case .ptyOutputActivity = signal {
                if status == .starting {
                    internalState.turnInFlight = true
                    setStatus(.working, &outcome)
                }
                internalState.lastSignalAt = now
            }
            return outcome
        }

        switch signal {
        case .processExit:
            break  // handled above
        case .ptyOutputActivity:
            // Refreshes recency of .working; never sets working for full models.
            internalState.lastSignalAt = now
        case .userKeystroke:
            internalState.lastSignalAt = now
            if case .needsInput = status { internalState.respondingSince = now }
        case .claudeHook(let hook, let isSubagent):
            handleClaudeHook(hook, isSubagent: isSubagent, now: now, &outcome)
        case .codexTurnComplete:
            internalState.lastSignalAt = now
            handleStrongIdle(now: now, &outcome)
        case .screen(let obs):
            handleScreen(obs, now: now, &outcome)
        case .tick:
            handleTick(now: now, &outcome)
        }

        return outcome
    }

    // MARK: Status mutation

    mutating func setStatus(_ new: SessionStatus, _ outcome: inout ReducerOutcome) {
        if status != new {
            status = new
            outcome.statusChange = new
        }
    }

    // MARK: Working / idle

    private mutating func cancelIdleCandidacy() {
        internalState.idleCandidateSince = nil
        internalState.idleConfirms = 0
        internalState.idleStrong = false
        internalState.pendingTurnCompleted = false
    }

    /// Enter (or refresh) the working state from a positive work signal.
    /// For hooks-primary, a work hook also clears a stale on-screen blocker.
    private mutating func goWorking(now: Date, clearScreenBlocker: Bool, _ outcome: inout ReducerOutcome) {
        cancelIdleCandidacy()
        if clearScreenBlocker {
            internalState.screenBlockerActive = false
            internalState.blockerMissScans = 0
        }
        internalState.turnInFlight = true
        internalState.lastSignalAt = now
        setStatus(.working, &outcome)
    }

    /// A strong idle signal (Stop / turn-complete). Commits immediately when an
    /// idle screen is already current, otherwise needs one further confirmation.
    private mutating func handleStrongIdle(now: Date, _ outcome: inout ReducerOutcome) {
        if status == .starting {
            // Definitive end-of-turn during startup → idle.
            setStatus(.idle, &outcome)
            return
        }
        guard status == .working else { return }
        internalState.idleStrong = true
        internalState.pendingTurnCompleted = internalState.turnInFlight
        if internalState.idleCandidateSince == nil { internalState.idleCandidateSince = now }
        if internalState.screenBelief == .idle {
            internalState.idleConfirms += 1
            commitIdle(now: now, &outcome)
        }
    }

    /// Register one idle-confirming observation (screen idle obs or tick re-check).
    private mutating func confirmIdle(now: Date, _ outcome: inout ReducerOutcome) {
        guard status == .working else { return }
        if internalState.idleCandidateSince == nil {
            internalState.idleCandidateSince = now
            internalState.idleConfirms = 0
        }
        internalState.idleConfirms += 1
        let required = internalState.idleStrong ? 1 : timing.idleConfirmations
        let elapsed = now.timeIntervalSince(internalState.idleCandidateSince ?? now)
        if internalState.idleConfirms >= required
            || (elapsed >= timing.idleConfirmCap && internalState.idleConfirms >= 1) {
            commitIdle(now: now, &outcome)
        }
    }

    private mutating func commitIdle(now: Date, _ outcome: inout ReducerOutcome) {
        let fire = internalState.pendingTurnCompleted
        setStatus(.idle, &outcome)
        internalState.turnInFlight = false
        if fire { outcome.turnCompleted = true }
        cancelIdleCandidacy()
        internalState.lastSignalAt = now
    }

    // MARK: Claude hooks

    private mutating func handleClaudeHook(
        _ hook: ClaudeHookSignal, isSubagent: Bool, now: Date, _ outcome: inout ReducerOutcome
    ) {
        internalState.lastSignalAt = now

        // Subagent lifecycle: bookkeeping only, never canonical state.
        switch hook {
        case .subagentStart(let id):
            internalState.activeSubagents.insert(id)
            return
        case .subagentStop(let id):
            internalState.activeSubagents.remove(id)
            return
        default:
            break
        }
        // Any event carrying an agent id is a subagent's — never touch parent state.
        if isSubagent { return }

        switch hook {
        case .sessionStart:
            // Definitive signal ending startup grace.
            if status == .starting { setStatus(.idle, &outcome) }
        case .userPromptSubmit:
            internalState.turnInFlight = true
            goWorking(now: now, clearScreenBlocker: true, &outcome)
        case .preToolUse:
            goWorking(now: now, clearScreenBlocker: true, &outcome)
        case .permissionRequest(let toolName, let inputSummary):
            let detail = buildPermissionDetail(toolName: toolName, inputSummary: inputSummary, now: now)
            internalState.pendingNeedsInput = detail
            outcome.needsInput = detail
            cancelIdleCandidacy()
            setStatus(.needsInput(.permission), &outcome)
        case .notification(let type, let message):
            handleNotification(type: type, message: message, now: now, &outcome)
        case .stop:
            handleStrongIdle(now: now, &outcome)
        case .sessionEnd:
            break  // hint only
        case .subagentStart, .subagentStop:
            break  // handled above
        }
    }

    private mutating func handleNotification(
        type: String?, message: String?, now: Date, _ outcome: inout ReducerOutcome
    ) {
        switch type {
        case "permission_prompt":
            let detail = NeedsInputDetail(
                kind: .permission, source: .claudeNotificationHook,
                summary: Redaction.redact(message ?? "Permission required"),
                riskHint: RiskHint.classify(message ?? ""), occurredAt: now)
            internalState.pendingNeedsInput = detail
            outcome.needsInput = detail
            cancelIdleCandidacy()
            setStatus(.needsInput(.permission), &outcome)
        case "idle_prompt", "agent_needs_input", "elicitation_dialog":
            let detail = NeedsInputDetail(
                kind: .question, source: .claudeNotificationHook,
                summary: Redaction.redact(message ?? "Waiting for input"),
                occurredAt: now)
            internalState.pendingNeedsInput = detail
            outcome.needsInput = detail
            cancelIdleCandidacy()
            setStatus(.needsInput(.question), &outcome)
        case "agent_completed":
            handleStrongIdle(now: now, &outcome)
        default:
            break
        }
    }

    private func buildPermissionDetail(
        toolName: String?, inputSummary: String?, now: Date
    ) -> NeedsInputDetail {
        let tn = toolName ?? ""
        let summary: String
        switch tn {
        case "Bash":
            summary = "wants to run `\(inputSummary ?? "")`"
        case "Edit", "Write", "MultiEdit", "NotebookEdit":
            summary = "wants to edit \(inputSummary ?? "a file")"
        case "":
            summary = inputSummary ?? "Permission required"
        default:
            summary = inputSummary.map { "wants to use \(tn): \($0)" } ?? "wants to use \(tn)"
        }
        return NeedsInputDetail(
            kind: .permission,
            source: .claudePermissionHook,
            toolName: toolName,
            summary: Redaction.redact(summary),
            promptExcerpt: inputSummary.map(Redaction.redact),
            riskHint: RiskHint.classify(inputSummary ?? tn),
            occurredAt: now)
    }

    // MARK: Screen

    private mutating func handleScreen(_ obs: ScreenObservation, now: Date, _ outcome: inout ReducerOutcome) {
        internalState.lastSignalAt = now

        // Skip screens hold current state and suppress screen-driven transitions.
        if obs.state == .skip {
            internalState.skipActive = true
            return
        }
        internalState.skipActive = false

        // Skip redundant scans when content hasn't changed.
        if let last = internalState.lastScreenSeq, last == obs.contentSeq { return }
        internalState.lastScreenSeq = obs.contentSeq
        internalState.screenBelief = obs.state

        // Visible on-screen blocker beats everything (except process exit).
        if let kind = obs.state.needsInputKind {
            internalState.screenBlockerActive = true
            internalState.blockerMissScans = 0
            let detail = buildScreenDetail(kind: kind, obs: obs, now: now)
            internalState.pendingNeedsInput = detail
            outcome.needsInput = detail
            cancelIdleCandidacy()
            setStatus(.needsInput(kind), &outcome)
            return
        }

        // Non-blocker observation while a screen blocker is active: only after
        // blockerClearScans consecutive misses do we release the blocker.
        if internalState.screenBlockerActive {
            internalState.blockerMissScans += 1
            guard internalState.blockerMissScans >= timing.blockerClearScans else { return }
            internalState.screenBlockerActive = false
            internalState.blockerMissScans = 0
            applyNonBlockerScreen(obs, now: now, clearedBlocker: true, &outcome)
            return
        }

        // Startup grace: hold `.starting` unless a definitive signal. A screen
        // working obs is definitive only for screen-primary agents (codex).
        if status == .starting {
            let graceActive = now.timeIntervalSince(internalState.spawnedAt) < timing.startupGrace
            let definitive = authority == .screenPrimary && obs.state == .working
            if graceActive && !definitive { return }
        }

        applyNonBlockerScreen(obs, now: now, clearedBlocker: false, &outcome)
    }

    private mutating func applyNonBlockerScreen(
        _ obs: ScreenObservation, now: Date, clearedBlocker: Bool, _ outcome: inout ReducerOutcome
    ) {
        switch obs.state {
        case .working:
            goWorking(now: now, clearScreenBlocker: false, &outcome)
        case .idle:
            if status == .working {
                confirmIdle(now: now, &outcome)
            } else if status == .starting {
                setStatus(.idle, &outcome)
            } else if clearedBlocker, case .needsInput = status {
                // Blocker released and the screen now reads idle.
                cancelIdleCandidacy()
                setStatus(.idle, &outcome)
            }
        case .blockedPermission, .blockedQuestion, .skip:
            break  // handled elsewhere
        }
    }

    private func buildScreenDetail(
        kind: NeedsInputKind, obs: ScreenObservation, now: Date
    ) -> NeedsInputDetail {
        let firstLine = obs.promptExcerpt?
            .split(separator: "\n", omittingEmptySubsequences: true)
            .map(String.init)
            .first { !$0.trimmingCharacters(in: .whitespaces).isEmpty }?
            .trimmingCharacters(in: .whitespaces)
        let summary = firstLine ?? "Waiting for input"
        return NeedsInputDetail(
            kind: kind,
            source: .screenScrape,
            summary: Redaction.redact(summary),
            promptExcerpt: obs.promptExcerpt,
            options: obs.options,
            riskHint: RiskHint.classify(obs.promptExcerpt ?? summary),
            occurredAt: now)
    }

    // MARK: Tick

    private mutating func handleTick(now: Date, _ outcome: inout ReducerOutcome) {
        // Staleness: running but unreadable → unknown (full models only).
        if status == .working,
           now.timeIntervalSince(internalState.lastSignalAt) > timing.stalenessTimeout {
            setStatus(.unknown, &outcome)
            return
        }
        // A tick re-check can supply the single confirmation a strong idle needs.
        if status == .working, internalState.idleStrong, internalState.idleCandidateSince != nil {
            internalState.idleConfirms += 1
            commitIdle(now: now, &outcome)
        }
    }
}
