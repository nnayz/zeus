import DirijorCore
import DirijorProtocol
import Foundation

/// The git + transcript legwork of `session.migrate`. Both sides of the
/// handoff run through `HostShell`, so "source" and "target" can each be the
/// local machine or a remote host — tests exercise the whole flow with two
/// local directories and no ssh at all.
///
/// The registry owns orchestration (preconditions, kill, respawn); this type
/// owns the mechanical steps and is deliberately record-in, values-out.
struct SessionMigrator: Sendable {
    var runner: ShellRunner

    /// Everything decided before the point of no return (killing the source
    /// agent): the branch travelled, both repo roots, and whether a WIP
    /// commit was created.
    struct Prepared: Sendable {
        var branch: String
        /// Absolute repo root on the source machine.
        var sourceRepoRoot: String
        /// Absolute repo root on the target machine.
        var targetRepoRoot: String
        var wipCommitted: Bool
    }

    struct TranscriptShuttle: Sendable {
        var migrated: Bool
        /// For a LOCAL target: where the record's transcriptPath should point.
        var localTargetPath: String?
        var warning: String?
    }

    static func wipCommitMessage(targetName: String, date: Date = Date()) -> String {
        let stamp = ISO8601DateFormatter().string(from: date)
        return "WIP: handoff to \(targetName) \(stamp)"
    }

    // MARK: Phase 1 — code state (safe while the source agent is still alive)

    /// Source side: WIP-commit a dirty tree on the CURRENT branch and push it
    /// (setting upstream if needed; never force). Target side: refuse a dirty
    /// tree, then fetch + hard-sync the same branch. Every step is idempotent
    /// so a re-click after a failure retries cleanly.
    func prepare(
        sourceCwd: String,
        sourceHost: HostEntry?,
        targetHost: HostEntry?,
        targetRepoRoot: String,
        targetName: String
    ) async throws -> Prepared {
        let source = HostShell(runner: runner, host: sourceHost)
        let target = HostShell(runner: runner, host: targetHost)
        let cwd = InjectionBuilder.shellQuotePath(sourceCwd)

        let root = try await require(
            source, "cd \(cwd) && git rev-parse --show-toplevel",
            or: "session cwd is not inside a git repository: \(sourceCwd)")
        let rootQ = InjectionBuilder.shellQuote(root)

        let branch = try await require(
            source, "git -C \(rootQ) rev-parse --abbrev-ref HEAD",
            or: "could not determine the current branch in \(root)")
        guard branch != "HEAD" else {
            throw ControlError.badRequest(
                "cannot migrate a detached HEAD checkout — check out a branch first")
        }
        let branchQ = InjectionBuilder.shellQuote(branch)

        let status = try await run(source, "git -C \(rootQ) status --porcelain")
        var wipCommitted = false
        if !status.trimmedStdout.isEmpty {
            let message = Self.wipCommitMessage(targetName: targetName)
            _ = try await require(
                source,
                "git -C \(rootQ) add -A && git -C \(rootQ) commit -m \(InjectionBuilder.shellQuote(message))",
                or: "could not create the WIP handoff commit")
            wipCommitted = true
        }
        _ = try await require(
            source, "git -C \(rootQ) push -u origin \(branchQ)",
            or: "git push to origin failed", timeout: .seconds(120))

        // A linked source worktree gets its own worktree next to the target
        // clone (same "<repo>-<branch>" naming as local worktrees). Parallel
        // worktree agents would otherwise fight over the one clone's checkout.
        let absGitDir = try await require(
            source, "git -C \(rootQ) rev-parse --absolute-git-dir",
            or: "could not inspect the source checkout")
        let finalTargetRoot: String
        if absGitDir.contains("/.git/worktrees/") {
            finalTargetRoot = try await ensureTargetWorktree(
                target, mainClone: targetRepoRoot, branch: branch)
        } else {
            // Target checkout: never destroy work — a dirty tree is a hard stop.
            let targetQ = InjectionBuilder.shellQuote(targetRepoRoot)
            let targetStatus = try await require(
                target, "git -C \(targetQ) status --porcelain",
                or: "target checkout \(targetRepoRoot) is not a usable git repository")
            guard targetStatus.isEmpty else {
                throw ControlError.badRequest(
                    "target checkout \(targetRepoRoot) has uncommitted changes — commit or stash them there first"
                )
            }
            _ = try await require(
                target, "git -C \(targetQ) fetch origin \(branchQ)",
                or: "git fetch on the target failed", timeout: .seconds(120))
            // checkout -B <branch> origin/<branch> = create-or-reset + checkout:
            // the "checkout the branch, hard-reset to origin/<branch>" step in one
            // idempotent command (the tree was verified clean above).
            _ = try await require(
                target,
                "git -C \(targetQ) checkout -B \(branchQ) \(InjectionBuilder.shellQuote("origin/\(branch)"))",
                or: "could not check out \(branch) on the target")
            finalTargetRoot = targetRepoRoot
        }

        return Prepared(
            branch: branch,
            sourceRepoRoot: root,
            targetRepoRoot: finalTargetRoot,
            wipCommitted: wipCommitted)
    }

