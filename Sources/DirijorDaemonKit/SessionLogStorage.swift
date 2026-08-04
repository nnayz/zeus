import DirijorCore
import Foundation

/// Owns the on-disk retention contract for raw PTY output. Session records and
/// agent-native transcripts remain authoritative; these files are only a
/// bounded reattach/replay cache and may be evicted safely at daemon startup.
enum SessionLogStorage {
    static let perSessionBytes = 8 << 20
    static let totalBytes = 256 << 20

    static func url(directory: URL, sessionID: SessionID) -> URL {
        directory.appendingPathComponent("\(sessionID.rawValue).bin")
    }

    static func remove(directory: URL, sessionID: SessionID) {
        try? FileManager.default.removeItem(at: url(directory: directory, sessionID: sessionID))
    }

    /// Removes orphaned files first, then oldest evictable caches until the
    /// global budget is met. `protectedSessionIDs` are live sessions with open
    /// fds; runtime pruning never unlinks those files.
    static func prune(
        directory: URL,
        keeping sessionIDs: Set<SessionID>,
        protectedSessionIDs: Set<SessionID> = [],
        budget: Int = totalBytes
    ) {
        let fm = FileManager.default
        guard let files = try? fm.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: [.fileSizeKey, .contentModificationDateKey],
            options: [.skipsHiddenFiles])
        else { return }

        var retained: [(url: URL, id: SessionID, size: Int, modified: Date)] = []
        for file in files where file.pathExtension == "bin" {
            let id = SessionID(rawValue: file.deletingPathExtension().lastPathComponent)
            guard sessionIDs.contains(id) else {
                try? fm.removeItem(at: file)
                continue
            }
            let values = try? file.resourceValues(forKeys: [.fileSizeKey, .contentModificationDateKey])
            retained.append(
                (file, id, values?.fileSize ?? 0, values?.contentModificationDate ?? .distantPast))
        }

        var total = retained.reduce(0) { $0 + $1.size }
        for item in retained.sorted(by: { $0.modified < $1.modified })
        where total > budget && !protectedSessionIDs.contains(item.id) {
            do {
                try fm.removeItem(at: item.url)
                total -= item.size
            } catch {}
        }
    }
}
