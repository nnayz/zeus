import CDirijorPTY
import Darwin
import Foundation

struct PTYError: Error, CustomStringConvertible {
    var message: String
    var description: String { "pty: \(message)" }
}

/// Low-level PTY + spawn helpers. All synchronous; called from AgentSession.
enum PTY {
    struct Spawned {
        var masterFD: Int32
        var pid: pid_t
    }

    /// Opens a PTY pair and spawns `argv` in a new session whose controlling
    /// terminal is the slave.
    ///
    /// We fork+execve rather than posix_spawn because acquiring the controlling
    /// terminal on macOS requires an explicit `TIOCSCTTY` ioctl in the child —
    /// which `posix_spawn` file actions can't express. Without it, strict shells
    /// like fish fail job-control setup (`tcgetpgrp failed`) and exit; lenient
    /// ones like zsh limp along without job control. So we do the classic
    /// `login_tty` dance by hand.
    static func spawn(
        argv: [String],
        cwd: String,
        environment: [String: String],
        cols: UInt16,
        rows: UInt16
    ) throws -> Spawned {
        // C strings owned here for the duration of the call; the C helper only
        // uses them synchronously (and copies them into the child via execve).
        let cArgv: [UnsafeMutablePointer<CChar>?] = argv.map { strdup($0) } + [nil]
        let cEnv: [UnsafeMutablePointer<CChar>?] =
            environment.map { strdup("\($0.key)=\($0.value)") } + [nil]
        defer {
            cArgv.forEach { free($0) }
            cEnv.forEach { free($0) }
        }

        var master: Int32 = -1
        let pid = cwd.withCString { cCwd in
            argv[0].withCString { cPath in
                dirijor_pty_spawn(cPath, cArgv, cEnv, cCwd, cols, rows, &master)
            }
        }
        guard pid > 0 else {
            throw PTYError(message: "pty spawn of \(argv[0]) failed")
        }
        return Spawned(masterFD: master, pid: pid)
    }

    static func resize(masterFD: Int32, cols: UInt16, rows: UInt16) {
        var winsize = Darwin.winsize(ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0)
        _ = ioctl(masterFD, TIOCSWINSZ, &winsize)
    }
}

// POSIX_SPAWN_SETSID / CLOEXEC_DEFAULT are Darwin-specific and not always
// surfaced by the overlay; define them defensively.
private let POSIX_SPAWN_SETSID: Int32 = 0x0400
private let POSIX_SPAWN_CLOEXEC_DEFAULT: Int32 = 0x4000
