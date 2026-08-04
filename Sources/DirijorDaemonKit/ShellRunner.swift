import DirijorProtocol
import Foundation

/// Outcome of one subprocess run through `ShellRunner`.
public struct ShellResult: Sendable, Equatable {
    public var exitCode: Int32
    public var stdout: String
    public var stderr: String

    public var ok: Bool { exitCode == 0 }
    /// stdout with trailing whitespace/newlines stripped (git-style output).
    public var trimmedStdout: String {
        stdout.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    public init(exitCode: Int32, stdout: String = "", stderr: String = "") {
        self.exitCode = exitCode
        self.stdout = stdout
        self.stderr = stderr
    }
}

public struct ShellTimeoutError: Error, CustomStringConvertible {
    public var argv: [String]
    public var timeout: Duration
    public var description: String {
        "command timed out after \(timeout): \(argv.joined(separator: " "))"
    }
}

/// Injectable subprocess seam for the daemon's remote operations (ssh, rsync,
/// scp, git). Tests swap `run` for a local executor or canned responses; the
/// live runner execs directly (no shell) with a hard timeout.
public struct ShellRunner: Sendable {
    public var run:
        @Sendable (_ argv: [String], _ cwd: String?, _ timeout: Duration) async throws
            -> ShellResult

    public init(
        run: @escaping @Sendable (_ argv: [String], _ cwd: String?, _ timeout: Duration)
            async throws -> ShellResult
    ) {
        self.run = run
    }

