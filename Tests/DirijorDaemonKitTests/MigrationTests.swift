import DirijorCore
import DirijorProtocol
import Foundation
import Testing

@testable import DirijorDaemonKit

// MARK: - Slug derivation (verified against real ~/.claude/projects dirs)

@Test func claudeProjectSlugReplacesSlashesAndDotsWithDashes() {
    #expect(
        InjectionBuilder.claudeProjectSlug(cwd: "/Users/giga/fun/anara")
            == "-Users-giga-fun-anara")
    // "." also becomes "-", so hidden dirs produce a double dash — observed:
    // /Users/giga/.claude/worktrees/x → -Users-giga--claude-worktrees-x
    #expect(
        InjectionBuilder.claudeProjectSlug(cwd: "/Users/giga/.claude/worktrees/blindnav")
            == "-Users-giga--claude-worktrees-blindnav")
    #expect(
        InjectionBuilder.claudeProjectSlug(cwd: "/home/cristi/code/app.v2")
            == "-home-cristi-code-app-v2")
    // The transcript path builder uses the same rule.
    #expect(
        InjectionBuilder.claudeTranscriptPath(cwd: "/a/b", sessionUUID: "u")
            .hasSuffix("/.claude/projects/-a-b/u.jsonl"))
}

// MARK: - Origin URL normalization

@Test func gitURLNormalizationEquatesSshAndHttpsSpellings() {
    let canonical = "github.com/org/x"
    for spelling in [
        "git@github.com:org/x.git",
        "https://github.com/org/x",
        "https://github.com/org/x/",
        "https://github.com/org/x.git",
        "ssh://git@github.com/org/x.git",
        "ssh://git@github.com:2222/org/x.git",
        "HTTPS://GitHub.com/org/x.GIT",
    ] {
        #expect(RepoLocator.normalizeGitURL(spelling) == canonical, "\(spelling)")
    }
    // Distinct repos stay distinct.
    #expect(
        RepoLocator.normalizeGitURL("git@github.com:org/x.git")
            != RepoLocator.normalizeGitURL("git@github.com:org/y.git"))
    // file:// fixtures (integration tests) survive too.
    #expect(
        RepoLocator.normalizeGitURL("file:///tmp/origin.git")
            == RepoLocator.normalizeGitURL("/tmp/origin.git/"))
}

@Test func remoteRepoListingParsesPathTabOriginLines() {
    let output = """
        /home/cristi/code/anara\tgit@github.com:anara/anara.git
        /home/cristi/code/scratch\t
        /home/cristi/code/dirijor\thttps://github.com/giga/dirijor
        """
    let repos = RepoLocator.parseRepoList(output)
    #expect(repos.count == 2)
    #expect(repos[0].path == "/home/cristi/code/anara")
    #expect(repos[1].origin == "https://github.com/giga/dirijor")
    // The listing command scans one level under defaultCwd and prints
    // ABSOLUTE paths (needed for transcript slugs).
    let command = RepoLocator.remoteListCommand(defaultCwd: "~/code")
    #expect(command.contains("~/'code'/*/"))
    #expect(command.contains("cd \"$d\" && pwd"))
}

// MARK: - Transfer argv

@Test func copyArgvUsesCpLocallyAndScpAcrossHosts() {
    let forge = HostEntry(id: "forge", ssh: "cristi@forge")
    let vps2 = HostEntry(id: "vps2", ssh: "root@vps2")
    let migrator = SessionMigrator(runner: .live)

    #expect(
        migrator.copyArgv(from: "/a/x", fromHost: nil, to: "/b/x", toHost: nil)
            == ["/bin/cp", "/a/x", "/b/x"])

    let push = migrator.copyArgv(
        from: "/a/x", fromHost: nil, to: ".claude/projects/-p/u.jsonl", toHost: forge)
    #expect(push.first == "scp")
    #expect(push.last == "cristi@forge:.claude/projects/-p/u.jsonl")
    #expect(push.contains { $0.contains("ConnectTimeout=10") })

    let pull = migrator.copyArgv(
        from: ".claude/projects/-p/u.jsonl", fromHost: forge, to: "/b/x", toHost: nil)
    #expect(pull.contains("cristi@forge:.claude/projects/-p/u.jsonl"))
    #expect(pull.last == "/b/x")

    // Remote→remote routes through the daemon (-3); the hosts never need to
    // reach each other.
    let across = migrator.copyArgv(from: "/a", fromHost: forge, to: "/b", toHost: vps2)
    #expect(across.contains("-3"))
}

