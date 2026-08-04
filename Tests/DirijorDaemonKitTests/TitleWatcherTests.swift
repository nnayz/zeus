import DirijorCore
import DirijorProtocol
import Foundation
import Testing

@testable import DirijorDaemonKit

@Test func codexPlaceholderPromotesFromRolloutFirstPrompt() async throws {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-codex-title-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: root) }

    let sessionsRoot = root.appendingPathComponent(".codex/sessions")
    let agentID = "019f5b72-390a-79a0-aa43-3891cd826759"
    let stateFile = root.appendingPathComponent("state.json")
    let config = DaemonConfig(
        socketPath: root.appendingPathComponent("d.sock").path,
        cliPath: "/usr/bin/true",
        injectDir: root,
        logsDir: root,
        stateFile: stateFile
    )
    let registry = SessionRegistry(config: config, events: EventBus())
    let session = try await registry.spawn(SessionSpawnParams(kind: .shell, cwd: "/tmp"))

    // CodexTranscript.find intentionally searches only the session creation
    // day ±1. Keep the fixture's day tied to the generated record rather than
    // a wall-clock date that eventually ages out of that bounded search.
    var utc = Calendar(identifier: .gregorian)
    utc.timeZone = TimeZone(secondsFromGMT: 0)!
    let parts = utc.dateComponents([.year, .month, .day], from: session.createdAt)
    let date = String(
        format: "%04d-%02d-%02d",
        parts.year ?? 1970, parts.month ?? 1, parts.day ?? 1)
    let day = sessionsRoot.appendingPathComponent(date.replacingOccurrences(of: "-", with: "/"))
    try FileManager.default.createDirectory(at: day, withIntermediateDirectories: true)
    let transcript = day.appendingPathComponent("rollout-\(date)T12-29-38-\(agentID).jsonl")
    let lines = [
        #"{"type":"session_meta","payload":{"id":"\#(agentID)","cwd":"/tmp"}}"#,
        #"{"type":"event_msg","payload":{"type":"user_message","message":"Fix Codex titles after the first prompt"}}"#,
    ]
    try (lines.joined(separator: "\n") + "\n")
        .write(to: transcript, atomically: true, encoding: .utf8)

    await registry.applyForegroundAgent(sessionID: session.id, agent: .codex)
    await registry.applyHookMetadata(
        sessionID: session.id,
        meta: HookMetadata(agentSessionID: agentID)
    )
    let watcher = TitleWatcher(registry: registry, codexSessionsRoot: sessionsRoot)

    let located = CodexTranscript.find(
        agentID: agentID, createdAt: session.createdAt, sessionsRoot: sessionsRoot)
    #expect(located?.resolvingSymlinksInPath() == transcript.resolvingSymlinksInPath())
    #expect(CodexTranscript.firstUserPrompt(file: transcript) == "Fix Codex titles after the first prompt")
    #expect(await registry.codexTitleCandidates().contains { $0.0 == session.id })

    await watcher.scan()

    let updated = try #require(await registry.record(session.id))
    #expect(updated.title == "Fix Codex titles after the first prompt")
    #expect(updated.titleSource == .firstPrompt)

    try await registry.kill(sessionID: session.id)
}

@Test func cursorPlaceholderPromotesFromGeneratedChatName() async throws {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-cursor-title-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: root) }

    let chatsRoot = root.appendingPathComponent(".cursor/chats")
    let stateFile = root.appendingPathComponent("state.json")
    let config = DaemonConfig(
        socketPath: root.appendingPathComponent("d.sock").path,
        cliPath: "/usr/bin/true",
        injectDir: root,
        logsDir: root,
        stateFile: stateFile
    )
    let registry = SessionRegistry(config: config, events: EventBus())
    let session = try await registry.spawn(SessionSpawnParams(kind: .shell, cwd: "/tmp"))
    await registry.applyForegroundAgent(sessionID: session.id, agent: .cursor)

    let chatID = "8969d118-cec6-47db-84f4-c318fdd7206a"
    let chatDirectory = chatsRoot.appendingPathComponent("workspace/\(chatID)")
    try FileManager.default.createDirectory(at: chatDirectory, withIntermediateDirectories: true)
    let database = chatDirectory.appendingPathComponent("store.db")
    func encodedMetadata(name: String) throws -> String {
        let stored: [String: Any] = [
            "agentId": chatID,
            "latestRootBlobId": String(repeating: "0", count: 64),
            "name": name,
            "mode": "default",
            "createdAt": session.createdAt.timeIntervalSince1970 * 1_000,
        ]
        let data = try JSONSerialization.data(withJSONObject: stored)
        return data.map { String(format: "%02x", $0) }.joined()
    }
    func runSQLite(_ statement: String) throws {
        let sqlite = Process()
        sqlite.executableURL = URL(fileURLWithPath: "/usr/bin/sqlite3")
        sqlite.arguments = [database.path, statement]
        try sqlite.run()
        sqlite.waitUntilExit()
        #expect(sqlite.terminationStatus == 0)
    }
    try runSQLite(
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT); "
            + "INSERT INTO meta VALUES ('0', '\(encodedMetadata(name: "Untitled"))');")

    let watcher = TitleWatcher(
        registry: registry,
        codexSessionsRoot: root.appendingPathComponent(".codex/sessions"),
        cursorChatsRoot: chatsRoot)

    await watcher.scan()

    let placeholder = try #require(await registry.record(session.id))
    #expect(placeholder.titleSource == .placeholder)

    try runSQLite(
        "UPDATE meta SET value = '\(encodedMetadata(name: "Make Cursor titles follow the first prompt"))' "
            + "WHERE key = '0';")
    await watcher.scan()

    let updated = try #require(await registry.record(session.id))
    #expect(updated.title == "Make Cursor titles follow the first prompt")
    #expect(updated.titleSource == .agentProvided)

    try runSQLite(
        "UPDATE meta SET value = '\(encodedMetadata(name: "Cursor's revised generated title"))' "
            + "WHERE key = '0';")
    await watcher.scan()
    let revised = try #require(await registry.record(session.id))
    #expect(revised.title == "Cursor's revised generated title")

    try await registry.rename(sessionID: session.id, title: "My pinned title")
    try runSQLite(
        "UPDATE meta SET value = '\(encodedMetadata(name: "Cursor changed it again"))' "
            + "WHERE key = '0';")
    await watcher.scan()
    let renamed = try #require(await registry.record(session.id))
    #expect(renamed.title == "My pinned title")
    #expect(renamed.titleSource == .userRename)

    try await registry.kill(sessionID: session.id)
}
