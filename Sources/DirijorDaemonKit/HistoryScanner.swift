import DirijorCore
import DirijorProtocol
import Foundation

/// Scans the coding agents' own on-disk transcript stores for past
/// conversations, independent of any daemon session record. This is what
/// powers the History panel: unlike the in-memory `recentlyClosed` reopen
/// stack, these entries survive app quit and daemon restart because the
/// underlying `.jsonl` files do.
///
/// - Claude Code: `~/.claude/projects/<encoded-cwd>/<uuid>.jsonl`. The filename
///   stem is the session UUID that `claude --resume` takes; the authoritative
///   `cwd` and titles are read out of the transcript's own JSON lines (the
///   directory-name encoding is lossy — see `InjectionBuilder.claudeTranscriptPath`).
/// - Codex: `~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-<ts>-<uuid>.jsonl`,
///   whose first line is a `session_meta` record carrying the id + cwd.
///
/// Reads are bounded per file (transcripts can run to many MB) and the whole
/// scan runs synchronously inline on the connection actor, matching
/// `WorktreeOverviewBuilder`.
enum HistoryScanner {
    /// Defensive cap on returned entries (mirrors `DirectoryIndex`).
    private static let maxEntries = 500
    /// How far into a Claude transcript we'll read looking for the first line
    /// carrying `cwd` (the first user message, which can be large when images
    /// were pasted inline). We early-stop as soon as that line is complete.
    private static let claudeHeadCap = 8 << 20
    /// Tail window scanned for the newest Claude `ai-title` (matches TitleWatcher).
    private static let claudeTailBytes = 16 << 10
    /// Cap on a Codex `session_meta` first line.
    private static let codexFirstLineCap = 512 << 10

    /// All resumable past conversations not already tracked by the daemon
    /// (`tracked` = agent-session ids currently in `SessionRegistry.records`),
    /// newest first.
    static func scan(excluding tracked: Set<String>) -> [HistoryEntry] {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return scan(
            claudeRoot: home.appendingPathComponent(".claude/projects"),
            codexRoot: home.appendingPathComponent(".codex/sessions"),
            excluding: tracked)
    }

    /// Root-injectable core, so tests can point at fixture transcript trees.
    static func scan(claudeRoot: URL, codexRoot: URL, excluding tracked: Set<String>) -> [HistoryEntry] {
        let fm = FileManager.default
        var entries = scanClaude(root: claudeRoot, fm: fm)
        entries += scanCodex(root: codexRoot, fm: fm)

        var seen = tracked
        var deduped: [HistoryEntry] = []
        for entry in entries where !seen.contains(entry.id) {
            seen.insert(entry.id)
            deduped.append(entry)
        }
        deduped.sort { $0.lastActiveAt > $1.lastActiveAt }
        if deduped.count > maxEntries { deduped = Array(deduped.prefix(maxEntries)) }
        return deduped
    }

    // MARK: - Claude

    private static func scanClaude(root: URL, fm: FileManager) -> [HistoryEntry] {
        guard let projectDirs = try? fm.contentsOfDirectory(
            at: root, includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles])
        else { return [] }

        var result: [HistoryEntry] = []
        for dir in projectDirs {
            guard (try? dir.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true,
                let files = try? fm.contentsOfDirectory(
                    at: dir,
                    includingPropertiesForKeys: [.contentModificationDateKey, .creationDateKey],
                    options: [.skipsHiddenFiles])
            else { continue }
            for file in files where file.pathExtension == "jsonl" {
                if let entry = claudeEntry(file: file, fm: fm) { result.append(entry) }
            }
        }
        return result
    }

    private static func claudeEntry(file: URL, fm: FileManager) -> HistoryEntry? {
        // Filename stem is the UUID `claude --resume` resolves against.
        let uuid = file.deletingPathExtension().lastPathComponent
        guard uuid.count >= 32 else { return nil }

        let rv = try? file.resourceValues(forKeys: [.contentModificationDateKey, .creationDateKey])
        let lastActive = rv?.contentModificationDate ?? .distantPast

        // Head: the first line carrying `cwd` is the first user message, which
        // also holds the first prompt text — grab both from it.
        var cwd: String?
        var firstPrompt: String?
        for line in readClaudeHead(file: file) {
            guard let obj = try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any]
            else { continue }
            if firstPrompt == nil { firstPrompt = claudeUserText(obj) }
            if let c = obj["cwd"] as? String, !c.isEmpty {
                cwd = c
                break
            }
        }
        guard let cwd else { return nil }  // no cwd → nothing to resume into

        let title =
            latestClaudeAITitle(file: file)
            ?? firstPrompt.map { TitleMaker.fromFirstPrompt($0) }