// MARK: - Locator caching

@Test func locatorCachesRemoteLookupsPerOriginAndHost() async {
    let forge = HostEntry(id: "forge", ssh: "cristi@forge", defaultCwd: "~/code")
    let recorder = CommandRecorder()
    let runner = ShellRunner { argv, _, _ in
        recorder.append(argv)
        let command = argv.joined(separator: " ")
        if command.contains("for d in") {
            return ShellResult(
                exitCode: 0,
                stdout: "/home/cristi/code/app\tgit@github.com:org/app.git\n")
        }
        return ShellResult(exitCode: 1)
    }
    let locator = RepoLocator(runner: runner)
    let first = await locator.locate(
        origin: "https://github.com/org/app", on: forge, localRoots: [])
    #expect(first == "/home/cristi/code/app")
    let second = await locator.locate(
        origin: "git@github.com:org/app.git", on: forge, localRoots: [])
    #expect(second == "/home/cristi/code/app")
    // Both spellings normalize to one cache key: a single ssh round trip.
    #expect(recorder.commands.count == 1)
}

// MARK: - WIP commit + target sync (real temp git repos, local "remote")

@discardableResult
private func sh(
    _ command: String, cwd: String? = nil,
    sourceLocation: SourceLocation = #_sourceLocation
) async throws -> ShellResult {
    let result = try await ShellRunner.live.run(["/bin/sh", "-c", command], cwd, .seconds(30))
    #expect(result.ok, "\(command) → \(result.stderr)", sourceLocation: sourceLocation)
    return result
}

/// origin.git (bare) + two clones with committed history, mimicking the Mac
/// checkout (source) and the VPS clone (target).
private func makeFixtureRepos() async throws -> (base: URL, source: String, target: String) {
    let base = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-migrate-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
    let path = base.path
    try await sh("git init --bare -b main origin.git", cwd: path)
    // Absolute origin URLs: the migrator's git commands don't run from `base`,
    // so a relative clone URL would not resolve.
    try await sh("git clone -q \"$PWD/origin.git\" source 2>/dev/null", cwd: path)
    try await sh("git clone -q \"$PWD/origin.git\" target 2>/dev/null", cwd: path)
    for clone in ["source", "target"] {
        try await sh(
            "git -C \(clone) config user.email t@t && git -C \(clone) config user.name T"
                + " && git -C \(clone) config commit.gpgsign false",
            cwd: path)
    }
    try await sh(
        "cd source && git checkout -q -B main && echo one > file.txt && git add -A "
            + "&& git commit -q -m init && git push -q -u origin main",
        cwd: path)
    try await sh(
        "git -C target fetch -q origin && git -C target checkout -q -B main origin/main",
        cwd: path)
    return (base, "\(path)/source", "\(path)/target")
}

@Test func migratorPrepareCommitsWipPushesAndHardSyncsTheTarget() async throws {
    let (base, source, target) = try await makeFixtureRepos()
    defer { try? FileManager.default.removeItem(at: base) }

    // Dirty tree: one edit, one untracked file.
    try Data("two".utf8).write(to: URL(fileURLWithPath: "\(source)/file.txt"))
    try Data("new".utf8).write(to: URL(fileURLWithPath: "\(source)/untracked.txt"))

    let migrator = SessionMigrator(runner: .live)
    let prepared = try await migrator.prepare(
        sourceCwd: source, sourceHost: nil, targetHost: nil,
        targetRepoRoot: target, targetName: "Forge")

    #expect(prepared.branch == "main")
    #expect(prepared.wipCommitted)
    #expect(URL(fileURLWithPath: prepared.sourceRepoRoot).lastPathComponent == "source")

    // Source tree is clean now; the WIP commit rode the CURRENT branch.
    let status = try await sh("git -C \(source) status --porcelain")
    #expect(status.trimmedStdout.isEmpty)
    let message = try await sh("git -C \(source) log -1 --format=%s")
    #expect(message.trimmedStdout.hasPrefix("WIP: handoff to Forge"))

    // Target is hard-synced to origin/main: same head, files present.
    let sourceHead = try await sh("git -C \(source) rev-parse HEAD").trimmedStdout
    let targetHead = try await sh("git -C \(target) rev-parse HEAD").trimmedStdout
    #expect(sourceHead == targetHead)
    #expect(FileManager.default.fileExists(atPath: "\(target)/untracked.txt"))

    // Idempotent: a second run is a no-op WIP-wise and still succeeds.
    let again = try await migrator.prepare(
        sourceCwd: source, sourceHost: nil, targetHost: nil,
        targetRepoRoot: target, targetName: "Forge")
    #expect(!again.wipCommitted)
}

