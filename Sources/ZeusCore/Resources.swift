import Foundation

// MARK: - Hibernation

public enum HibernationReason: String, Codable, Hashable, Sendable {
    /// Idle past the governor's threshold with no attached client.
    case idle
    /// Frozen by the RAM watchdog (hard limit / global budget).
    case memoryPressure
    /// Explicit `session.hibernate` request.
    case manual
}

/// Wire-visible proof that a session's process tree is SIGSTOPped, with what
/// the daemon needs to CONT it later — or KILL it (start-time verified) after
/// a daemon restart.
public struct HibernationInfo: Codable, Hashable, Sendable {
    public var since: Date
    public var reason: HibernationReason
    /// Every pid in the frozen tree at freeze time.
    public var treePids: [Int32]
    /// Process start times keyed by pid — the pid-reuse guard for wake/cleanup.
    public var treeStartTimes: [Int32: Int64]?

    public init(
        since: Date,
        reason: HibernationReason,
        treePids: [Int32],
        treeStartTimes: [Int32: Int64]? = nil
    ) {
        self.since = since
        self.reason = reason
        self.treePids = treePids
        self.treeStartTimes = treeStartTimes
    }
}

// MARK: - Artifacts

public enum ArtifactKind: String, Codable, Hashable, Sendable {
    case pullRequest
    case linearIssue
    case preview
    case link
}

/// A URL captured from a session's screen (PR, Linear issue, preview, …).
public struct SessionArtifact: Codable, Hashable, Sendable {
    public var kind: ArtifactKind
    public var url: String
    public var firstSeenAt: Date

    public init(kind: ArtifactKind, url: String, firstSeenAt: Date) {
        self.kind = kind
        self.url = url
        self.firstSeenAt = firstSeenAt
    }
}

// MARK: - Pull requests

/// One CI check / commit status on a PR, bucketed into pass / fail / pending.
public struct PRCheck: Codable, Hashable, Sendable {
    /// "CI / build" — workflow-qualified when GitHub provides a workflow name.
    public var name: String
    /// "pass" / "fail" / "pending".
    public var result: String
    /// GitHub's raw verdict (SUCCESS / FAILURE / IN_PROGRESS / …).
    public var detail: String?
    /// Link to the check's log / details page.
    public var url: String?

    public init(name: String, result: String, detail: String? = nil, url: String? = nil) {
        self.name = name
        self.result = result
        self.detail = detail
        self.url = url
    }
}

/// One visible item in the pull request conversation. GitHub returns issue
/// comments and submitted reviews as separate collections; the daemon folds
/// both into this small, chronological wire shape so clients can render one
/// familiar review timeline without knowing GitHub's GraphQL object graph.
public struct PRDiscussionItem: Codable, Hashable, Sendable {
    /// "comment" or "review".
    public var kind: String
    public var author: String
    public var body: String
    /// APPROVED / CHANGES_REQUESTED / COMMENTED for submitted reviews.
    public var state: String?
    public var createdAt: Date?
    public var url: String?

    public init(
        kind: String,
        author: String,
        body: String,
        state: String? = nil,
        createdAt: Date? = nil,
        url: String? = nil
    ) {
        self.kind = kind
        self.author = author
        self.body = body
        self.state = state
        self.createdAt = createdAt
        self.url = url
    }
}

