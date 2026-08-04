import DirijorProtocol
import Foundation
import Testing

@testable import DirijorDaemonKit

private let forge = HostEntry(
    id: "forge", name: "Forge", ssh: "cristi@forge", defaultCwd: "~/code")

/// Thread-safe recorder for the argv lists a fake runner receives.
final class CommandRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [[String]] = []

    func append(_ argv: [String]) {
        lock.lock()
        storage.append(argv)
        lock.unlock()
    }

    var commands: [[String]] {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }
}

private func makeHome(claude: [String] = [], codex: [String] = []) throws -> URL {
    let home = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-prefs-\(UUID().uuidString)")
    for (dir, items) in [(".claude", claude), (".codex", codex)] {
        let base = home.appendingPathComponent(dir)
        try FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        for item in items {
            if item.hasSuffix("/") {
                try FileManager.default.createDirectory(
                    at: base.appendingPathComponent(String(item.dropLast())),
                    withIntermediateDirectories: true)
            } else {
                try Data("x".utf8).write(to: base.appendingPathComponent(item))
            }
        }
    }
    return home
}

@Test func prefsSyncPushesOnlyTheFixedIncludeListAndNeverCredentials() async throws {
    // Credentials and transcripts on disk right next to the config…
    let home = try makeHome(
        claude: [
            "CLAUDE.md", "settings.json", "commands/", "skills/",
            ".credentials.json", "projects/", "todos/",
        ],
        codex: ["config.toml", "auth.json"])
    defer { try? FileManager.default.removeItem(at: home) }

    let recorder = CommandRecorder()
    let runner = ShellRunner { argv, _, _ in
        recorder.append(argv)
        return ShellResult(exitCode: 0)
    }
    let result = await PrefsSync.run(host: forge, runner: runner, home: home)

    let claude = try #require(result.tools.first { $0.tool == "claude" })
    #expect(claude.ok)
    #expect(claude.synced == ["CLAUDE.md", "settings.json", "commands", "skills"])
    let codex = try #require(result.tools.first { $0.tool == "codex" })
    #expect(codex.ok)
    #expect(codex.synced == ["config.toml"])

    // …but no executed command ever references them, and rsync is additive.
    let flat = recorder.commands.map { $0.joined(separator: " ") }.joined(separator: "\n")
    #expect(!flat.contains("credentials"))
    #expect(!flat.contains("auth.json"))
    #expect(!flat.contains("projects"))
    #expect(!flat.contains("todos"))
    #expect(!flat.contains("--delete"))

    // mkdir-over-ssh before each rsync, both with the non-interactive options.
    let rsyncs = recorder.commands.filter { $0.first == "rsync" }
    #expect(rsyncs.count == 2)
    for rsync in rsyncs {
        #expect(rsync.contains("-a"))
        #expect(rsync.contains { $0.contains("ConnectTimeout=10") })
    }
    #expect(rsyncs[0].last == "cristi@forge:.claude/")
    #expect(rsyncs[1].last == "cristi@forge:.codex/")
    let mkdirs = recorder.commands.filter { $0.first == "ssh" }
    #expect(mkdirs.count == 2)
    #expect(mkdirs.allSatisfy { $0.contains("cristi@forge") && $0.contains("-o") })
}

@Test func prefsSyncSkipsToolsWithNothingLocalWithoutTouchingTheNetwork() async throws {
    let home = try makeHome(claude: ["CLAUDE.md"])  // no codex config at all
    defer { try? FileManager.default.removeItem(at: home) }
    // The empty .codex dir exists but contains no syncable item.
    let recorder = CommandRecorder()
    let runner = ShellRunner { argv, _, _ in
        recorder.append(argv)
        return ShellResult(exitCode: 0)
    }
    let result = await PrefsSync.run(host: forge, runner: runner, home: home)
    let codex = try #require(result.tools.first { $0.tool == "codex" })
    #expect(codex.ok)
    #expect(codex.synced.isEmpty)
    #expect(codex.error == nil)
    // Only claude's mkdir+rsync pair ran.
    #expect(recorder.commands.count == 2)
}

@Test func prefsSyncMapsMissingRemoteRsyncToAClearError() async throws {
    let home = try makeHome(claude: ["CLAUDE.md"])
    defer { try? FileManager.default.removeItem(at: home) }
    let runner = ShellRunner { argv, _, _ in
        if argv.first == "rsync" {
            return ShellResult(
                exitCode: 127,
                stderr: "bash: rsync: command not found\nrsync: connection unexpectedly closed")
        }
        return ShellResult(exitCode: 0)
    }
    let result = await PrefsSync.run(host: forge, runner: runner, home: home)
    let claude = try #require(result.tools.first { $0.tool == "claude" })
    #expect(!claude.ok)
    #expect(claude.synced.isEmpty)
    #expect(claude.error?.contains("rsync is not installed on Forge") == true)
}

@Test func prefsSyncReportsSshFailuresPerToolWithoutThrowing() async throws {
    let home = try makeHome(claude: ["CLAUDE.md"], codex: ["config.toml"])
    defer { try? FileManager.default.removeItem(at: home) }
    let runner = ShellRunner { argv, _, _ in
        if argv.first == "ssh" {
            return ShellResult(exitCode: 255, stderr: "ssh: connect to host forge: timed out")
        }
        return ShellResult(exitCode: 0)
    }
    let result = await PrefsSync.run(host: forge, runner: runner, home: home)
    #expect(result.tools.count == 2)
    for tool in result.tools {
        #expect(!tool.ok)
        #expect(tool.error?.contains("timed out") == true)
    }
}
