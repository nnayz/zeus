import DirijorCore
import Foundation

/// Watches agent transcript metadata for better conversation titles: Claude's
/// generated `ai-title`, Codex's first prompt when notify omitted it, and
/// Cursor CLI's generated chat name.
///
/// Polling runs every 5s. Claude reads only appended JSONL bytes; Codex stops at
/// its first prompt; Cursor reads only the single metadata row in its chat DB.
actor TitleWatcher {
    private let registry: SessionRegistry
    private let codexSessionsRoot: URL
    private let cursorChatsRoot: URL
    private var offsets: [SessionID: UInt64] = [:]
    private var codexTranscriptPaths: [SessionID: URL] = [:]
    private var cursorStorePaths: [SessionID: URL] = [:]
    private var task: Task<Void, Never>?

    init(
        registry: SessionRegistry,
        codexSessionsRoot: URL = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".codex/sessions"),
        cursorChatsRoot: URL = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".cursor/chats")
    ) {
        self.registry = registry
        self.codexSessionsRoot = codexSessionsRoot
        self.cursorChatsRoot = cursorChatsRoot
    }

    func start() {
        guard task == nil else { return }
        task = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                await self?.scan()
            }
        }
    }

    func stop() {
        task?.cancel()
        task = nil
    }

    func scan() async {
        let claudeCandidates = await registry.claudeTitleCandidates()
        for (sessionID, path) in claudeCandidates {
            guard let title = latestAITitle(sessionID: sessionID, path: path) else { continue }
            await registry.applyAgentTitle(sessionID: sessionID, title: TitleMaker.fromFirstPrompt(title))
        }
        let liveClaude = Set(claudeCandidates.map(\.0))
        offsets = offsets.filter { liveClaude.contains($0.key) }

        let codexCandidates = await registry.codexTitleCandidates()
        for (sessionID, agentID, createdAt) in codexCandidates {
            let transcript = codexTranscriptPaths[sessionID]
                ?? CodexTranscript.find(
                    agentID: agentID, createdAt: createdAt,
                    sessionsRoot: codexSessionsRoot)
            guard let transcript else { continue }
            codexTranscriptPaths[sessionID] = transcript
            guard let prompt = CodexTranscript.firstUserPrompt(file: transcript) else { continue }
            await registry.applyFirstPromptTitle(
                sessionID: sessionID, title: TitleMaker.fromFirstPrompt(prompt))
        }
        let liveCodex = Set(codexCandidates.map(\.0))
        codexTranscriptPaths = codexTranscriptPaths.filter { liveCodex.contains($0.key) }

        let cursorCandidates = await registry.cursorTitleCandidates()
            .sorted { $0.1 > $1.1 }
        let liveCursor = Set(cursorCandidates.map(\.0))
        cursorStorePaths = cursorStorePaths.filter { liveCursor.contains($0.key) }
        var claimedStores = Set(cursorStorePaths.values)
        for (sessionID, createdAt) in cursorCandidates {
            let metadata: CursorChatStore.Metadata?
            if let database = cursorStorePaths[sessionID] {
                metadata = CursorChatStore.metadata(database: database)
            } else {
                metadata = CursorChatStore.find(
                    createdAt: createdAt,
                    chatsRoot: cursorChatsRoot,
                    excluding: claimedStores)
            }
            guard let metadata else { continue }
            cursorStorePaths[sessionID] = metadata.database
            claimedStores.insert(metadata.database)
            guard let title = CursorChatStore.usableTitle(metadata.title) else { continue }
            await registry.applyAgentTitle(
                sessionID: sessionID, title: TitleMaker.fromFirstPrompt(title))
        }
    }

    /// Reads bytes appended since the last scan and returns the newest aiTitle.
    private func latestAITitle(sessionID: SessionID, path: String) -> String? {
        guard let handle = FileHandle(forReadingAtPath: path) else { return nil }
        defer { try? handle.close() }
        let start = offsets[sessionID] ?? 0
        guard let end = try? handle.seekToEnd(), end > start else { return nil }
        try? handle.seek(toOffset: start)
        guard let data = try? handle.read(upToCount: Int(min(end - start, 4 << 20))) else {
            return nil
        }
        // Re-scan a partial trailing line next tick rather than losing it.
        var consumed = data.count
        if data.last != UInt8(ascii: "\n"),
            let lastNewline = data.lastIndex(of: UInt8(ascii: "\n"))
        {
            consumed = lastNewline + 1
        }
        offsets[sessionID] = start + UInt64(consumed)

        guard let text = String(data: data.prefix(consumed), encoding: .utf8) else { return nil }
        var newest: String?
        for line in text.split(separator: "\n") where line.contains("\"ai-title\"") {
            guard let obj = try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any],
                obj["type"] as? String == "ai-title",
                let title = obj["aiTitle"] as? String, !title.isEmpty
            else { continue }
            newest = title
        }
        return newest
    }
}