    /// Real subprocess execution. argv[0] resolves via the user's login PATH
    /// (the launchd-bare daemon can't see Homebrew otherwise) with /usr/bin as
    /// the fallback for the stock tools (ssh, scp, rsync, git). On timeout the
    /// child gets SIGTERM then SIGKILL and the call throws.
    ///
    /// Deliberately posix_spawn + waitpid, NOT Foundation.Process: NSTask's
    /// termination notification silently never fires under the swift-testing
    /// harness (waitUntilExit hangs after the child is gone), and its pipes
    /// block cooperative-executor threads. Pipe draining and the wait happen
    /// on GCD threads, never the concurrency pool.
    public static let live = ShellRunner { argv, cwd, timeout in
        guard let first = argv.first else {
            return ShellResult(exitCode: 127, stderr: "empty argv")
        }
        let executable =
            first.hasPrefix("/")
            ? first
            : LoginEnvironment.resolve(first) ?? "/usr/bin/\(first)"

        var outPipe: [Int32] = [-1, -1]
        var errPipe: [Int32] = [-1, -1]
        guard pipe(&outPipe) == 0, pipe(&errPipe) == 0 else {
            outPipe.filter { $0 >= 0 }.forEach { close($0) }
            return ShellResult(exitCode: 127, stderr: "pipe() failed")
        }

        var actions: posix_spawn_file_actions_t?
        posix_spawn_file_actions_init(&actions)
        defer { posix_spawn_file_actions_destroy(&actions) }
        posix_spawn_file_actions_addopen(&actions, STDIN_FILENO, "/dev/null", O_RDONLY, 0)
        posix_spawn_file_actions_adddup2(&actions, outPipe[1], STDOUT_FILENO)
        posix_spawn_file_actions_adddup2(&actions, errPipe[1], STDERR_FILENO)
        for fd in [outPipe[0], outPipe[1], errPipe[0], errPipe[1]] {
            posix_spawn_file_actions_addclose(&actions, fd)
        }
        if let cwd {
            posix_spawn_file_actions_addchdir_np(&actions, cwd)
        }

        let arguments = [executable] + argv.dropFirst()
        let cargv: [UnsafeMutablePointer<CChar>?] = arguments.map { strdup($0) } + [nil]
        defer { cargv.forEach { free($0) } }

        var pid: pid_t = -1
        let rc = executable.withCString { path in
            posix_spawn(&pid, path, &actions, nil, cargv, environ)
        }
        close(outPipe[1])
        close(errPipe[1])
        guard rc == 0, pid > 0 else {
            close(outPipe[0])
            close(errPipe[0])
            return ShellResult(
                exitCode: 127,
                stderr: "posix_spawn \(executable): \(String(cString: strerror(rc)))")
        }

        // Timeout side-task: mark + kill; the gather below then finishes
        // naturally (EOF + reap), so the wait can never leak.
        let childPID = pid
        let timedOut = TimeoutFlag()
        let killer = Task {
            try? await Task.sleep(for: timeout)
            guard !Task.isCancelled else { return }
            timedOut.set()
            kill(childPID, SIGTERM)
            try? await Task.sleep(for: .milliseconds(500))
            guard !Task.isCancelled else { return }
            kill(childPID, SIGKILL)
        }
        let outFD = outPipe[0]
        let errFD = errPipe[0]
        let result: ShellResult = await withCheckedContinuation { continuation in
            DispatchQueue.global(qos: .utility).async {
                // stderr drains on its own thread so a child that fills one
                // pipe while we block on the other can't wedge either side.
                let group = DispatchGroup()
                var err = Data()
                group.enter()
                DispatchQueue.global(qos: .utility).async {
                    err = drainFD(errFD)
                    group.leave()
                }
                let out = drainFD(outFD)
                group.wait()
                var status: Int32 = 0
                while waitpid(childPID, &status, 0) == -1, errno == EINTR {}
                // WIFEXITED / WEXITSTATUS / WTERMSIG by hand (no Swift macros).
                let code = (status & 0x7f) == 0 ? (status >> 8) & 0xff : 128 + (status & 0x7f)
                continuation.resume(
                    returning: ShellResult(
                        exitCode: code,
                        stdout: String(decoding: out, as: UTF8.self),
                        stderr: String(decoding: err, as: UTF8.self)))
            }
        }
        killer.cancel()
        if timedOut.value {
            throw ShellTimeoutError(argv: argv, timeout: timeout)
        }
        return result
    }
}

/// Reads `fd` to EOF and closes it.
private func drainFD(_ fd: Int32) -> Data {
    var data = Data()
    var buffer = [UInt8](repeating: 0, count: 64 << 10)
    while true {
        let count = read(fd, &buffer, buffer.count)
        if count > 0 {
            data.append(contentsOf: buffer[0..<count])
        } else if count == 0 || errno != EINTR {
            break
        }
    }
    close(fd)
    return data
}

private final class TimeoutFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var flag = false

    var value: Bool {
        lock.lock()
        defer { lock.unlock() }
        return flag
    }

    func set() {
        lock.lock()
        flag = true
        lock.unlock()
    }
}

/// Runs shell command LINES either locally (`/bin/sh -c`) or on a remote host
/// (`ssh <dest> -- <line>`) through the same `ShellRunner` seam — the single
/// abstraction the prefs sync, repo locator, and session migrator share, so
/// tests can treat a local directory as a fake "remote".
struct HostShell: Sendable {
    var runner: ShellRunner
    /// nil ⇒ local execution.
    var host: HostEntry?

    /// Non-interactive ssh client options shared by every daemon-initiated
    /// remote command: keys only (no password prompt to hang on), bounded
    /// connect time, and the same first-contact policy as remote spawns.
    static let sshOptions: [String] = [
        "-o", "BatchMode=yes",
        "-o", "ConnectTimeout=10",
        "-o", "StrictHostKeyChecking=accept-new",
    ]

    func argv(_ command: String) -> [String] {
        if let host {
            return ["ssh"] + Self.sshOptions + [host.ssh, "--", command]
        }
        return ["/bin/sh", "-c", command]
    }

    func run(_ command: String, timeout: Duration = .seconds(30)) async throws -> ShellResult {
        try await runner.run(argv(command), nil, timeout)
    }

    var describesHost: String { host?.displayName ?? "local" }
}