@Test func migratorPrepareGivesLinkedWorktreesTheirOwnTargetWorktree() async throws {
    let (base, source, target) = try await makeFixtureRepos()
    defer { try? FileManager.default.removeItem(at: base) }

    // Source: a linked worktree on its own branch, with dirty state — the
    // shape of a parallel agent checkout.
    try await sh("git -C \(source) worktree add -b agent/topic ../source-agent-topic main")
    let sourceWorktree = "\(base.path)/source-agent-topic"
    try Data("wt".utf8).write(to: URL(fileURLWithPath: "\(sourceWorktree)/wt.txt"))

    let migrator = SessionMigrator(runner: .live)
    let prepared = try await migrator.prepare(
        sourceCwd: sourceWorktree, sourceHost: nil, targetHost: nil,
        targetRepoRoot: target, targetName: "Forge")

    // The target root is a NEW worktree next to the clone, not the clone.
    #expect(prepared.targetRepoRoot == "\(base.path)/target-agent-topic")
    #expect(prepared.branch == "agent/topic")
    let branch = try await sh("git -C \(prepared.targetRepoRoot) rev-parse --abbrev-ref HEAD")
    #expect(branch.trimmedStdout == "agent/topic")
    #expect(FileManager.default.fileExists(atPath: "\(prepared.targetRepoRoot)/wt.txt"))
    // The main clone's checkout was left alone.
    let mainBranch = try await sh("git -C \(target) rev-parse --abbrev-ref HEAD")
    #expect(mainBranch.trimmedStdout == "main")

    // Idempotent: rerunning syncs the existing worktree instead of failing.
    let again = try await migrator.prepare(
        sourceCwd: sourceWorktree, sourceHost: nil, targetHost: nil,
        targetRepoRoot: target, targetName: "Forge")
    #expect(again.targetRepoRoot == prepared.targetRepoRoot)
}

@Test func migratorPrepareRefusesDirtyTargetAndNonRepoSource() async throws {
    let (base, source, target) = try await makeFixtureRepos()
    defer { try? FileManager.default.removeItem(at: base) }
    let migrator = SessionMigrator(runner: .live)

    // Dirty TARGET tree → hard stop, nothing destroyed.
    try Data("precious".utf8).write(to: URL(fileURLWithPath: "\(target)/wip.txt"))
    await #expect {
        _ = try await migrator.prepare(
            sourceCwd: source, sourceHost: nil, targetHost: nil,
            targetRepoRoot: target, targetName: "local")
    } throws: { error in
        (error as? ControlError)?.message.contains("uncommitted changes") == true
    }
    #expect(FileManager.default.fileExists(atPath: "\(target)/wip.txt"))

    // Source outside any git repo → clear precondition error.
    let plain = base.appendingPathComponent("plain")
    try FileManager.default.createDirectory(at: plain, withIntermediateDirectories: true)
    await #expect {
        _ = try await migrator.prepare(
            sourceCwd: plain.path, sourceHost: nil, targetHost: nil,
            targetRepoRoot: target, targetName: "local")
    } throws: { error in
        (error as? ControlError)?.message.contains("not inside a git repository") == true
    }
}

// MARK: - Transcript shuttle

