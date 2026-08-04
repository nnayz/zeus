import Foundation

public enum Resumability: String, Codable, Hashable, Sendable {
    /// Process is alive under the daemon right now.
    case live
    /// Process is gone but we have what we need to resume the conversation.
    case resumable
    /// We had a transcript reference but it no longer exists on disk.
    case transcriptMissing
    /// Nothing to resume (plain shells, generic agents).
    case notResumable
}

/// The persistent, wire-visible record of one session. This is what the sidebar,
/// menu bar, and iOS render; the daemon owns the authoritative copy.
public struct SessionRecord: Codable, Hashable, Sendable, Identifiable {
    public var id: SessionID
    public var kind: AgentKind
    public var cwd: String
    public var projectID: ProjectID
    public var worktreePath: String?
    public var gitBranch: String?

    public var title: String
    public var titleSource: TitleSource

    /// Agent-side id: Claude session UUID / Codex thread id. May rotate on resume.
    public var agentSessionID: String?
    public var transcriptPath: String?

    public var status: SessionStatus
    public var needsInput: NeedsInputDetail?
    public var resumability: Resumability

    /// Parent session when this agent was spawned by another agent via MCP.
    public var parent: SessionID?

    public var createdAt: Date
    public var updatedAt: Date
    public var lastTurnCompletedAt: Date?
    public var lastSeenAt: Date?
    public var pinned: Bool

    /// Non-nil while the session is archived: its process tree was killed to
    /// free resources but the record is kept so the conversation can be
    /// revived via the normal resume path. Optional for wire/persistence compat.
    public var archivedAt: Date?

    /// True while a mobile client currently owns the session (the phone took
    /// control in the session hand-off model): the PTY is sized to the phone and
    /// the Mac pane shows an "Active on iPhone" card instead of the reflowed grid.
    /// Absent in state/wire payloads written before this field existed, so the
    /// custom `init(from:)` below must decode it as optional-with-default —
    /// synthesized Codable would reject old payloads for the missing key.
    public var remoteActive: Bool = false

    /// `HostEntry.id` when this session runs on a remote host (the local PTY
    /// runs `ssh … tmux new-session -A …`; the agent lives on that host).
    /// nil ⇒ local. Persisted so reattach-by-tmux-name survives daemon restart.
    public var host: String?

    /// Non-nil while the process tree is SIGSTOPped (Chrome-tab hibernation).
    public var hibernation: HibernationInfo?
    /// Latest sampled physical footprint of the session's process tree.
    public var memoryBytes: UInt64?
    /// URLs captured from the screen (PRs, Linear issues, previews, links).
    public var artifacts: [SessionArtifact]?
    /// GitHub-side status for each PR in `artifacts`, polled via `gh` by the
    /// PullRequestMonitor. Keyed back to artifacts by URL.
    public var pullRequests: [PullRequestStatus]?
    /// TCP ports the session's tree is currently LISTENing on.
    public var listeningPorts: [PortInfo]?
    /// A first-class agent detected running in the PTY foreground of a shell
    /// session (the user typed `claude` in a terminal tab). Display + status
    /// detection follow it; `kind` stays what the session was spawned as.
    public var foregroundAgent: AgentKind?

    /// What this session is *behaving* as right now — the adopted foreground
    /// agent when present, else the spawned kind. UI + detection use this.
    public var effectiveKind: AgentKind { foregroundAgent ?? kind }

    public var isArchived: Bool { archivedAt != nil }

    public init(
        id: SessionID = .generate(),
        kind: AgentKind,
        cwd: String,
        projectID: ProjectID,
        worktreePath: String? = nil,
        gitBranch: String? = nil,
        title: String,
        titleSource: TitleSource = .placeholder,
        agentSessionID: String? = nil,
        transcriptPath: String? = nil,
        status: SessionStatus = .starting,
        needsInput: NeedsInputDetail? = nil,
        resumability: Resumability = .notResumable,
        parent: SessionID? = nil,
        createdAt: Date = Date(),
        updatedAt: Date = Date(),
        lastTurnCompletedAt: Date? = nil,
        lastSeenAt: Date? = nil,
        pinned: Bool = false,
        archivedAt: Date? = nil,
        remoteActive: Bool = false,
        host: String? = nil,
        hibernation: HibernationInfo? = nil,
        memoryBytes: UInt64? = nil,
        artifacts: [SessionArtifact]? = nil,
        pullRequests: [PullRequestStatus]? = nil,
        listeningPorts: [PortInfo]? = nil
    ) {
        self.id = id
        self.kind = kind
        self.cwd = cwd
        self.projectID = projectID
        self.worktreePath = worktreePath
        self.gitBranch = gitBranch
        self.title = title
        self.titleSource = titleSource
        self.agentSessionID = agentSessionID
        self.transcriptPath = transcriptPath
        self.status = status
        self.needsInput = needsInput
        self.resumability = resumability
        self.parent = parent
        self.createdAt = createdAt
        self.updatedAt = updatedAt
        self.lastTurnCompletedAt = lastTurnCompletedAt
        self.lastSeenAt = lastSeenAt
        self.pinned = pinned
        self.archivedAt = archivedAt
        self.remoteActive = remoteActive
        self.host = host
        self.hibernation = hibernation
        self.memoryBytes = memoryBytes
        self.artifacts = artifacts
        self.pullRequests = pullRequests
        self.listeningPorts = listeningPorts
    }

