import DirijorCore
import DirijorProtocol
import Foundation

struct PersistedState: Codable, Sendable {
    var version: Int = 1
    var projects: [Project] = []
    var sessions: [SessionRecord] = []
}

/// Debounced atomic JSON snapshots of daemon state.
actor PersistenceStore {
    private let fileURL: URL
    private var pending: PersistedState?
    private var writeTask: Task<Void, Never>?
    private let debounce: Duration

    init(fileURL: URL, debounce: Duration = .milliseconds(500)) {
        self.fileURL = fileURL
        self.debounce = debounce
    }

    func load() -> PersistedState? {
        guard let data = try? Data(contentsOf: fileURL) else { return nil }
        do {
            return try JSONDecoder.dirijor.decode(PersistedState.self, from: data)
        } catch {
            // An unreadable state file must never be mistaken for a fresh
            // install: the next persist would overwrite every session record.
            // Quarantine it so the records stay recoverable by hand.
            let quarantineURL = fileURL.appendingPathExtension("corrupt")
            try? FileManager.default.removeItem(at: quarantineURL)
            do {
                try FileManager.default.moveItem(at: fileURL, to: quarantineURL)
                DaemonLog.shared.log(
                    "state decode failed, quarantined to \(quarantineURL.lastPathComponent): \(error)")
            } catch let moveError {
                DaemonLog.shared.log(
                    "state decode failed (quarantine also failed: \(moveError)): \(error)")
            }
            return nil
        }
    }

    func schedule(_ state: PersistedState) {
        pending = state
        guard writeTask == nil else { return }
        writeTask = Task { [debounce] in
            try? await Task.sleep(for: debounce)
            self.writePending()
        }
    }

    func flushNow() {
        writeTask?.cancel()
        writeTask = nil
        writePendingSync()
    }

    private func writePending() {
        writeTask = nil
        writePendingSync()
    }

    private func writePendingSync() {
        guard let state = pending else { return }
        pending = nil
        do {
            let data = try JSONEncoder.dirijor.encode(state)
            try data.write(to: fileURL, options: .atomic)
            try FileManager.default.setAttributes(
                [.posixPermissions: NSNumber(value: Int16(0o600))],
                ofItemAtPath: fileURL.path)
        } catch {
            // A short-lived test or an uninstall can remove the containing
            // directory while a debounced write is still pending. There is no
            // useful recovery (or production signal) once its owner is gone.
            if FileManager.default.fileExists(atPath: fileURL.deletingLastPathComponent().path) {
                DaemonLog.shared.log("persist failed: \(error)")
            }
        }
    }
}
