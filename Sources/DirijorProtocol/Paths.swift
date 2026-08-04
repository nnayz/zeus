import Foundation

/// Filesystem locations and environment variables shared by daemon, app, and CLI.
public enum DirijorPaths {
    public static var appSupport: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Dirijor", isDirectory: true)
    }

    public static var socket: URL { appSupport.appendingPathComponent("daemon.sock") }
    public static var stateFile: URL { appSupport.appendingPathComponent("state.json") }
    public static var logsDir: URL { appSupport.appendingPathComponent("logs", isDirectory: true) }
    public static var holdersDir: URL {
        appSupport.appendingPathComponent("holders", isDirectory: true)
    }
    public static var injectDir: URL { appSupport.appendingPathComponent("inject", isDirectory: true) }
    public static var binDir: URL { appSupport.appendingPathComponent("bin", isDirectory: true) }
    public static var manifestOverridesDir: URL {
        appSupport.appendingPathComponent("manifests/overrides", isDirectory: true)
    }
    public static var daemonLogFile: URL { appSupport.appendingPathComponent("dirijord.log") }
    /// Remote-access config (port + token). Present ⇔ remote TCP listener enabled.
    public static var remoteConfigFile: URL { appSupport.appendingPathComponent("remote.json") }
    /// Catalog of SSH-reachable execution hosts. Missing ⇔ Local only.
    public static var hostsConfigFile: URL { appSupport.appendingPathComponent("hosts.json") }

    public static func ensureDirectoriesExist() throws {
        for dir in [appSupport, logsDir, holdersDir, injectDir, binDir, manifestOverridesDir] {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try FileManager.default.setAttributes(
                [.posixPermissions: NSNumber(value: Int16(0o700))],
                ofItemAtPath: dir.path)
        }
        // Tighten existing installations as part of every daemon boot. These
        // files can contain terminal output, local paths, and pairing secrets.
        for file in [stateFile, daemonLogFile, remoteConfigFile,
                     appSupport.appendingPathComponent("dirijord.boot.log"),
                     appSupport.appendingPathComponent("daemon.lock")]
        where FileManager.default.fileExists(atPath: file.path) {
            try? FileManager.default.setAttributes(
                [.posixPermissions: NSNumber(value: Int16(0o600))],
                ofItemAtPath: file.path)
        }
        if let logs = try? FileManager.default.contentsOfDirectory(
            at: logsDir, includingPropertiesForKeys: nil) {
            for file in logs where file.pathExtension == "bin" {
                try? FileManager.default.setAttributes(
                    [.posixPermissions: NSNumber(value: Int16(0o600))],
                    ofItemAtPath: file.path)
            }
        }
    }
}

public enum DirijorEnv {
    /// Set in every PTY the daemon spawns; how helpers and MCP identify the caller.
    public static let sessionID = "DIRIJOR_SESSION_ID"
    /// Path to the daemon control socket.
    public static let socket = "DIRIJOR_SOCKET"
    /// Path to the `dirijor` CLI binary (hooks reference it via env, not hardcoded paths).
    public static let cli = "DIRIJOR_CLI"
}
