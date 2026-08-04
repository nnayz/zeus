import DirijorProtocol
import Foundation

/// Pushes the local user's agent preferences to a remote host so agents there
/// behave like local ones (`host.sync_prefs`). Swift port of
/// `infra/sync-agent-prefs.sh` — users don't have the repo checked out, so the
/// daemon owns the logic.
///
/// The include list is FIXED and additive (`rsync -a`, never `--delete`):
///   claude: CLAUDE.md, settings.json, keybindings.json, commands/, skills/, agents/
///   codex:  config.toml, AGENTS.md, prompts/
/// Excluded on purpose: `.credentials.json` / `auth.json` (each user logs in
/// with their own account on the box), `projects/` (transcripts + memory are
/// per-machine, path-slugged), todos, caches, statsig, shell snapshots.
public enum PrefsSync {
    public struct ToolSpec: Sendable {
        /// Report name ("claude" / "codex").
        public var name: String
        /// Local config directory (e.g. `~/.claude`).
        public var localDir: URL
        /// Remote directory relative to the remote $HOME (e.g. `.claude`).
        public var remoteDir: String
        /// The fixed include list. Anything not named here never syncs.
        public var items: [String]
    }

    public static func toolSpecs(
        home: URL = FileManager.default.homeDirectoryForCurrentUser
    ) -> [ToolSpec] {
        [
            ToolSpec(
                name: "claude",
                localDir: home.appendingPathComponent(".claude", isDirectory: true),
                remoteDir: ".claude",
                items: [
                    "CLAUDE.md", "settings.json", "keybindings.json",
                    "commands", "skills", "agents",
                ]),
            ToolSpec(
                name: "codex",
                localDir: home.appendingPathComponent(".codex", isDirectory: true),
                remoteDir: ".codex",
                items: ["config.toml", "AGENTS.md", "prompts"]),
        ]
    }

    /// Items from the spec's include list that exist locally right now.
    static func presentItems(
        of spec: ToolSpec, fileManager: FileManager = .default
    ) -> [String] {
        spec.items.filter {
            fileManager.fileExists(atPath: spec.localDir.appendingPathComponent($0).path)
        }
    }

    /// `mkdir -p <remoteDir>` over ssh (relative to the remote $HOME).
    static func mkdirArgv(host: HostEntry, spec: ToolSpec) -> [String] {
        ["ssh"] + HostShell.sshOptions
            + [host.ssh, "--", "mkdir -p \(InjectionBuilder.shellQuote(spec.remoteDir))"]
    }

    /// The additive rsync push. `-a` only — no `--delete`, ever — and the ssh
    /// transport carries the same non-interactive options as every other
    /// daemon-initiated remote command.
    static func rsyncArgv(host: HostEntry, spec: ToolSpec, present: [String]) -> [String] {
        let transport = (["ssh"] + HostShell.sshOptions).joined(separator: " ")
        return ["rsync", "-a", "--timeout=60", "-e", transport]
            + present.map { spec.localDir.appendingPathComponent($0).path }
            + ["\(host.ssh):\(spec.remoteDir)/"]
    }

    /// Runs the full sync for every tool. Never throws — each tool reports its
    /// own success/failure so one broken tool doesn't hide the other's result.
    public static func run(
        host: HostEntry,
        runner: ShellRunner,
        home: URL = FileManager.default.homeDirectoryForCurrentUser
    ) async -> HostSyncPrefsResult {
        var reports: [PrefsSyncToolReport] = []
        for spec in toolSpecs(home: home) {
            reports.append(await sync(spec: spec, host: host, runner: runner))
        }
        return HostSyncPrefsResult(tools: reports)
    }

    static func sync(
        spec: ToolSpec, host: HostEntry, runner: ShellRunner
    ) async -> PrefsSyncToolReport {
        let present = presentItems(of: spec)
        guard !present.isEmpty else {
            // Nothing local to push is a success, not an error (matches the
            // reference script's "claude: nothing to sync").
            return PrefsSyncToolReport(tool: spec.name, ok: true, synced: [])
        }
        do {
            let mkdir = try await runner.run(
                mkdirArgv(host: host, spec: spec), nil, .seconds(30))
            guard mkdir.ok else {
                return PrefsSyncToolReport(
                    tool: spec.name, ok: false, synced: [],
                    error: "ssh to \(host.displayName) failed: \(failureDetail(mkdir))")
            }
            let rsync = try await runner.run(
                rsyncArgv(host: host, spec: spec, present: present), nil, .seconds(120))
            guard rsync.ok else {
                return PrefsSyncToolReport(
                    tool: spec.name, ok: false, synced: [],
                    error: rsyncFailureMessage(rsync, host: host))
            }
            return PrefsSyncToolReport(tool: spec.name, ok: true, synced: present)
        } catch {
            return PrefsSyncToolReport(
                tool: spec.name, ok: false, synced: [], error: "\(error)")
        }
    }

    /// Maps an rsync failure to something actionable. The classic trap is a
    /// remote box without rsync installed: the remote shell prints "command
    /// not found" and rsync dies with a protocol error — say so plainly.
    static func rsyncFailureMessage(_ result: ShellResult, host: HostEntry) -> String {
        let stderr = result.stderr.trimmingCharacters(in: .whitespacesAndNewlines)
        if stderr.localizedCaseInsensitiveContains("command not found")
            || stderr.localizedCaseInsensitiveContains("rsync: not found")
            || result.exitCode == 127
        {
            return
                "rsync is not installed on \(host.displayName) — install it there (e.g. apt install rsync) and retry"
        }
        return "rsync failed (exit \(result.exitCode)): \(stderr)"
    }

    private static func failureDetail(_ result: ShellResult) -> String {
        let stderr = result.stderr.trimmingCharacters(in: .whitespacesAndNewlines)
        return stderr.isEmpty ? "exit \(result.exitCode)" : stderr
    }
}