    /// Creates or re-syncs the dedicated worktree for `branch` next to the
    /// target's main clone and returns its path. Idempotent; a dirty existing
    /// worktree is a hard stop (never destroy work).
    private func ensureTargetWorktree(
        _ target: HostShell, mainClone: String, branch: String
    ) async throws -> String {
        let repoName = (mainClone as NSString).lastPathComponent
        let parent = (mainClone as NSString).deletingLastPathComponent
        let path = "\(parent)/\(repoName)-\(branch.replacingOccurrences(of: "/", with: "-"))"
        let pathQ = InjectionBuilder.shellQuote(path)
        let mainQ = InjectionBuilder.shellQuote(mainClone)
        let branchQ = InjectionBuilder.shellQuote(branch)
        let originRef = InjectionBuilder.shellQuote("origin/\(branch)")

        // Linked worktrees keep `.git` as a file, so probe with -e, not -d.
        let probe = try await run(target, "[ -e \(pathQ)/.git ] && echo yes || echo no")
        if probe.trimmedStdout == "yes" {
            let status = try await require(
                target, "git -C \(pathQ) status --porcelain",
                or: "target worktree \(path) is not a usable git checkout")
            guard status.isEmpty else {
                throw ControlError.badRequest(
                    "target worktree \(path) has uncommitted changes — commit or stash them there first"
                )
            }
            _ = try await require(
                target, "git -C \(pathQ) fetch origin \(branchQ)",
                or: "git fetch on the target failed", timeout: .seconds(120))
            _ = try await require(
                target,
                "git -C \(pathQ) checkout -B \(branchQ) \(originRef)",
                or: "could not check out \(branch) in \(path)")
        } else {
            _ = try await require(
                target, "git -C \(mainQ) fetch origin \(branchQ)",
                or: "git fetch on the target failed", timeout: .seconds(120))
            _ = try await require(
                target,
                "git -C \(mainQ) worktree add -B \(branchQ) \(pathQ) \(originRef)",
                or: "could not create worktree \(path) on the target (is \(branch) checked out elsewhere there?)",
                timeout: .seconds(120))
        }
        return path
    }

    // MARK: Phase 2 — transcript shuttle (source agent already stopped)

    /// Copies the Claude transcript jsonl from the source machine into the
    /// slug directory for the TARGET cwd on the target machine. Missing
    /// transcripts are non-fatal: the caller respawns a fresh conversation
    /// and the result says so. The source copy is never deleted.
    func shuttleTranscript(
        record: SessionRecord,
        sourceHost: HostEntry?,
        targetHost: HostEntry?,
        prepared: Prepared,
        home: URL = FileManager.default.homeDirectoryForCurrentUser
    ) async -> TranscriptShuttle {
        guard let uuid = record.agentSessionID else {
            return TranscriptShuttle(
                migrated: false, localTargetPath: nil,
                warning: "no conversation id recorded — starting a fresh conversation")
        }

        let sourcePath: String?
        if sourceHost == nil {
            sourcePath = localTranscript(record: record, uuid: uuid)
        } else {
            sourcePath = await remoteTranscript(
                host: sourceHost, sourceCwdAbs: prepared.sourceRepoRoot,
                recordCwd: record.cwd, uuid: uuid)
        }
        guard let sourcePath else {
            return TranscriptShuttle(
                migrated: false, localTargetPath: nil,
                warning:
                    "transcript not found on the source — code state moved, but the conversation restarts fresh"
            )
        }

        let slug = InjectionBuilder.claudeProjectSlug(cwd: prepared.targetRepoRoot)
        do {
            if let targetHost {
                let dir = ".claude/projects/\(slug)"
                let mkdir = try await HostShell(runner: runner, host: targetHost)
                    .run("mkdir -p \(InjectionBuilder.shellQuote(dir))")
                guard mkdir.ok else { throw shuttleError(mkdir) }
                let copy = try await copyFile(
                    from: sourcePath, fromHost: sourceHost,
                    to: "\(dir)/\(uuid).jsonl", toHost: targetHost)
                guard copy.ok else { throw shuttleError(copy) }
                return TranscriptShuttle(migrated: true, localTargetPath: nil, warning: nil)
            }
            let dir = home.appendingPathComponent(".claude/projects/\(slug)", isDirectory: true)
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let destination = dir.appendingPathComponent("\(uuid).jsonl").path
            let copy = try await copyFile(
                from: sourcePath, fromHost: sourceHost, to: destination, toHost: nil)
            guard copy.ok else { throw shuttleError(copy) }
            return TranscriptShuttle(migrated: true, localTargetPath: destination, warning: nil)
        } catch {
            return TranscriptShuttle(
                migrated: false, localTargetPath: nil,
                warning:
                    "transcript copy failed (\(error)) — code state moved, but the conversation restarts fresh"
            )
        }
    }

