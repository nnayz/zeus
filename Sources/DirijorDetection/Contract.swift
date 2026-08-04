import DirijorCore
import Foundation

// The public API surface of the detection engine. The daemon codes against
// exactly these signatures; the implementation lives in the other files of
// this target. Signatures here may gain members but must not break.

// MARK: - Screen input

/// A snapshot of one session's terminal, produced by the daemon's headless emulator.
public struct ScreenSnapshot: Sendable, Hashable {
    /// Visible grid rows, top to bottom, ANSI-stripped plain text.
    public var lines: [String]
    /// Last OSC 0/2 window title the child set, if any.
    public var oscTitle: String?
    /// Last OSC 9;4 progress report: (state, value 0-100), if any.
    public var oscProgressState: Int?
    public var oscProgressValue: Int?
    /// Monotonic counter bumped whenever screen content changes; used to skip
    /// redundant scans and to detect stale observations.
    public var contentSeq: UInt64
    public var cols: Int
    public var rows: Int

    public init(
        lines: [String],
        oscTitle: String? = nil,
        oscProgressState: Int? = nil,
        oscProgressValue: Int? = nil,
        contentSeq: UInt64,
        cols: Int,
        rows: Int
    ) {
        self.lines = lines
        self.oscTitle = oscTitle
        self.oscProgressState = oscProgressState
        self.oscProgressValue = oscProgressValue
        self.contentSeq = contentSeq
        self.cols = cols
        self.rows = rows
    }
}

// MARK: - Screen output

public enum ManifestState: String, Codable, Sendable, Hashable {
    case working
    case idle
    case blockedPermission
    case blockedQuestion
    /// The screen shows a transient view (transcript viewer, model picker) —
    /// hold the current state, don't transition.
    case skip
}

/// The manifest engine's verdict for one snapshot.
public struct ScreenObservation: Sendable, Hashable {
    public var state: ManifestState
    public var matchedRuleID: String
    public var priority: Int
    public var contentSeq: UInt64
    /// Captured region text (the prompt box) for needs-me routing.
    public var promptExcerpt: String?
    /// Scraped numbered options if the blocker presented any.
    public var options: [String]?

    public init(
        state: ManifestState,
        matchedRuleID: String,
        priority: Int,
        contentSeq: UInt64,
        promptExcerpt: String? = nil,
        options: [String]? = nil
    ) {
        self.state = state
        self.matchedRuleID = matchedRuleID
        self.priority = priority
        self.contentSeq = contentSeq
        self.promptExcerpt = promptExcerpt
        self.options = options
    }
}

// MARK: - Status signals

/// Claude hook events, pre-parsed by the daemon from the raw hook payload.
public enum ClaudeHookSignal: Sendable, Hashable {
    case sessionStart(source: String?, agentSessionID: String?, transcriptPath: String?)
    case userPromptSubmit(promptText: String?)
    case preToolUse(toolName: String?, toolInputSummary: String?)
    case permissionRequest(toolName: String?, toolInputSummary: String?)
    /// notificationType: permission_prompt | idle_prompt | agent_needs_input |
    /// agent_completed | elicitation_dialog
    case notification(notificationType: String?, message: String?)
    case stop
    case subagentStart(agentID: String)
    case subagentStop(agentID: String)
    case sessionEnd(reason: String?)
}

/// Everything the reducer can consume. The daemon translates raw sources
/// (hooks, notify, screen scans, PTY events) into these.
public enum StatusSignal: Sendable {
    /// `isSubagent` is true when the hook payload carried an agent_id —
    /// such events must never drive the parent session's canonical state.
    case claudeHook(ClaudeHookSignal, isSubagent: Bool)
    case codexTurnComplete(lastAssistantMessage: String?)
    case screen(ScreenObservation)
    case ptyOutputActivity
    case userKeystroke
    case processExit(code: Int32?, signal: Int32?)
    /// Periodic tick (~100ms) driving debounce timers.
    case tick
}

// MARK: - Reducer

/// Timing knobs (herdr-proven defaults).
public struct ReducerTiming: Sendable {
    public var idleConfirmations: Int = 3
    public var recheckInterval: TimeInterval = 0.1
    public var idleConfirmCap: TimeInterval = 0.7
    public var startupGrace: TimeInterval = 3.0
    public var hookAuthorityWindow: TimeInterval = 7.0
    public var blockerClearScans: Int = 2
    public var stalenessTimeout: TimeInterval = 60.0

    public init() {}
}

/// The outcome of reducing one signal.
public struct ReducerOutcome: Sendable {
    /// Non-nil when the canonical status changed.
    public var statusChange: SessionStatus?
    /// Non-nil when a needs-input detail was produced or updated.
    public var needsInput: NeedsInputDetail?
    /// True when a turn just completed (drives lastTurnCompletedAt / done-unseen).
    public var turnCompleted: Bool

    public init(
        statusChange: SessionStatus? = nil,
        needsInput: NeedsInputDetail? = nil,
        turnCompleted: Bool = false
    ) {
        self.statusChange = statusChange
        self.needsInput = needsInput
        self.turnCompleted = turnCompleted
    }
}

/// Pure per-session state machine. One instance per session, owned by the
/// daemon's status engine; all methods are synchronous and side-effect free.
public struct StatusReducer: Sendable {
    /// hooksPrimary: Claude (hooks drive state, screen arbitrates blockers).
    /// screenPrimary: Codex (screen drives state, notify confirms done).
    /// processOnly: generic agents (starting → working → exited only).
    public enum Authority: String, Sendable {
        case hooksPrimary
        case screenPrimary
        case processOnly
    }

    public internal(set) var status: SessionStatus
    public let authority: Authority
    public var timing: ReducerTiming

    public init(authority: Authority, timing: ReducerTiming = ReducerTiming(), spawnedAt: Date = Date()) {
        self.authority = authority
        self.timing = timing
        self.status = .starting
        self.internalState = InternalState(spawnedAt: spawnedAt)
    }

    /// Implemented in StatusReducerImpl.swift.
    public mutating func reduce(_ signal: StatusSignal, now: Date = Date()) -> ReducerOutcome {
        reduceImpl(signal, now: now)
    }

    // Internal mutable state, defined alongside the implementation.
    var internalState: InternalState
}

// MARK: - Manifest engine

/// Loads detection manifests (bundled JSON + user overrides) and evaluates
/// snapshots against them. Thread-safe after init; hot-reload creates a new engine.
public struct ManifestEngine: Sendable {
    /// Loads bundled manifests from this target's resources, then applies
    /// overrides from `overridesDir` (files named `<manifest-id>.json`).
    public init(overridesDir: URL? = nil) throws {
        try self.init(loadingBundledWithOverrides: overridesDir)
    }

    /// Evaluates a snapshot against the manifest for `manifestID`
    /// ("claude-code" | "codex" | "generic"). Returns nil when no rule matches.
    public func evaluate(_ snapshot: ScreenSnapshot, manifestID: String) -> ScreenObservation? {
        evaluateImpl(snapshot, manifestID: manifestID)
    }

    var storage: EngineStorage
}
