import Foundation

/// One entry from `git worktree list --porcelain`.
public struct WorktreeInfo: Codable, Sendable, Hashable {
    public var path: String
    public var branch: String?
    public var isBare: Bool
    public var isDetached: Bool
    public var isPrunable: Bool

    public init(
        path: String,
        branch: String?,
        isBare: Bool = false,
        isDetached: Bool = false,
        isPrunable: Bool = false
    ) {
        self.path = path
        self.branch = branch
        self.isBare = isBare
        self.isDetached = isDetached
        self.isPrunable = isPrunable
    }
}
