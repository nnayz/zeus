import DirijorProtocol
import Foundation
import Testing

@testable import DirijorDaemonKit

private actor DiffInvocationRecorder {
    private(set) var argv: [String] = []

    func record(_ value: [String]) { argv = value }
}

@Test func remoteWorktreeDiffRunsOnTheConfiguredHost() async throws {
    let recorder = DiffInvocationRecorder()
    let patch = "diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-old\n+new\n"
    let runner = ShellRunner { argv, _, _ in
        await recorder.record(argv)
        return ShellResult(exitCode: 0, stdout: "/srv/app\0origin/main\0\(patch)")
    }
    let host = HostEntry(
        id: "forge", name: "Forge", ssh: "dev@forge", defaultCwd: "~/code")

    let result = try await WorktreeDiffLoader.load(
        cwd: "~/code app", host: host, runner: runner)

    #expect(result.repoRoot == "/srv/app")
    #expect(result.baseRef == "origin/main")
    #expect(String(decoding: result.patch, as: UTF8.self) == patch)
    #expect(!result.truncated)
    let argv = await recorder.argv
    #expect(argv.first == "ssh")
    #expect(argv.contains("dev@forge"))
    #expect(argv.last?.contains("cd ~/'code app'") == true)
}

@Test func worktreeDiffCommandIncludesTrackedAndUntrackedChanges() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-diff-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    try runGit(["init", "--quiet"], cwd: dir)
    try Data("before\n".utf8).write(to: dir.appendingPathComponent("tracked.txt"))
    try runGit(["add", "tracked.txt"], cwd: dir)
    try runGit(
        [
            "-c", "user.name=Dirijor Tests",
            "-c", "user.email=dirijor@example.invalid",
            "commit", "--quiet", "-m", "fixture",
        ],
        cwd: dir)
    try Data("after\n".utf8).write(to: dir.appendingPathComponent("tracked.txt"))
    try Data("new\n".utf8).write(to: dir.appendingPathComponent("untracked file.txt"))

    let result = try await WorktreeDiffLoader.load(
        cwd: dir.path, host: nil, runner: .live)
    let patch = String(decoding: result.patch, as: UTF8.self)

    #expect(patch.contains("diff --git a/tracked.txt b/tracked.txt"))
    #expect(patch.contains("diff --git a/untracked file.txt b/untracked file.txt"))
    #expect(patch.contains("+after"))
    #expect(patch.contains("+new"))
}

@Test func worktreeDiffDefaultsToCommittedChangesAgainstMain() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-branch-diff-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    try runGit(["init", "--quiet", "--initial-branch=main"], cwd: dir)
    try Data("on main\n".utf8).write(to: dir.appendingPathComponent("tracked.txt"))
    try runGit(["add", "tracked.txt"], cwd: dir)
    try runGit(
        [
            "-c", "user.name=Dirijor Tests",
            "-c", "user.email=dirijor@example.invalid",
            "commit", "--quiet", "-m", "main fixture",
        ],
        cwd: dir)
    try runGit(["checkout", "--quiet", "-b", "feature"], cwd: dir)
    try Data("on feature\n".utf8).write(to: dir.appendingPathComponent("tracked.txt"))
    try runGit(["add", "tracked.txt"], cwd: dir)
    try runGit(
        [
            "-c", "user.name=Dirijor Tests",
            "-c", "user.email=dirijor@example.invalid",
            "commit", "--quiet", "-m", "feature change",
        ],
        cwd: dir)

    let result = try await WorktreeDiffLoader.load(
        cwd: dir.path, host: nil, runner: .live)
    let patch = String(decoding: result.patch, as: UTF8.self)

    #expect(patch.contains("diff --git a/tracked.txt b/tracked.txt"))
    #expect(patch.contains("-on main"))
    #expect(patch.contains("+on feature"))
    #expect(result.baseRef == "main")
}

@Test func worktreeDiffCanCompareOnlyUncommittedChangesAgainstHead() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-head-diff-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    try runGit(["init", "--quiet", "--initial-branch=main"], cwd: dir)
    try Data("main\n".utf8).write(to: dir.appendingPathComponent("tracked.txt"))
    try runGit(["add", "tracked.txt"], cwd: dir)
    try runGit(
        [
            "-c", "user.name=Dirijor Tests",
            "-c", "user.email=dirijor@example.invalid",
            "commit", "--quiet", "-m", "fixture",
        ], cwd: dir)
    try runGit(["checkout", "--quiet", "-b", "feature"], cwd: dir)
    try Data("committed feature\n".utf8).write(to: dir.appendingPathComponent("tracked.txt"))
    try runGit(["add", "tracked.txt"], cwd: dir)
    try runGit(
        [
            "-c", "user.name=Dirijor Tests",
            "-c", "user.email=dirijor@example.invalid",
            "commit", "--quiet", "-m", "feature",
        ], cwd: dir)

    var result = try await WorktreeDiffLoader.load(
        cwd: dir.path, host: nil, base: .head, runner: .live)
    #expect(result.patch.isEmpty)
    #expect(result.baseRef == "HEAD")

    try Data("working change\n".utf8).write(to: dir.appendingPathComponent("tracked.txt"))
    result = try await WorktreeDiffLoader.load(
        cwd: dir.path, host: nil, base: .head, runner: .live)
    let patch = String(decoding: result.patch, as: UTF8.self)
    #expect(patch.contains("+working change"))
    #expect(!patch.contains("-main"))
}

@Test func worktreeDiffPayloadIsCappedBeforeControlEncoding() async throws {
    let oversized = String(repeating: "x", count: WorktreeDiffLoader.maxPatchBytes + 1)
    let runner = ShellRunner { _, _, _ in
        ShellResult(exitCode: 0, stdout: "/srv/app\0main\0\(oversized)")
    }

    let result = try await WorktreeDiffLoader.load(
        cwd: "/srv/app", host: nil, runner: runner)

    #expect(result.patch.count == WorktreeDiffLoader.maxPatchBytes)
    #expect(result.truncated)
    let encoded = try JSONEncoder.dirijor.encode(result)
    #expect(encoded.count < NDJSONBuffer.maxLineBytes)
}

private func runGit(_ arguments: [String], cwd: URL) throws {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/git")
    process.arguments = arguments
    process.currentDirectoryURL = cwd
    try process.run()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else {
        throw ControlError.internalError(
            "git \(arguments.joined(separator: " ")) exited \(process.terminationStatus)")
    }
}
