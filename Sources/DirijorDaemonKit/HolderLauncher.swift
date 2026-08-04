import Darwin
import DirijorHolderKit
import Foundation

enum HolderLauncher {
    static func launch(
        executablePath: String,
        paths: HolderPaths,
        spec: HolderLaunchSpec
    ) throws -> pid_t {
        try FileManager.default.createDirectory(
            at: paths.directory, withIntermediateDirectories: true)
        // A concurrent revive or pre-manager holder may already own this exact
        // session. Adopt it without starting an otherwise-idle manager.
        if HolderClient(socketPath: paths.socket.path).isAlive(),
            let servingPID = readPIDFile(paths.pidFile)
        {
            return servingPID
        }
        let managerPaths = HolderManagerPaths(directory: paths.directory)
        let lockFD = open(managerPaths.launchLock.path, O_CREAT | O_RDWR, 0o600)
        guard lockFD >= 0 else {
            throw HolderError.launch(
                "open \(managerPaths.launchLock.path): \(String(cString: strerror(errno)))")
        }
        defer {
            _ = flock(lockFD, LOCK_UN)
            Darwin.close(lockFD)
        }
        guard flock(lockFD, LOCK_EX) == 0 else {
            throw HolderError.launch(
                "lock \(managerPaths.launchLock.path): \(String(cString: strerror(errno)))")
        }

        let manager = HolderManagerClient(socketPath: managerPaths.socket.path)
        if !manager.isAlive() {
            _ = try spawnManager(
                executablePath: executablePath,
                directory: managerPaths.directory)
            var ready = false
            for _ in 0..<250 {
                if manager.isAlive() {
                    ready = true
                    break
                }
                usleep(20_000)
            }
            guard ready else {
                throw HolderError.launch("shared holder manager did not become ready")
            }
        }

        do {
            return try manager.launch(spec)
        } catch {
            // The manager may have crossed its no-session idle boundary between
            // our readiness check and the launch request. One fresh-manager
            // retry is safe while the cross-daemon launch lock is held.
            guard !manager.isAlive() else { throw error }
            _ = try spawnManager(
                executablePath: executablePath,
                directory: managerPaths.directory)
            for _ in 0..<250 {
                if let pid = try? manager.launch(spec) { return pid }
                usleep(20_000)
            }
            throw HolderError.launch("shared holder manager did not accept launch")
        }
    }

    private static func spawnManager(
        executablePath: String,
        directory: URL
    ) throws -> pid_t {
        let arguments = [executablePath, "--manager", directory.path]
        let argv: [UnsafeMutablePointer<CChar>?] = arguments.map { strdup($0) } + [nil]
        defer { argv.forEach { free($0) } }

        var attributes: posix_spawnattr_t?
        var actions: posix_spawn_file_actions_t?
        posix_spawnattr_init(&attributes)
        posix_spawn_file_actions_init(&actions)
        defer {
            posix_spawnattr_destroy(&attributes)
            posix_spawn_file_actions_destroy(&actions)
        }

        // A new session prevents terminal/SIGHUP coupling to the daemon. One
        // manager owns all session holders; macOS does not kill it when its
        // daemon parent exits, so every managed PTY survives crashes/upgrades.
        let flags = Int16(POSIX_SPAWN_SETSID | POSIX_SPAWN_CLOEXEC_DEFAULT)
        posix_spawnattr_setflags(&attributes, flags)
        posix_spawn_file_actions_addopen(&actions, STDIN_FILENO, "/dev/null", O_RDONLY, 0)
        posix_spawn_file_actions_addopen(&actions, STDOUT_FILENO, "/dev/null", O_WRONLY, 0)
        posix_spawn_file_actions_addopen(&actions, STDERR_FILENO, "/dev/null", O_WRONLY, 0)

        var pid: pid_t = -1
        let result = executablePath.withCString { path in
            posix_spawn(&pid, path, &actions, &attributes, argv, environ)
        }
        guard result == 0, pid > 0 else {
            throw HolderError.launch(
                "posix_spawn \(executablePath): \(String(cString: strerror(result)))")
        }
        return pid
    }

    private static func readPIDFile(_ url: URL) -> pid_t? {
        guard
            let text = try? String(contentsOf: url, encoding: .utf8),
            let pid = Int32(text.trimmingCharacters(in: .whitespacesAndNewlines)),
            pid > 1
        else { return nil }
        return pid
    }

    static func defaultExecutablePath() -> String {
        if let configured = ProcessInfo.processInfo.environment["DIRIJOR_HOLDER_PATH"],
            FileManager.default.isExecutableFile(atPath: configured)
        {
            return configured
        }

        let executable = URL(fileURLWithPath: CommandLine.arguments[0])
            .resolvingSymlinksInPath()
        let candidates = [
            executable.deletingLastPathComponent().appendingPathComponent("dirijord-holder"),
            Bundle.main.executableURL?.deletingLastPathComponent()
                .appendingPathComponent("dirijord-holder"),
            URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
                .appendingPathComponent(".build/debug/dirijord-holder"),
        ].compactMap { $0 }
        return candidates.first {
            FileManager.default.isExecutableFile(atPath: $0.path)
        }?.path ?? candidates[0].path
    }
}

private let POSIX_SPAWN_SETSID: Int32 = 0x0400
private let POSIX_SPAWN_CLOEXEC_DEFAULT: Int32 = 0x4000