    /// Manual decode so fields added after records were first persisted can
    /// default instead of failing the whole state file for a missing key
    /// (synthesized Codable ignores property defaults). Encoding stays
    /// synthesized. When adding a field here, decode it with
    /// `decodeIfPresent` + a default.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(SessionID.self, forKey: .id)
        kind = try c.decode(AgentKind.self, forKey: .kind)
        cwd = try c.decode(String.self, forKey: .cwd)
        projectID = try c.decode(ProjectID.self, forKey: .projectID)
        worktreePath = try c.decodeIfPresent(String.self, forKey: .worktreePath)
        gitBranch = try c.decodeIfPresent(String.self, forKey: .gitBranch)
        title = try c.decode(String.self, forKey: .title)
        titleSource = try c.decode(TitleSource.self, forKey: .titleSource)
        agentSessionID = try c.decodeIfPresent(String.self, forKey: .agentSessionID)
        transcriptPath = try c.decodeIfPresent(String.self, forKey: .transcriptPath)
        status = try c.decode(SessionStatus.self, forKey: .status)
        needsInput = try c.decodeIfPresent(NeedsInputDetail.self, forKey: .needsInput)
        resumability = try c.decode(Resumability.self, forKey: .resumability)
        parent = try c.decodeIfPresent(SessionID.self, forKey: .parent)
        createdAt = try c.decode(Date.self, forKey: .createdAt)
        updatedAt = try c.decode(Date.self, forKey: .updatedAt)
        lastTurnCompletedAt = try c.decodeIfPresent(Date.self, forKey: .lastTurnCompletedAt)
        lastSeenAt = try c.decodeIfPresent(Date.self, forKey: .lastSeenAt)
        pinned = try c.decode(Bool.self, forKey: .pinned)
        archivedAt = try c.decodeIfPresent(Date.self, forKey: .archivedAt)
        remoteActive = try c.decodeIfPresent(Bool.self, forKey: .remoteActive) ?? false
        host = try c.decodeIfPresent(String.self, forKey: .host)
        hibernation = try c.decodeIfPresent(HibernationInfo.self, forKey: .hibernation)
        memoryBytes = try c.decodeIfPresent(UInt64.self, forKey: .memoryBytes)
        artifacts = try c.decodeIfPresent([SessionArtifact].self, forKey: .artifacts)
        pullRequests = try c.decodeIfPresent([PullRequestStatus].self, forKey: .pullRequests)
        listeningPorts = try c.decodeIfPresent([PortInfo].self, forKey: .listeningPorts)
        foregroundAgent = try c.decodeIfPresent(AgentKind.self, forKey: .foregroundAgent)
    }

    public var attention: AttentionLevel {
        AttentionLevel(
            status: status,
            lastTurnCompletedAt: lastTurnCompletedAt,
            lastSeenAt: lastSeenAt
        )
    }

    /// Applies a title update respecting the source priority ladder.
    /// Returns true if the title changed.
    @discardableResult
    public mutating func applyTitle(_ newTitle: String, source: TitleSource) -> Bool {
        guard source >= titleSource, !newTitle.isEmpty else { return false }
        guard newTitle != title || source != titleSource else { return false }
        title = newTitle
        titleSource = source
        return true
    }
}

public struct Project: Codable, Hashable, Sendable, Identifiable {
    public var id: ProjectID
    public var root: String
    public var name: String
    public var pinnedOrder: Int?

    public init(root: String, name: String? = nil, pinnedOrder: Int? = nil) {
        self.id = ProjectID(root: root)
        self.root = root
        self.name = name ?? (root as NSString).lastPathComponent
        self.pinnedOrder = pinnedOrder
    }
}
