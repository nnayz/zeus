import Foundation

/// Minimal timestamped file logger. One per daemon process.
public final class DaemonLog: @unchecked Sendable {
    private static let maxBytes = 4 << 20
    private let handle: FileHandle?
    private let lock = NSLock()
    private let formatter: ISO8601DateFormatter

    public static let shared = DaemonLog()

    private init() {
        formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let url = (try? DirijorPathsSafe.daemonLogFile()) ?? nil
        if let url {
            Self.rotateIfNeeded(url)
            FileManager.default.createFile(atPath: url.path, contents: nil)
            try? FileManager.default.setAttributes(
                [.posixPermissions: NSNumber(value: Int16(0o600))], ofItemAtPath: url.path)
            handle = try? FileHandle(forWritingTo: url)
            _ = try? handle?.seekToEnd()
        } else {
            handle = nil
        }
    }

    private static func rotateIfNeeded(_ url: URL) {
        let fm = FileManager.default
        guard let attrs = try? fm.attributesOfItem(atPath: url.path),
            let size = attrs[.size] as? NSNumber,
            size.intValue > maxBytes
        else { return }
        let previous = url.appendingPathExtension("1")
        try? fm.removeItem(at: previous)
        try? fm.moveItem(at: url, to: previous)
        try? fm.setAttributes(
            [.posixPermissions: NSNumber(value: Int16(0o600))], ofItemAtPath: previous.path)
    }

    public func log(_ message: String) {
        let line = "[\(formatter.string(from: Date()))] \(message)\n"
        lock.lock()
        defer { lock.unlock() }
        if let data = line.data(using: .utf8) {
            try? handle?.write(contentsOf: data)
        }
        #if DEBUG
        FileHandle.standardError.write(Data(line.utf8))
        #endif
    }
}

import DirijorProtocol

private enum DirijorPathsSafe {
    static func daemonLogFile() throws -> URL {
        try DirijorPaths.ensureDirectoriesExist()
        return DirijorPaths.daemonLogFile
    }
}
