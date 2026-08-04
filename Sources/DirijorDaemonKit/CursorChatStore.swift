import Foundation
import SQLite3

/// Read-only access to Cursor CLI's per-chat metadata stores under
/// `~/.cursor/chats/<workspace>/<chat-id>/store.db`.
enum CursorChatStore {
    struct Metadata: Sendable {
        let agentID: String
        let title: String
        let createdAt: Date
        let database: URL
    }

    private struct StoredMetadata: Decodable {
        let agentId: String
        let name: String?
        let createdAt: Double
    }

    /// Cursor does not expose its server-minted chat id at launch. Match the
    /// store it creates alongside the Dirijor session, excluding stores already
    /// claimed by another live Cursor session.
    static func find(
        createdAt: Date,
        chatsRoot: URL,
        excluding: Set<URL> = [],
        fileManager: FileManager = .default
    ) -> Metadata? {
        guard let workspaces = try? fileManager.contentsOfDirectory(
            at: chatsRoot, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
        else { return nil }

        var best: (distance: TimeInterval, metadata: Metadata)?
        for workspace in workspaces {
            guard let chats = try? fileManager.contentsOfDirectory(
                at: workspace, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
            else { continue }
            for chat in chats {
                let database = chat.appendingPathComponent("store.db")
                guard !excluding.contains(database),
                    fileManager.fileExists(atPath: database.path),
                    let metadata = metadata(database: database),
                    metadata.agentID == chat.lastPathComponent
                else { continue }

                let delta = metadata.createdAt.timeIntervalSince(createdAt)
                // Cursor creates the empty chat as its TUI starts. A generous
                // five-minute forward window tolerates slow startup. Only a
                // tiny clock/race cushion is allowed before session creation,
                // so a recent unrelated chat cannot be adopted while Cursor
                // itself is still starting.
                guard delta >= -5, delta <= 5 * 60 else { continue }
                let distance = abs(delta)
                if best == nil || distance < best!.distance {
                    best = (distance, metadata)
                }
            }
        }
        return best?.metadata
    }

    static func metadata(database: URL) -> Metadata? {
        var connection: OpaquePointer?
        guard sqlite3_open_v2(
            database.path, &connection, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOMUTEX, nil) == SQLITE_OK,
            let connection
        else {
            if connection != nil { sqlite3_close(connection) }
            return nil
        }
        defer { sqlite3_close(connection) }
        sqlite3_busy_timeout(connection, 100)

        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(
            connection, "SELECT value FROM meta WHERE key = '0' LIMIT 1", -1, &statement, nil)
            == SQLITE_OK,
            let statement
        else { return nil }
        defer { sqlite3_finalize(statement) }
        guard sqlite3_step(statement) == SQLITE_ROW,
            let raw = sqlite3_column_text(statement, 0)
        else { return nil }

        let hex = String(cString: raw)
        guard let data = decodeHex(hex),
            let stored = try? JSONDecoder().decode(StoredMetadata.self, from: data),
            !stored.agentId.isEmpty
        else { return nil }
        let seconds = stored.createdAt > 10_000_000_000
            ? stored.createdAt / 1_000
            : stored.createdAt
        return Metadata(
            agentID: stored.agentId,
            title: stored.name ?? "",
            createdAt: Date(timeIntervalSince1970: seconds),
            database: database)
    }

    static func usableTitle(_ raw: String) -> String? {
        let title = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty else { return nil }
        switch title.lowercased() {
        case "untitled", "new chat", "new agent", "agent": return nil
        default: return title
        }
    }

    private static func decodeHex(_ string: String) -> Data? {
        guard string.count.isMultiple(of: 2) else { return nil }
        var data = Data(capacity: string.count / 2)
        var index = string.startIndex
        while index < string.endIndex {
            let next = string.index(index, offsetBy: 2)
            guard let byte = UInt8(string[index..<next], radix: 16) else { return nil }
            data.append(byte)
            index = next
        }
        return data
    }
}