/// GitHub-side state of a PR captured as a session artifact, fetched by the
/// daemon via the `gh` CLI. Raw enum-ish fields keep GitHub's own uppercase
/// vocabulary (OPEN / MERGEABLE / CHANGES_REQUESTED …) so nothing is lost in
/// translation; `overall` is the derived one-word rollup UI and MCP show.
public struct PullRequestStatus: Codable, Hashable, Sendable {
    public var url: String
    public var number: Int
    public var title: String?
    public var author: String?
    public var body: String?
    public var baseRefName: String?
    public var headRefName: String?
    /// OPEN / MERGED / CLOSED.
    public var state: String
    public var isDraft: Bool
    /// APPROVED / CHANGES_REQUESTED / REVIEW_REQUIRED; nil when none required.
    public var reviewDecision: String?
    /// MERGEABLE / CONFLICTING / UNKNOWN.
    public var mergeable: String?
    /// CLEAN / BLOCKED / BEHIND / DIRTY / UNSTABLE / DRAFT / UNKNOWN.
    public var mergeStateStatus: String?
    public var additions: Int
    public var deletions: Int
    public var changedFiles: Int
    /// Issue-thread comments on the PR.
    public var commentCount: Int
    /// Submitted reviews (approvals, change requests, review comments).
    public var reviewCount: Int
    /// Resolved review threads (inline comment conversations), when known.
    public var resolvedThreads: Int?
    /// Total review threads, when known.
    public var totalThreads: Int?
    public var checksPassed: Int
    public var checksFailed: Int
    public var checksPending: Int
    /// The individual checks behind the counts, for detail UI.
    public var checks: [PRCheck]?
    /// Issue comments and submitted reviews, sorted oldest first.
    public var discussion: [PRDiscussionItem]?
    public var fetchedAt: Date

    public init(
        url: String,
        number: Int,
        title: String? = nil,
        author: String? = nil,
        body: String? = nil,
        baseRefName: String? = nil,
        headRefName: String? = nil,
        state: String,
        isDraft: Bool = false,
        reviewDecision: String? = nil,
        mergeable: String? = nil,
        mergeStateStatus: String? = nil,
        additions: Int = 0,
        deletions: Int = 0,
        changedFiles: Int = 0,
        commentCount: Int = 0,
        reviewCount: Int = 0,
        resolvedThreads: Int? = nil,
        totalThreads: Int? = nil,
        checksPassed: Int = 0,
        checksFailed: Int = 0,
        checksPending: Int = 0,
        checks: [PRCheck]? = nil,
        discussion: [PRDiscussionItem]? = nil,
        fetchedAt: Date
    ) {
        self.url = url
        self.number = number
        self.title = title
        self.author = author
        self.body = body
        self.baseRefName = baseRefName
        self.headRefName = headRefName
        self.state = state
        self.isDraft = isDraft
        self.reviewDecision = reviewDecision
        self.mergeable = mergeable
        self.mergeStateStatus = mergeStateStatus
        self.additions = additions
        self.deletions = deletions
        self.changedFiles = changedFiles
        self.commentCount = commentCount
        self.reviewCount = reviewCount
        self.resolvedThreads = resolvedThreads
        self.totalThreads = totalThreads
        self.checksPassed = checksPassed
        self.checksFailed = checksFailed
        self.checksPending = checksPending
        self.checks = checks
        self.discussion = discussion
        self.fetchedAt = fetchedAt
    }

    /// One-word rollup, worst blocker first: merged / closed / draft /
    /// conflicts / checks failing / changes requested / checks pending /
    /// needs review / blocked / ready.
    public var overall: String {
        if state == "MERGED" { return "merged" }
        if state == "CLOSED" { return "closed" }
        if isDraft { return "draft" }
        if mergeable == "CONFLICTING" { return "conflicts" }
        if checksFailed > 0 { return "checks failing" }
        if reviewDecision == "CHANGES_REQUESTED" { return "changes requested" }
        if checksPending > 0 { return "checks pending" }
        if reviewDecision == "REVIEW_REQUIRED" { return "needs review" }
        if mergeStateStatus == "BLOCKED" { return "blocked" }
        return "ready"
    }

    /// Equality ignoring `fetchedAt` — lets pollers skip publishing when a
    /// refetch changed nothing but the timestamp.
    public func sameStats(as other: PullRequestStatus) -> Bool {
        var a = self
        var b = other
        a.fetchedAt = .distantPast
        b.fetchedAt = .distantPast
        return a == b
    }
}

// MARK: - Ports

/// A TCP port some process in the session's tree is LISTENing on.
public struct PortInfo: Codable, Hashable, Sendable {
    public var port: Int
    public var processName: String

    public init(port: Int, processName: String) {
        self.port = port
        self.processName = processName
    }
}
