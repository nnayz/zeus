import Foundation

/// Where a session title came from. Higher raw value wins; an update is applied
/// iff `newSource >= currentSource` (equal source may refresh content, e.g. Codex
/// renaming a thread). `userRename` is absolute and never auto-replaced.
public enum TitleSource: Int, Comparable, Codable, Hashable, Sendable {
    case placeholder = 0
    case firstPrompt = 1
    case agentProvided = 2
    case dirijorAssigned = 3
    case userRename = 4

    public static func < (lhs: TitleSource, rhs: TitleSource) -> Bool {
        lhs.rawValue < rhs.rawValue
    }
}

public enum TitleMaker {
    /// Collapses whitespace and truncates a raw first prompt into a usable title.
    public static func fromFirstPrompt(_ prompt: String, maxLength: Int = 60) -> String {
        let collapsed = prompt
            .components(separatedBy: .whitespacesAndNewlines)
            .filter { !$0.isEmpty }
            .joined(separator: " ")
        guard collapsed.count > maxLength else { return collapsed }
        return String(collapsed.prefix(maxLength - 1)) + "…"
    }

    public static func placeholder(kind: AgentKind, cwd: String, date: Date = Date()) -> String {
        let folder = (cwd as NSString).lastPathComponent
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm"
        return "\(kind.displayName) — \(formatter.string(from: date)) — \(folder)"
    }
}