@Test func transcriptShuttleCopiesIntoTheTargetSlugDirAndKeepsTheSource() async throws {
    let base = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-shuttle-\(UUID().uuidString)")
    let home = base.appendingPathComponent("home")
    try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: base) }

    let uuid = UUID().uuidString.lowercased()
    let sourceTranscript = base.appendingPathComponent("\(uuid).jsonl")
    try Data("{\"line\":1}\n".utf8).write(to: sourceTranscript)

    let record = SessionRecord(
        id: SessionID.generate(), kind: .claudeCode, cwd: "/work/src",
        projectID: ProjectID(root: "/work/src"), title: "t",
        agentSessionID: uuid, transcriptPath: sourceTranscript.path)
    let prepared = SessionMigrator.Prepared(
        branch: "main", sourceRepoRoot: "/work/src",
        targetRepoRoot: "/work/target repo", wipCommitted: false)

    let migrator = SessionMigrator(runner: .live)
    let shuttle = await migrator.shuttleTranscript(
        record: record, sourceHost: nil, targetHost: nil, prepared: prepared, home: home)

    #expect(shuttle.migrated)
    let expected = home
        .appendingPathComponent(".claude/projects/-work-target repo/\(uuid).jsonl").path
    #expect(shuttle.localTargetPath == expected)
    #expect(FileManager.default.fileExists(atPath: expected))
    // The source copy is never deleted.
    #expect(FileManager.default.fileExists(atPath: sourceTranscript.path))
}

@Test func transcriptShuttleMissingSourceIsNonFatalWithAWarning() async {
    let record = SessionRecord(
        id: SessionID.generate(), kind: .claudeCode, cwd: "/nonexistent/src",
        projectID: ProjectID(root: "/nonexistent/src"), title: "t",
        agentSessionID: UUID().uuidString.lowercased())
    let prepared = SessionMigrator.Prepared(
        branch: "main", sourceRepoRoot: "/nonexistent/src",
        targetRepoRoot: "/nonexistent/target", wipCommitted: false)
    let migrator = SessionMigrator(runner: .live)
    let shuttle = await migrator.shuttleTranscript(
        record: record, sourceHost: nil, targetHost: nil, prepared: prepared)
    #expect(!shuttle.migrated)
    #expect(shuttle.warning?.contains("transcript not found") == true)
}

// MARK: - Registry preconditions + repo-preserving spawn (fake remote)

private func makeRegistry(
    runner: ShellRunner, hosts: [HostEntry]
) async throws -> (SessionRegistry, URL) {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-migreg-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    let hostsFile = dir.appendingPathComponent("hosts.json")
    try HostsConfig(hosts: hosts).save(to: hostsFile)
    let config = DaemonConfig(
        socketPath: dir.appendingPathComponent("d.sock").path,
        cliPath: "/usr/bin/true",
        injectDir: dir,
        logsDir: dir,
        stateFile: dir.appendingPathComponent("state.json"),
        hostsConfigFile: hostsFile)
    return (SessionRegistry(config: config, events: EventBus(), runner: runner), dir)
}

@Test func migratePreconditionErrorsAreClearControlErrors() async throws {
    // .invalid TLD: the spawned ssh PTYs die fast without touching the network;
    // the fake runner answers the daemon-side git/ssh probes.
    let forge = HostEntry(
        id: "forge", name: "Forge", ssh: "nobody@test.invalid", defaultCwd: "~/code")
    let runner = ShellRunner { argv, _, _ in
        let command = argv.joined(separator: " ")
        if command.contains("git remote get-url origin") {
            return ShellResult(exitCode: 0, stdout: "git@github.com:org/app.git\n")
        }
        if command.contains("for d in") {
            return ShellResult(exitCode: 0, stdout: "")  // nothing cloned anywhere
        }
        return ShellResult(exitCode: 1, stderr: "unexpected: \(command)")
    }
    let (registry, dir) = try await makeRegistry(runner: runner, hosts: [forge])
    defer { try? FileManager.default.removeItem(at: dir) }

    func expectBadRequest(
        _ target: String?, of sessionID: SessionID, contains fragment: String
    ) async {
        await #expect {
            _ = try await registry.migrate(sessionID: sessionID, targetHostID: target)
        } throws: { error in
            let control = error as? ControlError
            return control?.code == "bad_request"
                && control?.message.contains(fragment) == true
        }
    }

    // Non-Claude sessions are refused outright.
    let shell = try await registry.spawn(
        SessionSpawnParams(kind: .shell, cwd: "~/code", host: "forge"))
    await expectBadRequest(nil, of: shell.id, contains: "only Claude Code")

    let claude = try await registry.spawn(
        SessionSpawnParams(kind: .claudeCode, cwd: "~/code/app", host: "forge"))
    // No-op moves and unknown targets fail before any remote work.
    await expectBadRequest("forge", of: claude.id, contains: "already on forge")
    await expectBadRequest("missing", of: claude.id, contains: "unknown host")
    // Origin resolves, but no local project has a clone of the repo.
    await expectBadRequest(nil, of: claude.id, contains: "repo not cloned locally")
    // And an unknown session is a proper not-found.
    await #expect {
        _ = try await registry.migrate(sessionID: SessionID.generate(), targetHostID: nil)
    } throws: { error in
        (error as? ControlError)?.code == "not_found"
    }

    try await registry.kill(sessionID: shell.id)
    try await registry.kill(sessionID: claude.id)
}