    /// Best-effort: end the source-side tmux session after a remote→local
    /// migration so the remote host doesn't keep a zombie agent. Idempotent (a
    /// missing session is success) and never fatal — a warning at most.
    func killRemoteTmux(host: HostEntry, sessionID: SessionID) async -> String? {
        let name = InjectionBuilder.remoteTmuxSessionName(sessionID: sessionID)
        let result = try? await HostShell(runner: runner, host: host)
            .run("tmux kill-session -t \(InjectionBuilder.shellQuote(name)) 2>/dev/null || true",
                 timeout: .seconds(15))
        if result?.ok != true {
            return "could not stop the tmux session on \(host.displayName) (\(name)) — it may still be running there"
        }
        return nil
    }

    // MARK: Helpers

    private func localTranscript(record: SessionRecord, uuid: String) -> String? {
        let fm = FileManager.default
        if let path = record.transcriptPath, fm.fileExists(atPath: path) { return path }
        let predicted = InjectionBuilder.claudeTranscriptPath(cwd: record.cwd, sessionUUID: uuid)
        if fm.fileExists(atPath: predicted) { return predicted }
        return InjectionBuilder.findClaudeTranscript(sessionUUID: uuid)
    }

    /// Finds the transcript on a remote source: the slug path for the repo
    /// root (and the recorded cwd), falling back to a scan of every project
    /// dir — Claude relocates transcripts when the agent enters a worktree.
    private func remoteTranscript(
        host: HostEntry?, sourceCwdAbs: String, recordCwd: String, uuid: String
    ) async -> String? {
        let candidates = [sourceCwdAbs, recordCwd].map {
            ".claude/projects/\(InjectionBuilder.claudeProjectSlug(cwd: $0))/\(uuid).jsonl"
        }
        let probes = candidates.map { "if [ -f \(InjectionBuilder.shellQuote($0)) ]; then echo \(InjectionBuilder.shellQuote($0)); exit 0; fi" }
            .joined(separator: "; ")
        let command =
            "\(probes); ls -1 \"$HOME\"/.claude/projects/*/\(uuid).jsonl 2>/dev/null | head -n1"
        guard
            let result = try? await HostShell(runner: runner, host: host)
                .run(command, timeout: .seconds(20)),
            result.ok, !result.trimmedStdout.isEmpty
        else { return nil }
        return result.trimmedStdout
    }

    /// cp locally, scp when either side is remote (`-3` routes remote→remote
    /// through the daemon so the two hosts never need to reach each other).
    func copyArgv(
        from: String, fromHost: HostEntry?, to: String, toHost: HostEntry?
    ) -> [String] {
        if fromHost == nil, toHost == nil {
            return ["/bin/cp", from, to]
        }
        let source = fromHost.map { "\($0.ssh):\(from)" } ?? from
        let destination = toHost.map { "\($0.ssh):\(to)" } ?? to
        var argv = ["scp"] + HostShell.sshOptions + ["-q"]
        if fromHost != nil, toHost != nil { argv.append("-3") }
        return argv + [source, destination]
    }

    private func copyFile(
        from: String, fromHost: HostEntry?, to: String, toHost: HostEntry?
    ) async throws -> ShellResult {
        try await runner.run(
            copyArgv(from: from, fromHost: fromHost, to: to, toHost: toHost),
            nil, .seconds(120))
    }

    private func shuttleError(_ result: ShellResult) -> ControlError {
        .internalError(
            result.stderr.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                ? "exit \(result.exitCode)"
                : result.stderr.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    private func run(
        _ shell: HostShell, _ command: String, timeout: Duration = .seconds(30)
    ) async throws -> ShellResult {
        do {
            return try await shell.run(command, timeout: timeout)
        } catch let error as ShellTimeoutError {
            throw ControlError.internalError("\(error)")
        }
    }

    /// Runs a command and maps any failure to a clear precondition error that
    /// includes the underlying stderr. Returns trimmed stdout.
    @discardableResult
    private func require(
        _ shell: HostShell, _ command: String, or message: String,
        timeout: Duration = .seconds(30)
    ) async throws -> String {
        let result = try await run(shell, command, timeout: timeout)
        guard result.ok else {
            let stderr = result.stderr.trimmingCharacters(in: .whitespacesAndNewlines)
            throw ControlError.badRequest(
                stderr.isEmpty ? message : "\(message): \(stderr)")
        }
        return result.trimmedStdout
    }
}