        return HistoryEntry(
            id: uuid, kind: .claudeCode, cwd: cwd, title: title,
            transcriptPath: file.path, lastActiveAt: lastActive,
            createdAt: rv?.creationDate, cwdExists: fm.fileExists(atPath: cwd))
    }

    /// The user prompt text from a Claude `user` transcript line, if any.
    private static func claudeUserText(_ obj: [String: Any]) -> String? {
        guard obj["type"] as? String == "user",
            let message = obj["message"] as? [String: Any]
        else { return nil }
        if let str = message["content"] as? String, !str.isEmpty { return str }
        if let items = message["content"] as? [[String: Any]] {
            for item in items where item["type"] as? String == "text" {
                if let text = item["text"] as? String, !text.isEmpty { return text }
            }
        }
        return nil
    }

    /// Reads transcript bytes until the first line containing `"cwd"` is
    /// complete (or the cap is hit), returning the complete lines read.
    private static func readClaudeHead(file: URL) -> [String] {
        guard let handle = try? FileHandle(forReadingFrom: file) else { return [] }
        defer { try? handle.close() }
        let cwdKey = Data("\"cwd\"".utf8)
        var data = Data()
        while data.count < claudeHeadCap {
            let want = min(64 << 10, claudeHeadCap - data.count)
            guard let chunk = try? handle.read(upToCount: want), !chunk.isEmpty else { break }
            data.append(chunk)
            if let match = data.range(of: cwdKey),
                data[match.upperBound...].firstIndex(of: 0x0A) != nil
            {
                break  // the cwd-bearing line is now complete
            }
        }
        return String(decoding: data, as: UTF8.self)
            .split(separator: "\n", omittingEmptySubsequences: true)
            .map(String.init)
    }

    /// Newest `ai-title` in the transcript tail — the title Claude generates for
    /// the conversation (same record TitleWatcher promotes live).
    private static func latestClaudeAITitle(file: URL) -> String? {
        guard let handle = try? FileHandle(forReadingFrom: file) else { return nil }
        defer { try? handle.close() }
        guard let end = try? handle.seekToEnd() else { return nil }
        let start = end > UInt64(claudeTailBytes) ? end - UInt64(claudeTailBytes) : 0
        try? handle.seek(toOffset: start)
        guard let data = try? handle.read(upToCount: claudeTailBytes + (4 << 10)) else { return nil }
        var newest: String?
        for line in String(decoding: data, as: UTF8.self).split(separator: "\n")
        where line.contains("\"ai-title\"") {
            guard let obj = try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any],
                obj["type"] as? String == "ai-title",
                let title = obj["aiTitle"] as? String, !title.isEmpty
            else { continue }
            newest = title
        }
        return newest
    }

    // MARK: - Codex

    private static func scanCodex(root: URL, fm: FileManager) -> [HistoryEntry] {
        var result: [HistoryEntry] = []
        // Bounded YYYY/MM/DD walk — not a generic recursive walker.
        for year in childDirs(of: root, fm: fm) {
            for month in childDirs(of: year, fm: fm) {
                for day in childDirs(of: month, fm: fm) {
                    guard let files = try? fm.contentsOfDirectory(
                        at: day,
                        includingPropertiesForKeys: [.contentModificationDateKey, .creationDateKey],
                        options: [.skipsHiddenFiles])
                    else { continue }
                    for file in files
                    where file.pathExtension == "jsonl"
                        && file.lastPathComponent.hasPrefix("rollout-") {
                        if let entry = codexEntry(file: file, fm: fm) { result.append(entry) }
                    }
                }
            }
        }
        return result
    }

    private static func codexEntry(file: URL, fm: FileManager) -> HistoryEntry? {
        guard let first = readFirstLine(file: file, cap: codexFirstLineCap),
            let obj = try? JSONSerialization.jsonObject(with: Data(first.utf8)) as? [String: Any],
            obj["type"] as? String == "session_meta",
            let payload = obj["payload"] as? [String: Any],
            let id = payload["id"] as? String,
            let cwd = payload["cwd"] as? String, !cwd.isEmpty
        else { return nil }

        let rv = try? file.resourceValues(forKeys: [.contentModificationDateKey, .creationDateKey])
        let lastActive = rv?.contentModificationDate ?? .distantPast
        // Codex has no Claude-style ai-title record. Its first user_message is
        // the same title fallback used for live sessions.
        let title = CodexTranscript.firstUserPrompt(file: file)
            .map { TitleMaker.fromFirstPrompt($0) }
            ?? TitleMaker.placeholder(
                kind: .codex, cwd: cwd, date: rv?.creationDate ?? lastActive)

        return HistoryEntry(
            id: id, kind: .codex, cwd: cwd, title: title,
            transcriptPath: file.path, lastActiveAt: lastActive,
            createdAt: rv?.creationDate, cwdExists: fm.fileExists(atPath: cwd))
    }

    // MARK: - Shared helpers

    private static func childDirs(of url: URL, fm: FileManager) -> [URL] {
        let children = (try? fm.contentsOfDirectory(
            at: url, includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles])) ?? []
        return children.filter {
            (try? $0.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true
        }
    }

    private static func readFirstLine(file: URL, cap: Int) -> String? {
        guard let handle = try? FileHandle(forReadingFrom: file) else { return nil }
        defer { try? handle.close() }
        var data = Data()
        while data.count < cap {
            let want = min(64 << 10, cap - data.count)
            guard let chunk = try? handle.read(upToCount: want), !chunk.isEmpty else { break }
            data.append(chunk)
            if let newline = data.firstIndex(of: 0x0A) {
                return String(decoding: data[..<newline], as: UTF8.self)
            }
        }
        return data.isEmpty ? nil : String(decoding: data, as: UTF8.self)
    }
}
