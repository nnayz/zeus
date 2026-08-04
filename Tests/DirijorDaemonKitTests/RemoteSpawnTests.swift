import DirijorCore
import DirijorProtocol
import Foundation
import Testing

@testable import DirijorDaemonKit

private let forge = HostEntry(
    id: "forge", name: "Forge", ssh: "cristi@forge", defaultCwd: "~/code")

/// Claude drops to a login shell on exit so the tmux window outlives the agent
/// (`InjectionBuilder.remoteAgentCommand`). Only claude carries this suffix.
private let dropToShell = #"; exec "${SHELL:-bash}" -l"#

/// The tmux portion of a remote argv, anchored on the `tmux` word rather than
/// an absolute offset.
///
/// The ssh preamble grows over time — keepalive options were added to it — and
/// hardcoded indices do not fail loudly when it does: they slide onto a
/// neighbouring argument and the assertion compares the wrong thing.
/// Layout from `tmux`: 0 tmux, 1 new-session, 2 -A, 3 -s, 4 name, 5 -c, 6 cwd,
/// 7 the agent command (absent for `.shell`).
private func tmuxArgs(_ argv: [String]) -> [String] {
    Array(argv.drop(while: { $0 != "tmux" }))
}

private func remoteCwdArg(_ argv: [String]) -> String { tmuxArgs(argv)[6] }
private func remoteCommandArg(_ argv: [String]) -> String { tmuxArgs(argv)[7] }

@Test func remoteTmuxSessionNameDerivesFromSessionID() {
    #expect(
        InjectionBuilder.remoteTmuxSessionName(
            sessionID: SessionID(rawValue: "s_9f8e7d6c5b4a")) == "diri-9f8e7d6c")
    // Ids without the s_ prefix still yield a stable 8-char suffix.
    #expect(
        InjectionBuilder.remoteTmuxSessionName(
            sessionID: SessionID(rawValue: "abcdef1234")) == "diri-abcdef12")
}

@Test func remoteClaudeArgvIsSshTmuxWithPlainAgentCommand() {
    let argv = InjectionBuilder.remoteArgv(
        kind: .claudeCode,
        sessionID: SessionID(rawValue: "s_9f8e7d6c5b4a"),
        host: forge,
        remoteCwd: "~/code",
        agentSessionID: "11111111-2222-3333-4444-555555555555"
    )
    // Pins the whole shape, keepalives included: this is the canonical argv.
    #expect(
        argv == [
            "ssh", "-t", "-o", "StrictHostKeyChecking=accept-new",
            "-o", "ServerAliveInterval=20", "-o", "ServerAliveCountMax=3",
            "-o", "TCPKeepAlive=yes",
            "cristi@forge", "--",
            "tmux", "new-session", "-A", "-s", "diri-9f8e7d6c",
            "-c", "~/'code'",
            "'claude --session-id 11111111-2222-3333-4444-555555555555\(dropToShell)'",
            "\\;", "set", "status", "off",
        ])
    // No local-path injection may ever cross the SSH boundary.
    #expect(!argv.joined(separator: " ").contains("--settings"))
    #expect(!argv.joined(separator: " ").contains("--mcp-config"))
}

