import ArgumentParser
import DirijorCore
import DirijorProtocol
import Foundation

/// `dirijor worktree <action> <repo> [options]` — the git-worktree half of the
/// automation surface, mirroring the MCP tools of the same names so a script
/// and an agent drive the daemon through identical semantics.
struct Worktree: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "worktree",
        abstract: "Create, list, and remove git worktrees through the daemon.",
        subcommands: [WorktreeList.self, WorktreeCreate.self, WorktreeRemove.self],
        defaultSubcommand: WorktreeList.self
    )
}

struct WorktreeList: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "list", abstract: "List a repository's worktrees.")

    @OptionGroup var output: OutputOptions

    @Argument(help: "Repository path (default: the current directory).")
    var repo: String?

    func run() throws {
        let repoPath = repo ?? FileManager.default.currentDirectoryPath
        let worktrees = try CLIClient.withConn { conn in
            try conn.request(Method.worktreeList, params: WorktreeListParams(repoPath: repoPath))
                .decoded(as: [WorktreeInfo].self)
        }
        if output.json {
            output.emit(.object([
                "repo": .string(repoPath),
                "worktrees": .array(worktrees.map { (try? JSONValue(encoding: $0)) ?? .null }),
            ]))
            return
        }
        guard !worktrees.isEmpty else {
            print("No worktrees for \(repoPath).")
            return
        }
        let branchWidth = max(6, worktrees.map { ($0.branch ?? "-").count }.max() ?? 6)
        for worktree in worktrees {
            var flags: [String] = []
            if worktree.isBare { flags.append("bare") }
            if worktree.isDetached { flags.append("detached") }
            if worktree.isPrunable { flags.append("prunable") }
            let suffix = flags.isEmpty ? "" : "  [\(flags.joined(separator: ","))]"
            print(padColumn(worktree.branch ?? "-", branchWidth) + "  " + worktree.path + suffix)
        }
    }
}

struct WorktreeCreate: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "create", abstract: "Create a worktree off a repository.")

    @OptionGroup var output: OutputOptions

    @Argument(help: "Repository path (default: the current directory).")
    var repo: String?

    @Option(name: .long, help: "Branch to create or check out.")
    var branch: String?

    @Option(name: .long, help: "Base revision for a new branch.")
    var base: String?

    func run() throws {
        let repoPath = repo ?? FileManager.default.currentDirectoryPath
        let params = WorktreeCreateParams(repoPath: repoPath, branch: branch, base: base)
        // `git worktree add` on a large repo comfortably outruns 3 seconds.
        let info = try CLIClient.withConn { conn in
            try conn.request(Method.worktreeCreate, params: params, readTimeout: 120)
                .decoded(as: WorktreeInfo.self)
        }
        if output.json {
            output.emit(encoding: info)
        } else {
            print("\(info.branch ?? "-")  \(info.path)")
        }
    }
}

struct WorktreeRemove: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "remove", abstract: "Remove a worktree.")

    @OptionGroup var output: OutputOptions

    @Argument(help: "Repository path.")
    var repo: String

    @Argument(help: "Worktree path to remove.")
    var path: String

    @Flag(name: .long, help: "Remove even when the worktree is dirty.")
    var force = false

    func run() throws {
        let params = WorktreeRemoveParams(repoPath: repo, worktreePath: path, force: force)
        _ = try CLIClient.withConn { conn in
            try conn.request(Method.worktreeRemove, params: params, readTimeout: 60)
        }
        if output.json {
            output.emit(.object(["ok": .bool(true), "path": .string(path)]))
        } else {
            print("removed \(path)")
        }
    }
}
