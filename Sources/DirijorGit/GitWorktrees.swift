import DirijorCore
import Foundation

public struct GitError: Error, Sendable, CustomStringConvertible {
    public var message: String
    public var exitCode: Int32
    public init(message: String, exitCode: Int32) {
        self.message = message
        self.exitCode = exitCode
    }
    public var description: String { "git failed (\(exitCode)): \(message)" }
}

/// Shell-out wrapper around `git worktree`. The daemon calls these; all are
/// synchronous and safe to run off the main actor.
public enum GitWorktrees {
    /// True if `path` is inside a git repository.
    public static func isRepository(_ path: String) -> Bool {
        (try? run(["rev-parse", "--is-inside-work-tree"], in: path)) == "true"
    }

    public static func currentBranch(in path: String) -> String? {
        try? run(["rev-parse", "--abbrev-ref", "HEAD"], in: path)
    }

    /// Repo root (main worktree) for a path inside any worktree.
    public static func repositoryRoot(of path: String) -> String? {
        try? run(["rev-parse", "--show-toplevel"], in: path)
    }

    public static func list(repoPath: String) throws -> [WorktreeInfo] {
        let output = try run(["worktree", "list", "--porcelain"], in: repoPath)
        return parsePorcelainList(output)
    }

    /// Creates a worktree. When `branch` is nil a friendly name is generated.
    /// Returns the created worktree info.
    public static func create(
        repoPath: String, branch: String? = nil, base: String? = nil
    ) throws -> WorktreeInfo {
        let branchName = branch ?? generatedBranchName()
        let slug = branchToPathSlug(branchName)
        let parent = (repoPath as NSString).deletingLastPathComponent
        let repoName = (repoPath as NSString).lastPathComponent
        let worktreePath = "\(parent)/\(repoName)-\(slug)"
        var args = ["worktree", "add", "-b", branchName, worktreePath]
        if let base { args.append(base) }
        _ = try run(args, in: repoPath)
        return WorktreeInfo(path: worktreePath, branch: branchName)
    }

    public static func remove(repoPath: String, worktreePath: String, force: Bool = false) throws {
        var args = ["worktree", "remove"]
        if force { args.append("--force") }
        args.append(worktreePath)
        _ = try run(args, in: repoPath)
    }

    // MARK: Implemented in GitWorktreesImpl.swift

    static func run(_ args: [String], in directory: String) throws -> String {
        try runImpl(args, in: directory)
    }
    static func parsePorcelainList(_ porcelain: String) -> [WorktreeInfo] {
        parsePorcelainListImpl(porcelain)
    }
    /// e.g. "dirijor/brisk-otter-3f2a"
    public static func generatedBranchName() -> String {
        generatedBranchNameImpl()
    }
    public static func branchToPathSlug(_ branch: String) -> String {
        branchToPathSlugImpl(branch)
    }
}
