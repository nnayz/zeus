import DirijorCore
import DirijorProtocol
import Foundation
import Testing

@testable import DirijorDaemonKit

/// Fixtures mirror the real on-disk transcript shapes verified against
/// `~/.claude/projects` and `~/.codex/sessions`.
private struct Fixtures {
    let claudeRoot: URL
    let codexRoot: URL

    init() {
        let base = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("historyscanner-\(UUID().uuidString)")
        claudeRoot = base.appendingPathComponent(".claude/projects")
        codexRoot = base.appendingPathComponent(".codex/sessions")
        try? FileManager.default.createDirectory(at: claudeRoot, withIntermediateDirectories: true)
        try? FileManager.default.createDirectory(at: codexRoot, withIntermediateDirectories: true)
    }

    func cleanup() {
        try? FileManager.default.removeItem(at: claudeRoot.deletingLastPathComponent().deletingLastPathComponent())
    }

    /// Writes a Claude transcript `<uuid>.jsonl` under an encoded-cwd project dir.
    func writeClaude(uuid: String, cwd: String, firstPrompt: String, aiTitle: String?, imageBlobBytes: Int = 0) {
        let encoded = cwd.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ".", with: "-")
        let dir = claudeRoot.appendingPathComponent(encoded)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

        var lines: [String] = []
        lines.append(#"{"type":"mode","mode":"normal","sessionId":"\#(uuid)"}"#)
        // The first user line carries `cwd` near its end, after any inline image
        // blob — exactly the layout that defeats a naive fixed-size head read.
        let blob = imageBlobBytes > 0 ? String(repeating: "A", count: imageBlobBytes) : ""
        let content =
            imageBlobBytes > 0
            ? #"[{"type":"text","text":"\#(firstPrompt)"},{"type":"image","source":{"type":"base64","data":"\#(blob)"}}]"#
            : #"[{"type":"text","text":"\#(firstPrompt)"}]"#
        lines.append(
            #"{"type":"user","message":{"role":"user","content":\#(content)},"sessionId":"\#(uuid)","cwd":"\#(cwd)","gitBranch":"main"}"#
        )
        if let aiTitle {
            lines.append(#"{"type":"ai-title","aiTitle":"\#(aiTitle)","sessionId":"\#(uuid)"}"#)
        }
        let path = dir.appendingPathComponent("\(uuid).jsonl")
        try? (lines.joined(separator: "\n") + "\n").write(to: path, atomically: true, encoding: .utf8)
    }

    /// Writes a Codex rollout `.jsonl` whose first line is `session_meta`.
    func writeCodex(
        id: String, cwd: String, firstPrompt: String = "Fix Codex chat titles",
        y: String = "2026", m: String = "03", d: String = "10"
    ) {
        let dir = codexRoot.appendingPathComponent("\(y)/\(m)/\(d)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let meta =
            #"{"timestamp":"2026-03-10T13:09:23Z","type":"session_meta","payload":{"id":"\#(id)","cwd":"\#(cwd)","cli_version":"0.1"}}"#
        let more = #"{"type":"event_msg","payload":{"type":"user_message","message":"\#(firstPrompt)"}}"#
        let path = dir.appendingPathComponent("rollout-2026-03-10T15-09-14-\(id).jsonl")
        try? (meta + "\n" + more + "\n").write(to: path, atomically: true, encoding: .utf8)
    }
}

@Test func historyScannerReadsClaudeCwdAndAITitle() throws {
    let fx = Fixtures()
    defer { fx.cleanup() }
    let uuid = "10028003-b809-4417-b9ad-45a89bbf1bf2"
    fx.writeClaude(
        uuid: uuid, cwd: FileManager.default.homeDirectoryForCurrentUser.path,
        firstPrompt: "compare ghostty icons", aiTitle: "Compare icon display with Ghostty")

    let entries = HistoryScanner.scan(claudeRoot: fx.claudeRoot, codexRoot: fx.codexRoot, excluding: [])
    let entry = try #require(entries.first { $0.id == uuid })
    #expect(entry.kind == .claudeCode)
    #expect(entry.cwd == FileManager.default.homeDirectoryForCurrentUser.path)
    #expect(entry.title == "Compare icon display with Ghostty")  // ai-title wins
    #expect(entry.cwdExists)  // home always exists
    #expect(entry.transcriptPath.hasSuffix("\(uuid).jsonl"))
}

@Test func historyScannerFindsCwdPastAnInlineImageBlob() throws {
    let fx = Fixtures()
    defer { fx.cleanup() }
    let uuid = "22222222-2222-4222-8222-222222222222"
    // A 200 KB blob pushes `cwd` well past any fixed 64 KB head window.
    fx.writeClaude(
        uuid: uuid, cwd: FileManager.default.homeDirectoryForCurrentUser.path,
        firstPrompt: "hello", aiTitle: nil, imageBlobBytes: 200_000)

    let entries = HistoryScanner.scan(claudeRoot: fx.claudeRoot, codexRoot: fx.codexRoot, excluding: [])
    let entry = try #require(entries.first { $0.id == uuid })
    #expect(entry.cwd == FileManager.default.homeDirectoryForCurrentUser.path)
    #expect(entry.title == "hello")  // no ai-title → first-prompt fallback
}

@Test func historyScannerMarksMissingCwd() throws {
    let fx = Fixtures()
    defer { fx.cleanup() }
    let uuid = "33333333-3333-4333-8333-333333333333"
    fx.writeClaude(
        uuid: uuid, cwd: "/nonexistent/gone/folder",
        firstPrompt: "orphaned", aiTitle: nil)

    let entries = HistoryScanner.scan(claudeRoot: fx.claudeRoot, codexRoot: fx.codexRoot, excluding: [])
    let entry = try #require(entries.first { $0.id == uuid })
    #expect(!entry.cwdExists)
}

@Test func historyScannerReadsCodexSessionMeta() throws {
    let fx = Fixtures()
    defer { fx.cleanup() }
    let id = "019cd7dd-a95f-7620-9aa1-5f560cf1dfdb"
    fx.writeCodex(id: id, cwd: FileManager.default.homeDirectoryForCurrentUser.path)

    let entries = HistoryScanner.scan(claudeRoot: fx.claudeRoot, codexRoot: fx.codexRoot, excluding: [])
    let entry = try #require(entries.first { $0.id == id })
    #expect(entry.kind == .codex)
    #expect(entry.cwd == FileManager.default.homeDirectoryForCurrentUser.path)
    #expect(entry.cwdExists)
    #expect(entry.title == "Fix Codex chat titles")
}

@Test func historyScannerExcludesTrackedIDs() {
    let fx = Fixtures()
    defer { fx.cleanup() }
    let uuid = "44444444-4444-4444-8444-444444444444"
    fx.writeClaude(
        uuid: uuid, cwd: FileManager.default.homeDirectoryForCurrentUser.path,
        firstPrompt: "already open", aiTitle: nil)

    let entries = HistoryScanner.scan(
        claudeRoot: fx.claudeRoot, codexRoot: fx.codexRoot, excluding: [uuid])
    #expect(!entries.contains { $0.id == uuid })
}