@Test func sameRepoSpawnResolvesTheTargetCheckoutByOrigin() async throws {
    let forge = HostEntry(
        id: "forge", name: "Forge", ssh: "nobody@test.invalid", defaultCwd: "~/code")
    let runner = ShellRunner { argv, _, _ in
        let command = argv.joined(separator: " ")
        if command.contains("git remote get-url origin") {
            return ShellResult(exitCode: 0, stdout: "git@github.com:org/app.git\n")
        }
        if command.contains("for d in") {
            return ShellResult(
                exitCode: 0,
                stdout: "/home/nobody/code/app\thttps://github.com/org/app\n")
        }
        return ShellResult(exitCode: 1)
    }
    let (registry, dir) = try await makeRegistry(runner: runner, hosts: [forge])
    defer { try? FileManager.default.removeItem(at: dir) }

    let reference = try await registry.spawn(
        SessionSpawnParams(kind: .claudeCode, cwd: "~/code/app", host: "forge"))
    // Same repo, same host: the picker's ⌘T default lands in the clone path
    // resolved by origin, not the raw defaultCwd.
    let sibling = try await registry.spawn(
        SessionSpawnParams(kind: .shell, cwd: "", host: "forge", sameRepoAs: reference.id))
    #expect(sibling.cwd == "/home/nobody/code/app")

    // Remote reference → Local target with no local clone: the remote cwd is
    // useless as a local path, so the spawn falls back to home.
    let local = try await registry.spawn(
        SessionSpawnParams(kind: .shell, cwd: "~/code/app", sameRepoAs: reference.id))
    #expect(local.cwd == FileManager.default.homeDirectoryForCurrentUser.path)

    for id in [reference.id, sibling.id, local.id] {
        try await registry.kill(sessionID: id)
    }
}

// MARK: - Respawn argv after migration (both directions)

@Test func migratedRecordsResumeWithClaudeResumeInBothDirections() throws {
    let forge = HostEntry(id: "forge", ssh: "cristi@forge", defaultCwd: "~/code")
    let id = SessionID(rawValue: "s_9f8e7d6c5b4a")
    let uuid = "11111111-2222-3333-4444-555555555555"

    // Local → forge: the respawn is the existing remote revive plan.
    let remote = InjectionBuilder.remotePlan(
        kind: .claudeCode, sessionID: id, host: forge,
        remoteCwd: "/home/cristi/code/app",
        socketPath: "/tmp/d.sock", cliPath: "/opt/dirijor",
        agentSessionID: uuid, resume: true)
    // The remote command resumes claude and drops to a shell on exit.
    #expect(remote.argv.contains { $0.contains("claude --resume \(uuid)") && $0.contains("exec") })
    #expect(remote.argv.contains("diri-9f8e7d6c"))

    // Forge → local: the plain local resume argv.
    let record = SessionRecord(
        id: id, kind: .claudeCode, cwd: "/Users/giga/fun/app",
        projectID: ProjectID(root: "/Users/giga/fun/app"), title: "t",
        agentSessionID: uuid)
    // Claude's manifest sets returnToLoginShell, so the resume argv is the
    // login shell wrapping a quoted command line (same shape as a fresh spawn)
    // rather than bare argv.
    let local = InjectionBuilder.resumeArgv(
        record: record, injectDir: URL(fileURLWithPath: "/nonexistent"))
    let localCommand = try #require(local?.last)
    #expect(localCommand.contains("'--resume'"))
    #expect(localCommand.contains(uuid))
}