@Test func remoteArgvPerKindAndResume() {
    let id = SessionID(rawValue: "s_0011223344ff")

    func command(_ kind: AgentKind, agentID: String? = nil, resume: Bool = false) -> [String] {
        InjectionBuilder.remoteArgv(
            kind: kind, sessionID: id, host: forge, remoteCwd: "~/code",
            agentSessionID: agentID, resume: resume)
    }

    // Shell: no command argument — tmux starts the remote login shell, so the
    // tmux portion ends right after the cwd.
    let shell = command(.shell)
    #expect(Array(tmuxArgs(shell).dropFirst(7)) == ["\\;", "set", "status", "off"])

    // Drop-to-shell-on-exit is now a manifest property (`returnToLoginShell`)
    // applied uniformly to local spawn, local resume, and remote — it used to be
    // spelled out per call site and claude was the only remote kind that got it.
    // Codex needs it most: its self-updater exits the CLI and asks for a
    // restart, which without a parent shell ends the whole tmux window.
    #expect(remoteCommandArg(command(.codex)) == "'codex\(dropToShell)'")
    // Cursor and Gemini declare returnToLoginShell: false, so they stay bare.
    #expect(remoteCommandArg(command(.cursor)) == "'cursor-agent'")
    #expect(
        remoteCommandArg(command(.gemini, agentID: "u-u-i-d")) == "'gemini --session-id u-u-i-d'")
    #expect(
        remoteCommandArg(command(.gemini, agentID: "u-u-i-d", resume: true))
            == "'gemini --resume u-u-i-d'")
    #expect(
        remoteCommandArg(command(.claudeCode, agentID: "u-u-i-d", resume: true))
            == "'claude --resume u-u-i-d\(dropToShell)'")
    // Generic commands with embedded quotes survive POSIX single-quoting.
    #expect(
        remoteCommandArg(command(.generic(command: "echo 'hi there'")))
            == #"'echo '\''hi there'\'''"#)
}

@Test func remoteCwdQuotingPreservesTildeExpansion() {
    func cwdArg(_ cwd: String) -> String {
        remoteCwdArg(
            InjectionBuilder.remoteArgv(
                kind: .shell, sessionID: SessionID(rawValue: "s_1"), host: forge,
                remoteCwd: cwd, agentSessionID: nil))
    }
    #expect(cwdArg("~") == "~")
    #expect(cwdArg("~/code") == "~/'code'")
    #expect(cwdArg("~/my code/x") == "~/'my code/x'")
    #expect(cwdArg("/srv/deploy") == "'/srv/deploy'")
}

@Test func remotePlanMintsAgentIDAndSkipsLocalTranscript() {
    let plan = InjectionBuilder.remotePlan(
        kind: .claudeCode,
        sessionID: SessionID(rawValue: "s_9f8e7d6c5b4a"),
        host: forge,
        remoteCwd: "~/code",
        socketPath: "/tmp/d.sock",
        cliPath: "/opt/dirijor"
    )
    let uuid = try! #require(plan.agentSessionID)
    #expect(UUID(uuidString: uuid) != nil)
    // Transcript lives on the VPS — nothing local to record/watch.
    #expect(plan.transcriptPath == nil)
    #expect(plan.argv[0].hasSuffix("ssh"))
    #expect(plan.argv.contains("'claude --session-id \(uuid)\(dropToShell)'"))
    #expect(plan.extraEnv[DirijorEnv.sessionID] == "s_9f8e7d6c5b4a")

    // Resume reuses the persisted conversation id instead of minting one.
    let revived = InjectionBuilder.remotePlan(
        kind: .claudeCode,
        sessionID: SessionID(rawValue: "s_9f8e7d6c5b4a"),
        host: forge,
        remoteCwd: "~/code",
        socketPath: "/tmp/d.sock",
        cliPath: "/opt/dirijor",
        agentSessionID: uuid,
        resume: true
    )
    #expect(revived.agentSessionID == uuid)
    #expect(revived.argv.contains("'claude --resume \(uuid)\(dropToShell)'"))
    // Identical tmux target ⇒ new-session -A reattaches the survivor.
    #expect(revived.argv.contains("diri-9f8e7d6c"))
}

@Test func remoteSshCommandLineParsesBackToTmuxArgv() {
    // ssh joins argv with spaces and the remote shell re-tokenizes; simulate
    // that parse to prove the tmux command arrives as intended.
    let argv = InjectionBuilder.remoteArgv(
        kind: .claudeCode,
        sessionID: SessionID(rawValue: "s_9f8e7d6c5b4a"),
        host: forge,
        remoteCwd: "~/my code",
        agentSessionID: "abc"
    )
    let remoteCommandLine = tmuxArgs(argv).joined(separator: " ")
    let words = posixShellSplit(remoteCommandLine)
    #expect(
        words == [
            "tmux", "new-session", "-A", "-s", "diri-9f8e7d6c",
            "-c", "~/my code",  // one word: tilde-prefixed, spaces quoted
            // A single tmux shell-command argument, exec suffix included.
            "claude --session-id abc\(dropToShell)",
            ";", "set", "status", "off",
        ])
}

