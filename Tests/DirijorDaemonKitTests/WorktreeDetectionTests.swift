import Darwin
import DirijorCore
import DirijorGit
import Foundation
import Testing

@testable import DirijorDaemonKit

/// The signal that drives the sidebar's worktree hover: a linked git worktree
/// (`.git` is a file) vs the main checkout (`.git` is a directory). Exercised
/// against REAL git so it tracks git's actual on-disk layout — including Claude
/// Code's own convention of nesting worktrees under `.claude/worktrees/<name>/`.
@Test func isLinkedWorktreeDistinguishesMainFromLinked() throws {
    let git = "/usr/bin/git"
    guard FileManager.default.fileExists(atPath: git) else { return }

    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-wt-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let repo = root.appendingPathComponent("repo").path

    // Two hazards here, both of which hung this test on CI for twelve minutes
    // until the job was killed with no failure reported.
    //
    // A `Pipe()` nobody reads deadlocks the child as soon as it fills the
    // buffer, and `waitUntilExit()` then waits forever for a process that can
    // never make progress. Send the output to /dev/null: this test asserts on
    // exit status, never on what git printed.
    //
    // And an unbounded wait on a subprocess has no place in a test suite — a
    // git that stalls for any reason at all should fail this test, not stall
    // the run. Bound it and say which command hung.
    func run(_ args: [String], in dir: String) throws {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: git)
        p.arguments = args
        p.currentDirectoryURL = URL(fileURLWithPath: dir)
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        let finished = DispatchSemaphore(value: 0)
        p.terminationHandler = { _ in finished.signal() }
        try p.run()
        let command = "git \(args.joined(separator: " "))"
        guard finished.wait(timeout: .now() + 60 * testTimeoutScale) == .success else {
            p.terminate()
            Issue.record("\(command) did not exit; terminated it")
            return
        }
        #expect(p.terminationStatus == 0, "\(command)")
    }

    try FileManager.default.createDirectory(
        at: URL(fileURLWithPath: repo), withIntermediateDirectories: true)
    try run(["init", "-q", "-b", "main"], in: repo)
    try run(["config", "user.email", "t@t.t"], in: repo)
    try run(["config", "user.name", "t"], in: repo)
    FileManager.default.createFile(atPath: repo + "/f.txt", contents: Data("x".utf8))
    try run(["add", "."], in: repo)
    try run(["commit", "-q", "-m", "init"], in: repo)

    // A worktree the way Claude Code makes them: nested under .claude/worktrees/.
    let claudeWt = repo + "/.claude/worktrees/bright-fox"
    try run(["worktree", "add", "-q", "-b", "worktree-bright-fox", claudeWt], in: repo)
    // A subdir inside it — detection walks up to the worktree's .git file.
    let claudeSub = claudeWt + "/src"
    try FileManager.default.createDirectory(
        at: URL(fileURLWithPath: claudeSub), withIntermediateDirectories: true)

    // Main checkout: .git is a directory ⇒ NOT a linked worktree.
    #expect(GitHead.isLinkedWorktree(repo) == false)
    // Claude's worktree and a subdir of it: .git is a file ⇒ linked worktree.
    #expect(GitHead.isLinkedWorktree(claudeWt) == true)
    #expect(GitHead.isLinkedWorktree(claudeSub) == true)
    // And its branch reads from the linked HEAD, not the main checkout's.
    #expect(GitHead.branch(inWorkingDir: claudeWt) == "worktree-bright-fox")
    #expect(GitHead.branch(inWorkingDir: repo) == "main")

    // A plain non-git directory is never a worktree.
    #expect(GitHead.isLinkedWorktree(root.path) == false)
}

/// `agentWorkingDir()` must reflect the process's ACTUAL cwd, not the spawn cwd —
/// that's what lets the branch monitor notice an agent that chdir'd into a
/// worktree on its own.
@Test func agentWorkingDirReadsLiveCwd() async throws {
    let logs = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-cwd-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: logs, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: logs) }

    // A real, symlink-resolved directory to spawn in (/tmp → /private/tmp on macOS).
    let workURL = URL(fileURLWithPath: logs.path).resolvingSymlinksInPath()
    let work = workURL.path

    let session = AgentSession(
        id: SessionID(rawValue: "s_cwd"), kind: .shell, logDirectory: logs)
    try await session.start(argv: ["/bin/cat"], cwd: work, extraEnv: [:]) { _, _ in }
    defer { Task { await session.terminate(graceSeconds: 0.2) } }

    try await waitUntil(timeout: .seconds(5)) { await session.pid > 0 }

    try await waitUntil(timeout: .seconds(5)) {
        (await session.agentWorkingDir()).map {
            URL(fileURLWithPath: $0).resolvingSymlinksInPath().path == work
        } ?? false
    }

    await session.terminate(graceSeconds: 0.2)
    try await waitUntil(timeout: .seconds(5)) { await session.isRunning == false }
}
