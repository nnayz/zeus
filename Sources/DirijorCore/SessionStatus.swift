import Foundation

public enum NeedsInputKind: String, Codable, Hashable, Sendable {
    case permission
    case question
}

public enum ExitReason: String, Codable, Hashable, Sendable {
    /// The child process exited on its own.
    case exited
    /// The child was killed by a signal.
    case signaled
    /// No live holder could be adopted after a daemon or machine restart.
    case daemonRestart
    /// Imported from agent CLI state on disk; never ran under Dirijor.
    case external
    /// Archived by the user: the tree was killed to free resources, but the
    /// record is kept so the conversation can be revived via resume.
    case archived
}

public struct ExitInfo: Codable, Hashable, Sendable {
    public var reason: ExitReason
    public var code: Int32?
    public var signal: Int32?

    public init(reason: ExitReason, code: Int32? = nil, signal: Int32? = nil) {
        self.reason = reason
        self.code = code
        self.signal = signal
    }
}

/// Canonical session state as decided by the status engine.
///
/// Note there is no `done` case on purpose: "done" is *derived attention*
/// (`idle` + the user hasn't looked since the turn completed) — see `AttentionLevel`.
public enum SessionStatus: Codable, Hashable, Sendable {
    case starting
    case idle
    case working
    case needsInput(NeedsInputKind)
    case exited(ExitInfo)
    case unknown

    public var isRunning: Bool {
        switch self {
        case .exited: false
        default: true
        }
    }

    /// Canonical flat string for wire payloads, CLI tables, and `--until`
    /// comparison. Associated values are appended after a colon so the coarse
    /// name still prefix-matches (`"exited:archived"`.hasPrefix("exited")).
    public var label: String {
        switch self {
        case .starting: "starting"
        case .idle: "idle"
        case .working: "working"
        case .needsInput(let kind): "needsInput:\(kind.rawValue)"
        case .exited(let info): "exited:\(info.reason.rawValue)"
        case .unknown: "unknown"
        }
    }

    /// Does this status satisfy an `events.wait --until` target? Accepts the
    /// case names plus the vocabulary the MCP tools and the CLI expose
    /// ("done" ⇒ idle, "needs_me"/"needs-input"/"blocked" ⇒ needsInput), so
    /// every caller resolves the same aliases instead of each inventing a map.
    public func satisfies(waitTarget target: String) -> Bool {
        switch target {
        case "idle", "done": if case .idle = self { return true }
        case "working": if case .working = self { return true }
        case "starting": if case .starting = self { return true }
        case "unknown": if case .unknown = self { return true }
        case "needsInput", "needs_input", "needs-input", "needs_me", "blocked":
            if case .needsInput = self { return true }
        case "exited", "dead": if case .exited = self { return true }
        default: return false
        }
        return false
    }

    /// Every `--until` target the CLI accepts, for help text and validation.
    public static let waitTargets = [
        "done", "idle", "working", "starting", "needs-input", "exited",
    ]
}
