import DirijorProtocol
import Foundation

/// Finds checkouts of "the same repository" across machines by matching
/// `git remote get-url origin` — the shared engine behind `host.locate_repo`,
/// repo-preserving spawns (`SessionSpawnParams.sameRepoAs`), and the target
/// half of `session.migrate`.
///
/// Remote lookups scan one directory level under the host's `defaultCwd`
/// (`~/code/*/`), the documented default layout for remote clones. Results are cached
/// (origin+host → path) so the new-agent picker feels instant; misses use a
/// short TTL so "clone it, click again" works without a daemon restart.
actor RepoLocator {
    private let runner: ShellRunner

    private struct CacheEntry {
        var path: String?
        var expires: ContinuousClock.Instant
    }
    /// (hostKey|normalizedOrigin) → checkout path.
    private var locations: [String: CacheEntry] = [:]
    /// (hostKey|cwd) → origin URL of the repo containing cwd.
    private var origins: [String: (origin: String?, expires: ContinuousClock.Instant)] = [:]

    static let hitTTL: Duration = .seconds(300)
    static let missTTL: Duration = .seconds(15)

    init(runner: ShellRunner) {
        self.runner = runner
    }

    private static func hostKey(_ host: HostEntry?) -> String {
        host?.id ?? "local"
    }

    // MARK: Origin-URL normalization

    /// Canonicalizes an origin URL so the ssh and https spellings of one repo
    /// compare equal: `git@github.com:org/x.git` == `https://github.com/org/x`.
    static func normalizeGitURL(_ url: String) -> String {
        var s = url.trimmingCharacters(in: .whitespacesAndNewlines)
        var hadScheme = false
        for prefix in ["ssh://", "git://", "https://", "http://", "file://"]
        where s.lowercased().hasPrefix(prefix) {
            s = String(s.dropFirst(prefix.count))
            hadScheme = true
        }
        // scp-like syntax (only without a scheme — after one, `host:2222` is
        // a port): [user@]host:path → host/path.
        if !hadScheme,
            let colon = s.firstIndex(of: ":"),
            !s[..<colon].contains("/")
        {
            s = s[..<colon] + "/" + s[s.index(after: colon)...]
        }
        // Drop user@ and an explicit :port from the host component.
        if let at = s.firstIndex(of: "@"), !s[..<at].contains("/") {
            s = String(s[s.index(after: at)...])
        }
        if let slash = s.firstIndex(of: "/"), slash != s.startIndex {
            var hostPart = String(s[..<slash])
            if let portColon = hostPart.firstIndex(of: ":") {
                hostPart = String(hostPart[..<portColon])
            }
            s = hostPart + String(s[slash...])
        }
        while s.hasSuffix("/") { s.removeLast() }
        if s.lowercased().hasSuffix(".git") { s.removeLast(4) }
        return s.lowercased()
    }

    // MARK: Origin of a checkout

    /// The origin URL of the repository containing `cwd` on `host` (nil host =
    /// local). nil when cwd isn't in a git repo or the repo has no origin.
    func origin(ofCwd cwd: String, host: HostEntry?) async -> String? {
        let key = "\(Self.hostKey(host))|\(cwd)"
        if let cached = origins[key], cached.expires > .now {
            return cached.origin
        }
        let shell = HostShell(runner: runner, host: host)
        let quoted = InjectionBuilder.shellQuotePath(cwd)
        let result = try? await shell.run(
            "cd \(quoted) && git remote get-url origin", timeout: .seconds(20))
        let origin = (result?.ok == true) ? result?.trimmedStdout : nil
        let ttl = origin == nil ? Self.missTTL : Self.hitTTL
        origins[key] = (origin: origin?.isEmpty == true ? nil : origin, expires: .now + ttl)
        return origins[key]?.origin
    }

    // MARK: Locating a checkout by origin

    /// Absolute path of a checkout with the given origin on `host`, or nil.
    /// Local searches walk `localRoots` (the daemon's known project roots);
    /// remote searches scan `defaultCwd/*/` on the host.
    func locate(
        origin: String, on host: HostEntry?, localRoots: [String]
    ) async -> String? {
        let normalized = Self.normalizeGitURL(origin)
        let key = "\(Self.hostKey(host))|\(normalized)"
        if let cached = locations[key], cached.expires > .now {
            return cached.path
        }
        let path: String?
        if let host {
            path = await locateRemote(normalizedOrigin: normalized, host: host)
        } else {
            path = await locateLocal(normalizedOrigin: normalized, roots: localRoots)
        }
        let ttl = path == nil ? Self.missTTL : Self.hitTTL
        locations[key] = CacheEntry(path: path, expires: .now + ttl)
        return path
    }

    /// Drops a cached location (used after a migration reveals a stale path).
    func invalidate(origin: String, host: HostEntry?) {
        locations.removeValue(
            forKey: "\(Self.hostKey(host))|\(Self.normalizeGitURL(origin))")
    }

    /// One `path<TAB>origin` line per repo directly under the host's
    /// defaultCwd. `cd && pwd` yields the ABSOLUTE path (needed for transcript
    /// slugs) even when defaultCwd is `~`-relative.
    static func remoteListCommand(defaultCwd: String) -> String {
        let root = InjectionBuilder.shellQuotePath(defaultCwd)
        return """
            for d in \(root)/*/; do \
            [ -e "$d/.git" ] || continue; \
            printf '%s\t%s\n' "$(cd "$d" && pwd)" \
            "$(git -C "$d" remote get-url origin 2>/dev/null)"; \
            done
            """
    }

    /// Parses `remoteListCommand` output into (absolute path, origin) pairs.
    static func parseRepoList(_ output: String) -> [(path: String, origin: String)] {
        output.split(separator: "\n").compactMap { line in
            let parts = line.split(separator: "\t", maxSplits: 1)
            guard parts.count == 2, !parts[1].isEmpty else { return nil }
            return (path: String(parts[0]), origin: String(parts[1]))
        }
    }

    private func locateRemote(normalizedOrigin: String, host: HostEntry) async -> String? {
        let shell = HostShell(runner: runner, host: host)
        guard
            let result = try? await shell.run(
                Self.remoteListCommand(defaultCwd: host.defaultCwd ?? "~"),
                timeout: .seconds(20)),
            result.ok
        else { return nil }
        return Self.parseRepoList(result.stdout)
            .first { Self.normalizeGitURL($0.origin) == normalizedOrigin }?
            .path
    }

    private func locateLocal(normalizedOrigin: String, roots: [String]) async -> String? {
        let shell = HostShell(runner: runner, host: nil)
        for root in roots where FileManager.default.fileExists(atPath: root) {
            let quoted = InjectionBuilder.shellQuote(root)
            guard
                let result = try? await shell.run(
                    "git -C \(quoted) remote get-url origin", timeout: .seconds(10)),
                result.ok,
                Self.normalizeGitURL(result.trimmedStdout) == normalizedOrigin
            else { continue }
            return root
        }
        return nil
    }
}
