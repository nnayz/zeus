import Foundation

public enum RiskHint: String, Codable, Hashable, Sendable {
    case destructive
    case network
    case fileWrite
    case neutral

    /// Heuristic classification of a shell command / tool summary.
    public static func classify(_ text: String) -> RiskHint {
        let lower = text.lowercased()
        let destructivePatterns = [
            "rm -rf", "rm -fr", "sudo ", "force-push", "push --force", "push -f",
            "reset --hard", "clean -fd", "drop table", "mkfs", "> /dev/",
        ]
        if destructivePatterns.contains(where: lower.contains) { return .destructive }
        let networkPatterns = ["curl ", "wget ", "http://", "https://", "ssh ", "scp ", "nc "]
        if networkPatterns.contains(where: lower.contains) { return .network }
        let writePatterns = ["write", "edit", "mv ", "cp ", "mkdir", "touch ", "chmod", "chown"]
        if writePatterns.contains(where: lower.contains) { return .fileWrite }
        return .neutral
    }
}

/// Everything we know about *why* a session needs the user, rich enough to render
/// "Claude wants to run `rm -rf build`" in the menu bar / a notification / iOS.
public struct NeedsInputDetail: Codable, Hashable, Sendable {
    public enum Source: String, Codable, Hashable, Sendable {
        case claudePermissionHook
        case claudeNotificationHook
        case codexNotify
        case screenScrape
    }

    public var kind: NeedsInputKind
    public var source: Source
    /// Tool being gated, if known ("Bash", "Edit", "mcp__github__create_pr").
    public var toolName: String?
    /// Pre-formatted, redacted human line, ≤200 chars.
    public var summary: String
    /// Raw captured screen text of the prompt box, ≤400 chars. Never leaves the Mac.
    public var promptExcerpt: String?
    /// Scraped numbered options, if the prompt presented any.
    public var options: [String]?
    public var riskHint: RiskHint
    public var occurredAt: Date

    public init(
        kind: NeedsInputKind,
        source: Source,
        toolName: String? = nil,
        summary: String,
        promptExcerpt: String? = nil,
        options: [String]? = nil,
        riskHint: RiskHint = .neutral,
        occurredAt: Date = Date()
    ) {
        self.kind = kind
        self.source = source
        self.toolName = toolName
        self.summary = String(summary.prefix(200))
        self.promptExcerpt = promptExcerpt.map { String($0.prefix(400)) }
        self.options = options
        self.riskHint = riskHint
        self.occurredAt = occurredAt
    }

    /// Identity used for notification dedupe: one notification per distinct blocker.
    public var blockerIdentity: Int {
        var hasher = Hasher()
        hasher.combine(kind)
        hasher.combine(toolName)
        hasher.combine(summary)
        return hasher.finalize()
    }
}

public enum Redaction {
    /// Masks obvious secrets and strips control characters before text leaves the daemon.
    public static func redact(_ text: String) -> String {
        var result = text.replacingOccurrences(
            of: #"(?i)(token|secret|password|api[_-]?key|authorization)\s*[=:]\s*\S+"#,
            with: "$1=•••",
            options: .regularExpression
        )
        result = result.replacingOccurrences(
            of: #"\u{1B}\[[0-9;?]*[a-zA-Z]"#,
            with: "",
            options: .regularExpression
        )
        return result
    }
}
