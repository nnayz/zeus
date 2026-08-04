import Foundation

/// Reads the current branch by parsing `.git/HEAD` directly — no `git`
/// subprocess — so it's cheap enough to poll frequently for a live branch label.
/// Handles linked worktrees (the `.git` file indirection) and detached HEAD.
public enum GitHead {
    /// The current branch name for a working directory, a short SHA when the
    /// HEAD is detached, or nil when the directory isn't inside a git repo.
    public static func branch(inWorkingDir cwd: String) -> String? {
        guard let gitDir = gitDir(for: cwd),
            let head = try? String(contentsOfFile: gitDir + "/HEAD", encoding: .utf8)
        else { return nil }

        let trimmed = head.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasPrefix("ref: ") {
            let ref = String(trimmed.dropFirst(5))
            if let range = ref.range(of: "refs/heads/") {
                return String(ref[range.upperBound...])
            }
            return (ref as NSString).lastPathComponent
        }
        // Detached HEAD: a raw object id.
        return String(trimmed.prefix(8))
    }

    /// True when `cwd` lives inside a *linked* git worktree — its nearest `.git`
    /// is a file carrying `gitdir:` indirection — rather than the main checkout,
    /// whose `.git` is a directory. This is the signal that distinguishes Claude
    /// Code's own worktrees (`.claude/worktrees/<name>/`, created by `--worktree`
    /// or EnterWorktree) and any `git worktree add` tree from the primary working
    /// copy, without hard-coding a path convention.
    public static func isLinkedWorktree(_ cwd: String) -> Bool {
        let fm = FileManager.default
        var dir = URL(fileURLWithPath: cwd).standardizedFileURL
        while true {
            let dotGit = dir.appendingPathComponent(".git")
            var isDir: ObjCBool = false
            if fm.fileExists(atPath: dotGit.path, isDirectory: &isDir) {
                return !isDir.boolValue   // file ⇒ linked worktree; dir ⇒ main checkout
            }
            let parent = dir.deletingLastPathComponent()
            if parent.path == dir.path { return false }   // reached filesystem root
            dir = parent
        }
    }

    /// Resolves the directory that holds HEAD for `cwd`, walking up to find
    /// `.git` and following the worktree/submodule `gitdir:` indirection.
    private static func gitDir(for cwd: String) -> String? {
        let fm = FileManager.default
        var dir = URL(fileURLWithPath: cwd).standardizedFileURL

        while true {
            let dotGit = dir.appendingPathComponent(".git")
            var isDir: ObjCBool = false
            if fm.fileExists(atPath: dotGit.path, isDirectory: &isDir) {
                if isDir.boolValue { return dotGit.path }
                // `.git` is a file: "gitdir: <path>" — a linked worktree, whose
                // own HEAD lives at that path.
                guard let contents = try? String(contentsOfFile: dotGit.path, encoding: .utf8),
                    let line = contents.split(whereSeparator: \.isNewline).first,
                    line.hasPrefix("gitdir: ")
                else { return nil }
                let path = String(line.dropFirst("gitdir: ".count))
                    .trimmingCharacters(in: .whitespaces)
                if path.hasPrefix("/") { return path }
                return URL(fileURLWithPath: path, relativeTo: dir).standardizedFileURL.path
            }
            let parent = dir.deletingLastPathComponent()
            if parent.path == dir.path { return nil }   // reached filesystem root
            dir = parent
        }
    }
}