@Test func registryRejectsWorktreeAndUnknownHostSpawns() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-remote-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }
    let hostsFile = dir.appendingPathComponent("hosts.json")
    try HostsConfig(hosts: [forge]).save(to: hostsFile)

    let config = DaemonConfig(
        socketPath: dir.appendingPathComponent("d.sock").path,
        cliPath: "/usr/bin/true",
        injectDir: dir,
        logsDir: dir,
        stateFile: dir.appendingPathComponent("state.json"),
        hostsConfigFile: hostsFile
    )
    let registry = SessionRegistry(config: config, events: EventBus())

    // Worktrees are a local-git feature; remote spawns must refuse them.
    await #expect {
        _ = try await registry.spawn(
            SessionSpawnParams(kind: .shell, cwd: "~/code", newWorktree: true, host: "forge"))
    } throws: { error in
        (error as? ControlError)?.code == "bad_request"
    }

    // A host id that is not in hosts.json is a proper control error.
    await #expect {
        _ = try await registry.spawn(
            SessionSpawnParams(kind: .shell, cwd: "~/code", host: "missing"))
    } throws: { error in
        (error as? ControlError)?.code == "bad_request"
    }

    // Nothing was recorded by the failed attempts.
    let list = await registry.list()
    #expect(list.sessions.isEmpty)
}

@Test func registryRemoteSpawnRecordsHostAndRemoteCwd() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-remote-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }
    let hostsFile = dir.appendingPathComponent("hosts.json")
    // .invalid TLD guarantees the ssh child fails fast without touching the
    // network; the registry-side bookkeeping is what's under test.
    let host = HostEntry(id: "forge", ssh: "nobody@test.invalid", defaultCwd: "~/code")
    try HostsConfig(hosts: [host]).save(to: hostsFile)

    let config = DaemonConfig(
        socketPath: dir.appendingPathComponent("d.sock").path,
        cliPath: "/usr/bin/true",
        injectDir: dir,
        logsDir: dir,
        stateFile: dir.appendingPathComponent("state.json"),
        hostsConfigFile: hostsFile
    )
    let registry = SessionRegistry(config: config, events: EventBus())

    let record = try await registry.spawn(
        SessionSpawnParams(kind: .claudeCode, cwd: "~/code/app", host: "forge"))
    #expect(record.host == "forge")
    // The record keeps the REMOTE cwd (UI truth), not the local ssh cwd.
    #expect(record.cwd == "~/code/app")
    // No local transcript for remote agents; conversation id is minted.
    #expect(record.transcriptPath == nil)
    #expect(record.agentSessionID != nil)
    #expect(record.resumability == .resumable)

    // An empty cwd falls back to the host's defaultCwd.
    let defaulted = try await registry.spawn(
        SessionSpawnParams(kind: .shell, cwd: "", host: "forge"))
    #expect(defaulted.cwd == "~/code")
    // Remote sessions are always revivable (tmux new-session -A reattaches).
    #expect(defaulted.resumability == .resumable)

    try await registry.kill(sessionID: record.id)
    try await registry.kill(sessionID: defaulted.id)
}

/// Minimal POSIX-shell word splitter (quotes + backslash) for the test above.
private func posixShellSplit(_ line: String) -> [String] {
    var words: [String] = []
    var current = ""
    var started = false
    var index = line.startIndex
    var inSingle = false
    while index < line.endIndex {
        let ch = line[index]
        if inSingle {
            if ch == "'" { inSingle = false } else { current.append(ch) }
        } else if ch == "'" {
            inSingle = true
            started = true
        } else if ch == "\\" {
            index = line.index(after: index)
            if index < line.endIndex {
                current.append(line[index])
                started = true
            }
        } else if ch == " " {
            if started || !current.isEmpty { words.append(current) }
            current = ""
            started = false
        } else {
            current.append(ch)
            started = true
        }
        index = line.index(after: index)
    }
    if started || !current.isEmpty { words.append(current) }
    return words
}
