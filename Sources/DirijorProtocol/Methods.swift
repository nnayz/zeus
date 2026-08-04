import DirijorCore
import Foundation

/// Control-channel method names. Params/results are the typed structs below,
/// carried through the envelope as JSONValue.
public enum Method {
    public static let hello = "hello"
    public static let sessionSpawn = "session.spawn"
    public static let sessionList = "session.list"
    public static let sessionKill = "session.kill"
    public static let sessionRemove = "session.remove"
    public static let sessionRename = "session.rename"
    public static let sessionResume = "session.resume"
    public static let sessionSendText = "session.send_text"
    public static let sessionResize = "session.resize"
    public static let sessionSetOwner = "session.set_owner"
    public static let sessionReadScreen = "session.read_screen"
    public static let sessionReadScrollback = "session.read_scrollback"
    public static let sessionReadScrollbackCells = "session.read_scrollback_cells"
    /// Read the Git working-tree patch for a session. The daemon resolves the
    /// session's host, so callers never execute a remote path locally.
    public static let sessionReadDiff = "session.read_diff"
    public static let sessionMarkSeen = "session.mark_seen"
    public static let sessionHibernate = "session.hibernate"
    public static let sessionWake = "session.wake"
    /// Kill the session's process tree but keep the record for later revival
    /// via `session.resume`.
    public static let sessionArchive = "session.archive"
    /// Bring an archived record back into the normal list (does NOT respawn).
    public static let sessionUnarchive = "session.unarchive"
    public static let sessionReopenLast = "session.reopen_last"
    /// Move a LIVE Claude session between local and a remote host: WIP-commit +
    /// push the branch, sync the target checkout, shuttle the transcript, then
    /// respawn the SAME record on the target with `claude --resume`.
    public static let sessionMigrate = "session.migrate"
    /// Push local agent preferences (claude/codex config — never credentials)
    /// to a configured remote host via additive rsync-over-ssh.
    public static let hostSyncPrefs = "host.sync_prefs"
    /// Find a checkout of a repository (matched by `git remote get-url origin`)
    /// on a host — under the host's defaultCwd remotely, among the daemon's
    /// known projects locally. Cached daemon-side so pickers feel instant.
    public static let hostLocateRepo = "host.locate_repo"
    /// Scan on-disk agent transcripts for conversations the daemon isn't
    /// tracking (survives daemon restart, unlike the in-memory reopen stack).
    public static let sessionHistory = "session.history"
    /// Resume a conversation discovered via `session.history` into a fresh
    /// tracked session.
    public static let sessionResumeFromHistory = "session.resume_from_history"
    public static let worktreeCreate = "worktree.create"
    public static let worktreeList = "worktree.list"
    public static let worktreeRemove = "worktree.remove"
    public static let worktreeOverview = "worktree.overview"
    public static let projectAdd = "project.add"
    /// Run a cross-browser test flow via the daemon's Playwright pool.
    public static let testRun = "test.run"
    public static let browserAct = "browser.act"
    public static let eventsSubscribe = "events.subscribe"
    public static let eventsWait = "events.wait"
    public static let hookReport = "hook.report"
    public static let stateSnapshot = "state.snapshot"
    /// The desktop app reports its own active/occluded state so the daemon can
    /// slow its status-scan tick while nobody is watching the UI.
    public static let clientSetActive = "client.set_active"
    /// Apply user-facing resource policy without restarting the daemon.
    public static let governorConfigure = "governor.configure"
    public static let agentReadiness = "agent.readiness"
    public static let daemonPrepareShutdown = "daemon.prepare_shutdown"
    /// Persist state and exit — used by the app to replace a stale daemon
    /// binary without the manual quit/kill/relaunch dance.
    public static let daemonShutdown = "daemon.shutdown"
}

/// Event names pushed on subscribed control channels.
///
/// Two layers, published from the same funnel: the *coarse* `session.updated`
/// carries the whole record (what the desktop app renders from), and the
/// *narrow* events below are derived from the same record diff so scripts can
/// subscribe to one thing — "tell me when anything blocks" — without decoding
/// and comparing full records themselves. Narrow events always follow the
/// `session.updated` that produced them, so a consumer that reacts to a narrow
/// event and then re-reads state never sees a stale record.
public enum EventName {
    /// params: SessionRecord — added or any field changed (status, title, …).
    public static let sessionUpdated = "session.updated"
    /// params: SessionResourcesEvent — high-frequency resource fields only.
    public static let sessionResources = "session.resources"
    /// params: { "id": SessionID, "reason": String? }
    public static let sessionRemoved = "session.removed"
    /// params: Project
    public static let projectUpdated = "project.updated"

    /// params: SessionRecord — first time the daemon published this session.
    public static let sessionSpawned = "session.spawned"
    /// params: SessionStatusEvent — a status transition, with both endpoints.
    public static let sessionStatus = "session.status"
    /// params: SessionNeedsInputEvent — a *distinct* blocker appeared (dedupes
    /// on `NeedsInputDetail.blockerIdentity`, like the notification path).
    public static let sessionNeedsInput = "session.needs_input"
    /// params: SessionOutputEvent — the PTY produced output. Coalesced daemon
    /// side: a busy agent bumps its content seq at 10Hz, which nobody wants as
    /// 10 events per second per session.
    public static let sessionOutput = "session.output"
    /// params: SessionArtifactEvent — a PR / preview / link / listening port
    /// was discovered for the first time on this session.
    public static let sessionArtifact = "session.artifact"
    /// params: SessionRecord — the tree was killed but the record kept.
    public static let sessionArchived = "session.archived"
    /// params: WorktreeEvent
    public static let worktreeCreated = "worktree.created"
    /// params: WorktreeEvent
    public static let worktreeRemoved = "worktree.removed"

    /// params: EventsDroppedEvent, seq 0 — synthetic backpressure marker, see
    /// `EventsSubscribeParams`. Never filtered out: a subscriber that asked for
    /// a narrow slice still has to learn that its slice has a hole.
    public static let eventsDropped = "events.dropped"

    /// Every name a subscriber may filter on, for CLI validation and help.
    public static let all: [String] = [
        sessionUpdated, sessionResources, sessionRemoved, projectUpdated,
        sessionSpawned, sessionStatus, sessionNeedsInput, sessionOutput,
        sessionArtifact, sessionArchived,
        worktreeCreated, worktreeRemoved,
        eventsDropped,
    ]
}

// MARK: - Handshake

public struct HelloParams: Codable, Sendable {
    public var proto: Int
    public var build: String
    public var token: String?

    public init(proto: Int = WireVersion.current, build: String, token: String? = nil) {
        self.proto = proto
        self.build = build
        self.token = token
    }
}

public struct HelloResult: Codable, Sendable {
    public var proto: Int
    public var build: String
    public var pid: Int32
    /// SHA-256 of the daemon's own executable, so a client can tell a stale
    /// daemon (old binary still running after a rebuild) from the one in its
    /// bundle and trigger an upgrade restart. Optional for wire compatibility.
    public var executableHash: String?

    public init(proto: Int, build: String, pid: Int32, executableHash: String? = nil) {
        self.proto = proto
        self.build = build
        self.pid = pid
        self.executableHash = executableHash
    }
}

/// `client.set_active`: the app is frontmost (`true`) or backgrounded/occluded
/// (`false`). The daemon backs off its 10Hz status tick when no client is active.
public struct ClientActiveParams: Codable, Sendable {
    public var active: Bool
    public init(active: Bool) { self.active = active }
}

public struct GovernorSettingsParams: Codable, Sendable, Equatable {
    /// 0 disables automatic idle hibernation.
    public var idleThresholdSeconds: Double
    public var hardMemoryBytes: UInt64

    public init(idleThresholdSeconds: Double, hardMemoryBytes: UInt64) {
        self.idleThresholdSeconds = max(0, idleThresholdSeconds)
        self.hardMemoryBytes = hardMemoryBytes
    }
}

public struct AgentReadinessItem: Codable, Sendable, Equatable {
    public var kind: AgentKind
    public var binary: String
    public var path: String?
    /// The agent's full manifest descriptor. This is how the AGENT CATALOG
    /// reaches clients: `agent.readiness` doubles as "what agents exist and
    /// what can they do", so a client renders a manifest-only agent (name,
    /// glyph, approve keystroke) without shipping its own copy of the table.
    ///
    /// Optional purely for wire compatibility — a daemon predating this field
    /// omits it and the client falls back to its built-in knowledge of the
    /// four original agents.
    public var descriptor: AgentDescriptor?
    public var available: Bool { path != nil }

    public init(kind: AgentKind, binary: String, path: String?, descriptor: AgentDescriptor? = nil) {
        self.kind = kind
        self.binary = binary
        self.path = path
        self.descriptor = descriptor
    }
}

public struct AgentReadinessResult: Codable, Sendable, Equatable {
    public var agents: [AgentReadinessItem]
    public init(agents: [AgentReadinessItem]) { self.agents = agents }
}

// MARK: - Sessions

public struct SessionSpawnParams: Codable, Sendable {
    public var kind: AgentKind
    public var cwd: String
    /// Create a fresh worktree off the repo at `cwd` and run there instead.
    public var newWorktree: Bool?
    public var worktreeBranch: String?
    public var title: String?
    public var initialPrompt: String?
    public var parent: SessionID?
    /// Initial PTY geometry so the agent renders at the right width from its
    /// first byte (a TUI's one-shot banner is baked at spawn size).
    public var initialCols: Int?
    public var initialRows: Int?
    /// `HostEntry.id` from `hosts.json` — spawn on that remote host over
    /// ssh+tmux instead of locally. nil ⇒ local (wire-compatible).
    public var host: String?
    /// Repo-preserving spawn: open in the checkout of the SAME repository as
    /// this session (matched by origin URL) on the target host. Falls back to
    /// `cwd` / the host's defaultCwd when the repo isn't cloned there.
    public var sameRepoAs: SessionID?

    public init(
        kind: AgentKind,
        cwd: String,
        newWorktree: Bool? = nil,
        worktreeBranch: String? = nil,
        title: String? = nil,
        initialPrompt: String? = nil,
        parent: SessionID? = nil,
        initialCols: Int? = nil,
        initialRows: Int? = nil,
        host: String? = nil,
        sameRepoAs: SessionID? = nil
    ) {
        self.kind = kind
        self.cwd = cwd
        self.newWorktree = newWorktree
        self.worktreeBranch = worktreeBranch
        self.title = title
        self.initialPrompt = initialPrompt
        self.parent = parent
        self.initialCols = initialCols
        self.initialRows = initialRows
        self.host = host
        self.sameRepoAs = sameRepoAs
    }
}

public struct SessionListResult: Codable, Sendable {
    public var sessions: [SessionRecord]
    public var projects: [Project]

    public init(sessions: [SessionRecord], projects: [Project]) {
        self.sessions = sessions
        self.projects = projects
    }
}

public struct SessionIDParams: Codable, Sendable {
    public var sessionID: SessionID
    public init(sessionID: SessionID) { self.sessionID = sessionID }
}

/// One past conversation discovered on disk by scanning the agents' own
/// transcript stores (`~/.claude/projects`, `~/.codex/sessions`) — independent
/// of any daemon session record, so it survives app quit / daemon restart.
public struct HistoryEntry: Codable, Sendable, Identifiable, Hashable {
    /// Agent-side conversation id: Claude session UUID / Codex thread id. This
    /// is what `--resume` / `resume` take, and it's stable + unique.
    public var id: String
    /// `.claudeCode` or `.codex` — the only kinds with resumable transcripts.
    public var kind: AgentKind
    public var cwd: String
    /// Best-effort: latest Claude `ai-title`, else first user prompt, else a
    /// folder+date placeholder.
    public var title: String?
    public var transcriptPath: String
    /// Transcript file modification time — the conversation's last activity.
    public var lastActiveAt: Date
    /// Transcript file creation time, when the filesystem exposes it.
    public var createdAt: Date?
    /// Whether `cwd` still exists, so the UI can disable resume for dead
    /// folders rather than triggering a guaranteed server-side failure.
    public var cwdExists: Bool

    public init(
        id: String,
        kind: AgentKind,
        cwd: String,
        title: String? = nil,
        transcriptPath: String,
        lastActiveAt: Date,
        createdAt: Date? = nil,
        cwdExists: Bool
    ) {
        self.id = id
        self.kind = kind
        self.cwd = cwd
        self.title = title
        self.transcriptPath = transcriptPath
        self.lastActiveAt = lastActiveAt
        self.createdAt = createdAt
        self.cwdExists = cwdExists
    }
}

public struct SessionHistoryResult: Codable, Sendable {
    public var entries: [HistoryEntry]
    public init(entries: [HistoryEntry]) { self.entries = entries }
}

public struct ResumeFromHistoryParams: Codable, Sendable {
    public var entry: HistoryEntry
    public init(entry: HistoryEntry) { self.entry = entry }
}

/// Params for methods that just reference one session (hibernate / wake).
public typealias SessionRefParams = SessionIDParams

public struct SessionRenameParams: Codable, Sendable {
    public var sessionID: SessionID
    public var title: String
    public init(sessionID: SessionID, title: String) {
        self.sessionID = sessionID
        self.title = title
    }
}

public struct SendTextParams: Codable, Sendable {
    public var sessionID: SessionID
    public var text: String
    /// Append a carriage return so the agent submits the prompt.
    public var submit: Bool

    public init(sessionID: SessionID, text: String, submit: Bool = true) {
        self.sessionID = sessionID
        self.text = text
        self.submit = submit
    }
}

public struct ResizeParams: Codable, Sendable {
    public var sessionID: SessionID
    public var cols: Int
    public var rows: Int
    public init(sessionID: SessionID, cols: Int, rows: Int) {
        self.sessionID = sessionID
        self.cols = cols
        self.rows = rows
    }
}

/// `session.set_owner`: explicitly claim geometry ownership of a session in the
/// hand-off model. `.desktop` → the Mac reclaims (phone shows "Active on your
/// Mac"); `.mobile` → the phone (re)claims (Mac shows "Active on iPhone").
public struct SessionSetOwnerParams: Codable, Sendable {
    public var sessionID: SessionID
    public var role: ClientRole
    public init(sessionID: SessionID, role: ClientRole) {
        self.sessionID = sessionID
        self.role = role
    }
}

public struct ReadScreenResult: Codable, Sendable {
    public var text: String
    public var cols: Int
    public var rows: Int
    public init(text: String, cols: Int, rows: Int) {
        self.text = text
        self.cols = cols
        self.rows = rows
    }
}

public enum SessionDiffBase: String, Codable, Sendable {
    case defaultBranch
    case head
}

public struct SessionReadDiffParams: Codable, Sendable {
    public var sessionID: SessionID
    /// Optional so updated clients remain compatible with older daemons. A
    /// missing value means the repository's default branch.
    public var base: SessionDiffBase?

    public init(sessionID: SessionID, base: SessionDiffBase? = nil) {
        self.sessionID = sessionID
        self.base = base
    }
}

/// A bounded unified patch. `Data` is base64 on the JSON wire, keeping the
/// control-line size predictable even when source text contains many quotes.
public struct SessionReadDiffResult: Codable, Sendable, Equatable {
    public var patch: Data
    public var repoRoot: String
    public var truncated: Bool
    /// The ref actually selected by the daemon (`origin/main`, `main`, or
    /// `HEAD`). Optional for responses from older daemons.
    public var baseRef: String?

    public init(
        patch: Data, repoRoot: String, truncated: Bool, baseRef: String? = nil
    ) {
        self.patch = patch
        self.repoRoot = repoRoot
        self.truncated = truncated
        self.baseRef = baseRef
    }
}

/// Full scrollback as plain text, for client-side "find in scrollback". Rows are
/// addressed in SwiftTerm's scroll-invariant space so the follow-up cell read
/// (`ReadScrollbackCellsParams`) can point at the same rows. `lines[0]` is
/// scroll-invariant row `firstRow`; `visibleStartRow` is the invariant index of
/// the live screen's top row (so `visibleStartRow - firstRow == lines.count -
/// rows` when the emulator is at the bottom).
public struct ReadScrollbackResult: Codable, Sendable {
    public var lines: [String]
    public var firstRow: Int
    public var visibleStartRow: Int
    public var cols: Int
    public var rows: Int
    public var contentSeq: UInt64
    public var isAltScreen: Bool

    public init(
        lines: [String], firstRow: Int, visibleStartRow: Int,
        cols: Int, rows: Int, contentSeq: UInt64, isAltScreen: Bool
    ) {
        self.lines = lines
        self.firstRow = firstRow
        self.visibleStartRow = visibleStartRow
        self.cols = cols
        self.rows = rows
        self.contentSeq = contentSeq
        self.isAltScreen = isAltScreen
    }
}

public struct ReadScrollbackCellsParams: Codable, Sendable {
    public var sessionID: SessionID
    /// Scroll-invariant index of the first row to read (from `ReadScrollbackResult`).
    public var firstRow: Int
    public var maxRows: Int
    public init(sessionID: SessionID, firstRow: Int, maxRows: Int) {
        self.sessionID = sessionID
        self.firstRow = firstRow
        self.maxRows = maxRows
    }
}

/// A window of scrollback rows as cells, for the client-side scrollback viewport.
/// `payload` is the `GridRowCodec` block (`rowCount` consecutive rows, no y
/// prefix) — the same RLE cell encoding grid frames use; JSON carries it base64.
public struct ReadScrollbackCellsResult: Codable, Sendable {
    public var payload: Data
    public var firstRow: Int
    public var rowCount: Int
    public var totalRows: Int
    public var liveStartRow: Int
    public var cols: Int
    public var contentSeq: UInt64

    public init(
        payload: Data, firstRow: Int, rowCount: Int, totalRows: Int,
        liveStartRow: Int, cols: Int, contentSeq: UInt64
    ) {
        self.payload = payload
        self.firstRow = firstRow
        self.rowCount = rowCount
        self.totalRows = totalRows
        self.liveStartRow = liveStartRow
        self.cols = cols
        self.contentSeq = contentSeq
    }

    /// Decodes `payload` back into `rowCount` rows of cells (each `cols` wide).
    public func decodedRows() -> [[GridCell]]? {
        GridRowCodec.decodeRows(payload, rowCount: rowCount)
    }
}

// MARK: - Remote hosts (prefs sync / repo location / session migration)

/// `host.sync_prefs`: push the local user's agent preferences to `host` so
/// remote agents behave like local ones. The include list is FIXED server-side
/// (claude: CLAUDE.md, settings.json, keybindings.json, commands/, skills/,
/// agents/; codex: config.toml, AGENTS.md, prompts/) and never contains
/// credentials, transcripts, or caches.
public struct HostSyncPrefsParams: Codable, Sendable {
    /// `HostEntry.id` from `hosts.json`.
    public var host: String
    public init(host: String) { self.host = host }
}

/// Per-tool outcome of a prefs sync. `ok` with an empty `synced` list means
/// the tool had nothing to push (no local config).
public struct PrefsSyncToolReport: Codable, Sendable, Equatable {
    /// "claude" | "codex"
    public var tool: String
    public var ok: Bool
    /// Item names that existed locally and were pushed (e.g. "CLAUDE.md").
    public var synced: [String]
    /// rsync/ssh failure detail; nil on success.
    public var error: String?

    public init(tool: String, ok: Bool, synced: [String], error: String? = nil) {
        self.tool = tool
        self.ok = ok
        self.synced = synced
        self.error = error
    }
}

public struct HostSyncPrefsResult: Codable, Sendable, Equatable {
    public var tools: [PrefsSyncToolReport]
    public init(tools: [PrefsSyncToolReport]) { self.tools = tools }
}

/// `host.locate_repo`: find a checkout of a repo on a host by origin URL.
/// Provide either `originURL` directly, or `sessionID` to derive the origin
/// from that session's checkout (cwd + host) — one round trip for pickers.
public struct HostLocateRepoParams: Codable, Sendable {
    /// Target host id; nil ⇒ search the daemon's local project roots.
    public var host: String?
    public var originURL: String?
    public var sessionID: SessionID?

    public init(host: String? = nil, originURL: String? = nil, sessionID: SessionID? = nil) {
        self.host = host
        self.originURL = originURL
        self.sessionID = sessionID
    }
}

public struct HostLocateRepoResult: Codable, Sendable, Equatable {
    /// Absolute checkout path on the target host; nil when not found.
    public var path: String?
    /// The origin URL that was matched (echoed, or derived from `sessionID`).
    /// nil ⇒ the session's cwd is not a git repo with an origin remote.
    public var originURL: String?

    public init(path: String? = nil, originURL: String? = nil) {
        self.path = path
        self.originURL = originURL
    }
}

/// `session.migrate`: one-click handoff of a live Claude session between local
/// and a remote host, preserving conversation context (`claude --resume`) and
/// code state (WIP commit + push + hard-reset of the target checkout).
public struct SessionMigrateParams: Codable, Sendable {
    public var sessionID: SessionID
    /// Target `HostEntry.id`; nil ⇒ migrate back to local.
    public var targetHost: String?

    public init(sessionID: SessionID, targetHost: String? = nil) {
        self.sessionID = sessionID
        self.targetHost = targetHost
    }
}

public struct SessionMigrateResult: Codable, Sendable {
    /// The migrated record (same id/title; host + cwd updated, respawned).
    public var session: SessionRecord
    /// False when no transcript existed to shuttle: the session respawned
    /// with a fresh conversation — code state moved, context was lost.
    public var transcriptMigrated: Bool
    /// Non-fatal issues (e.g. lingering remote tmux, missing transcript).
    public var warning: String?

    public init(session: SessionRecord, transcriptMigrated: Bool, warning: String? = nil) {
        self.session = session
        self.transcriptMigrated = transcriptMigrated
        self.warning = warning
    }
}

// MARK: - Worktrees / projects

public struct WorktreeCreateParams: Codable, Sendable {
    public var repoPath: String
    public var branch: String?
    public var base: String?
    public init(repoPath: String, branch: String? = nil, base: String? = nil) {
        self.repoPath = repoPath
        self.branch = branch
        self.base = base
    }
}

public struct WorktreeListParams: Codable, Sendable {
    public var repoPath: String
    public init(repoPath: String) { self.repoPath = repoPath }
}

public struct WorktreeRemoveParams: Codable, Sendable {
    public var repoPath: String
    public var worktreePath: String
    public var force: Bool
    public init(repoPath: String, worktreePath: String, force: Bool = false) {
        self.repoPath = repoPath
        self.worktreePath = worktreePath
        self.force = force
    }
}

public struct ProjectAddParams: Codable, Sendable {
    public var root: String
    public init(root: String) { self.root = root }
}

/// `worktree.overview` takes no arguments (scans every known project root).
public struct WorktreeOverviewParams: Codable, Sendable {
    public init() {}
}

/// One worktree card: git state + the session (if any) running inside it.
public struct WorktreeOverviewEntry: Codable, Sendable, Hashable {
    public var path: String
    public var branch: String?
    public var projectRoot: String
    public var sessionID: SessionID?
    public var sessionStatus: SessionStatus?
    /// `git status --porcelain` is non-empty.
    public var dirty: Bool
    /// Branch is merged into the repository's default branch.
    public var merged: Bool
    /// Age of the worktree directory in days (filesystem creation time).
    public var ageDays: Int
    /// Suggest-only cleanup flag: no live session ∧ merged ∧ clean ∧ old.
    public var staleSuggestion: Bool

    public init(
        path: String,
        branch: String?,
        projectRoot: String,
        sessionID: SessionID? = nil,
        sessionStatus: SessionStatus? = nil,
        dirty: Bool = false,
        merged: Bool = false,
        ageDays: Int = 0,
        staleSuggestion: Bool = false
    ) {
        self.path = path
        self.branch = branch
        self.projectRoot = projectRoot
        self.sessionID = sessionID
        self.sessionStatus = sessionStatus
        self.dirty = dirty
        self.merged = merged
        self.ageDays = ageDays
        self.staleSuggestion = staleSuggestion
    }
}

public struct WorktreeOverviewResult: Codable, Sendable {
    public var entries: [WorktreeOverviewEntry]
    public init(entries: [WorktreeOverviewEntry]) { self.entries = entries }
}

// MARK: - Browser testing

/// `test.run`: drive a cross-browser flow through the daemon's Playwright pool.
/// `steps` are single-key JSON objects (click/type/drag/select/assert/…) passed
/// through verbatim to the sidecar; the result is the sidecar's structured JSON
/// (per-engine pass/fail, a11y diff, console errors, on-failure screenshot path).
/// One step of an interactive browser session. Unlike `TestRunParams` — a whole
/// scripted flow submitted at once — this is a single act in a loop the agent
/// drives: open, look, act on what it saw, look again.
///
/// The browser is keyed by `sessionID`, so each agent gets its own isolated
/// context (cookies, storage, logins) without ever naming one. Every field past
/// `action` is optional because the actions genuinely differ in shape; the
/// sidecar validates what each one needs and says what is missing.
public struct BrowserParams: Codable, Sendable {
    public var sessionID: SessionID
    public var action: String
    public var url: String?
    /// Element handle from the last snapshot, e.g. "e3" or "@e3".
    public var ref: String?
    /// CSS escape hatch for the cases a ref can't express (counting matches).
    public var selector: String?
    public var text: String?
    public var key: String?
    public var value: String?
    /// For `get`: url | title | text | html | value | count.
    public var what: String?
    public var ms: Double?
    public var state: String?
    public var direction: String?
    public var amount: Double?
    public var button: String?
    public var double: Bool?
    public var full: Bool?
    public var annotate: Bool?
    public var engine: String?
    /// Named profile: persist cookies + logins across sessions and restarts.
    public var profile: String?

    public init(
        sessionID: SessionID, action: String, url: String? = nil, ref: String? = nil,
        selector: String? = nil, text: String? = nil, key: String? = nil, value: String? = nil,
        what: String? = nil, ms: Double? = nil, state: String? = nil, direction: String? = nil,
        amount: Double? = nil, button: String? = nil, double: Bool? = nil, full: Bool? = nil,
        annotate: Bool? = nil, engine: String? = nil, profile: String? = nil
    ) {
        self.sessionID = sessionID
        self.action = action
        self.url = url
        self.ref = ref
        self.selector = selector
        self.text = text
        self.key = key
        self.value = value
        self.what = what
        self.ms = ms
        self.state = state
        self.direction = direction
        self.amount = amount
        self.button = button
        self.double = double
        self.full = full
        self.annotate = annotate
        self.engine = engine
        self.profile = profile
    }
}

public struct TestRunParams: Codable, Sendable {
    public var url: String
    public var engines: [String]?
    public var steps: [JSONValue]
    /// "a11y" (default) or "screenshot" — controls default observation cost.
    public var observe: String?
    /// Named visual baseline to compare against (v1b).
    public var baseline: String?
    /// Named profile: persist cookies + localStorage across runs so a login
    /// carries forward. Absent ⇒ a clean, isolated context each run.
    public var profile: String?
    /// Auth hand-off: `{cookies: [...], localStorage: {...}}` seeded into every
    /// engine's context before the first navigation, so authed flows work on
    /// engines nobody logged into. Passed through verbatim to the sidecar.
    public var auth: JSONValue?

    public init(
        url: String, engines: [String]? = nil, steps: [JSONValue],
        observe: String? = nil, baseline: String? = nil, profile: String? = nil,
        auth: JSONValue? = nil
    ) {
        self.url = url
        self.engines = engines
        self.steps = steps
        self.observe = observe
        self.baseline = baseline
        self.profile = profile
        self.auth = auth
    }
}

// MARK: - Events

public struct EventsSubscribeParams: Codable, Sendable {
    /// Replay events with seq > sinceSeq from the daemon's ring before going live.
    public var sinceSeq: UInt64?
    /// Deliver only events tagged with one of these sessions. Untagged events
    /// (project / worktree) are suppressed while this is set — asking for two
    /// sessions and getting every worktree event back would defeat the point.
    public var sessions: [SessionID]?
    /// Deliver only these event names (see `EventName.all`). `events.dropped`
    /// is always delivered regardless.
    public var kinds: [String]?

    public init(sinceSeq: UInt64? = nil, sessions: [SessionID]? = nil, kinds: [String]? = nil) {
        self.sinceSeq = sinceSeq
        self.sessions = sessions
        self.kinds = kinds
    }
}

/// `events.wait`: one-shot long poll. Two independent match modes, OR'd — a
/// status target (`until`, the original shape MCP's `wait_for_agent` uses) and
/// an event-name target (`kinds`). Whichever fires first returns.
///
/// `sessionID` and `until` stay non-optional on the wire for every existing
/// client; the custom decoder makes them optional so a name-only wait
/// ("tell me about the next PR anywhere") needs no session at all.
public struct EventsWaitParams: Codable, Sendable {
    /// Scope the wait to one session. nil ⇒ any session.
    public var sessionID: SessionID?
    /// Wait until the session's status becomes one of these (by case name:
    /// "idle", "working", "needsInput", "exited"). "done" is accepted as an
    /// alias for "idle".
    public var until: [String]
    /// Wait until one of these event names is published (see `EventName.all`).
    public var kinds: [String]?
    public var timeoutMs: Int

    public init(
        sessionID: SessionID? = nil,
        until: [String] = [],
        kinds: [String]? = nil,
        timeoutMs: Int = 600_000
    ) {
        self.sessionID = sessionID
        self.until = until
        self.kinds = kinds
        self.timeoutMs = timeoutMs
    }

    private enum CodingKeys: String, CodingKey {
        case sessionID, until, kinds, timeoutMs
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.sessionID = try c.decodeIfPresent(SessionID.self, forKey: .sessionID)
        self.until = try c.decodeIfPresent([String].self, forKey: .until) ?? []
        self.kinds = try c.decodeIfPresent([String].self, forKey: .kinds)
        self.timeoutMs = try c.decodeIfPresent(Int.self, forKey: .timeoutMs) ?? 600_000
    }
}

/// One event as it appears in an `events.wait` result — the same triple the
/// control channel pushes, so a caller can handle streamed and polled events
/// with one code path.
public struct EventEnvelope: Codable, Sendable {
    public var name: String
    public var seq: UInt64
    public var params: JSONValue

    public init(name: String, seq: UInt64, params: JSONValue) {
        self.name = name
        self.seq = seq
        self.params = params
    }
}

public struct EventsWaitResult: Codable, Sendable {
    /// The session the wait resolved on. Always present for a `sessionID`-scoped
    /// wait (every existing caller); absent only for a name-only wait that
    /// matched an event carrying no session.
    public var session: SessionRecord?
    public var timedOut: Bool
    /// The event that satisfied a `kinds` match. Absent for status matches,
    /// which are already fully described by `session`.
    public var event: EventEnvelope?

    public init(session: SessionRecord?, timedOut: Bool, event: EventEnvelope? = nil) {
        self.session = session
        self.timedOut = timedOut
        self.event = event
    }
}

// MARK: Event payloads

/// `session.status`. `label` is the canonical flat string (`"needsInput:permission"`,
/// `"exited:archived"`) so a shell script can compare without a JSON decoder.
public struct SessionStatusEvent: Codable, Sendable {
    public var id: SessionID
    public var from: SessionStatus?
    public var to: SessionStatus
    public var label: String
    /// Populated when `to` is `.needsInput`, so a single subscription to
    /// `session.status` carries the blocker detail too.
    public var needsInput: NeedsInputDetail?

    public init(
        id: SessionID, from: SessionStatus?, to: SessionStatus, needsInput: NeedsInputDetail? = nil
    ) {
        self.id = id
        self.from = from
        self.to = to
        self.label = to.label
        self.needsInput = needsInput
    }
}

/// `session.needs_input`.
public struct SessionNeedsInputEvent: Codable, Sendable {
    public var id: SessionID
    public var needsInput: NeedsInputDetail

    public init(id: SessionID, needsInput: NeedsInputDetail) {
        self.id = id
        self.needsInput = needsInput
    }
}

/// `session.output` — coalesced PTY activity. `contentSeq` is the emulator's
/// monotonic content counter, so a consumer can tell how much moved.
public struct SessionOutputEvent: Codable, Sendable {
    public var id: SessionID
    public var contentSeq: UInt64

    public init(id: SessionID, contentSeq: UInt64) {
        self.id = id
        self.contentSeq = contentSeq
    }
}

/// `session.resources` — resource-governor fields that do not represent user
/// or agent activity. Optional members mean "not sampled in this update".
public struct SessionResourcesEvent: Codable, Sendable {
    public var id: SessionID
    public var memoryBytes: UInt64?
    public var listeningPorts: [PortInfo]?
    public var artifacts: [SessionArtifact]?

    public init(
        id: SessionID,
        memoryBytes: UInt64? = nil,
        listeningPorts: [PortInfo]? = nil,
        artifacts: [SessionArtifact]? = nil
    ) {
        self.id = id
        self.memoryBytes = memoryBytes
        self.listeningPorts = listeningPorts
        self.artifacts = artifacts
    }
}

/// `session.artifact`. URL artifacts carry `url`; a newly-observed listening
/// port carries `port` + `process` and `kind == "port"`.
public struct SessionArtifactEvent: Codable, Sendable {
    public var id: SessionID
    public var kind: String
    public var url: String?
    public var port: Int?
    public var process: String?

    public init(
        id: SessionID, kind: String, url: String? = nil,
        port: Int? = nil, process: String? = nil
    ) {
        self.id = id
        self.kind = kind
        self.url = url
        self.port = port
        self.process = process
    }

    public static func port(id: SessionID, _ info: PortInfo) -> SessionArtifactEvent {
        SessionArtifactEvent(id: id, kind: "port", port: info.port, process: info.processName)
    }

    public static func artifact(id: SessionID, _ artifact: SessionArtifact) -> SessionArtifactEvent {
        SessionArtifactEvent(id: id, kind: artifact.kind.rawValue, url: artifact.url)
    }
}

/// `session.removed`.
public struct SessionRemovedEvent: Codable, Sendable {
    public var id: SessionID
    /// "released" for an explicit kill/remove; absent for older daemons.
    public var reason: String?

    public init(id: SessionID, reason: String? = nil) {
        self.id = id
        self.reason = reason
    }
}

/// `worktree.created` / `worktree.removed`.
public struct WorktreeEvent: Codable, Sendable {
    public var repoPath: String
    public var path: String
    public var branch: String?

    public init(repoPath: String, path: String, branch: String? = nil) {
        self.repoPath = repoPath
        self.path = path
        self.branch = branch
    }
}

/// `events.dropped` — the backpressure marker. `dropped` events with seq in
/// `[fromSeq, toSeq]` never reached this subscriber; re-read state (or
/// resubscribe with `sinceSeq: fromSeq - 1`) to close the hole.
public struct EventsDroppedEvent: Codable, Sendable {
    public var dropped: Int
    public var fromSeq: UInt64
    public var toSeq: UInt64

    public init(dropped: Int, fromSeq: UInt64, toSeq: UInt64) {
        self.dropped = dropped
        self.fromSeq = fromSeq
        self.toSeq = toSeq
    }
}

// MARK: - Hook ingestion (from the dirijor CLI helpers)

public struct HookReportParams: Codable, Sendable {
    /// "claude-hook" | "codex-notify"
    public var kind: String
    public var dirijorSessionID: SessionID?
    /// Claude hook event name ("PreToolUse", …); nil for codex-notify.
    public var event: String?
    /// The raw payload (Claude hook stdin JSON / Codex notify argv JSON), verbatim.
    public var payload: JSONValue

    public init(kind: String, dirijorSessionID: SessionID?, event: String?, payload: JSONValue) {
        self.kind = kind
        self.dirijorSessionID = dirijorSessionID
        self.event = event
        self.payload = payload
    }
}

// MARK: - Data-channel attach handshake

/// Who a client is, for geometry arbitration under the session hand-off model.
/// A `.mobile` client that attaches TAKES CONTROL: the shared PTY is sized to the
/// phone and the Mac shows a hand-off card. Ownership flips back to `.desktop`
/// when the phone leaves, or when the Mac explicitly reclaims via `session.set_owner`
/// (`desktopHold`). Decoding an absent role yields `.desktop` so old Mac clients
/// (which send no role) stay the default geometry owner on the wire.
public enum ClientRole: String, Codable, Sendable {
    case desktop
    case mobile
}

/// First (and only) JSON line a client sends on a data connection before the
/// stream switches to binary frames.
public struct AttachRequest: Codable, Sendable {
    public var attach: SessionID
    public var fromOffset: UInt64?
    public var token: String?
    /// The attaching client's role. Absent on the wire ⇒ `.desktop` (legacy
    /// clients predate this field and must remain geometry-authoritative).
    public var role: ClientRole

    public init(
        attach: SessionID, fromOffset: UInt64? = nil, token: String? = nil,
        role: ClientRole = .desktop
    ) {
        self.attach = attach
        self.fromOffset = fromOffset
        self.token = token
        self.role = role
    }

    private enum CodingKeys: String, CodingKey {
        case attach, fromOffset, token, role
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.attach = try c.decode(SessionID.self, forKey: .attach)
        self.fromOffset = try c.decodeIfPresent(UInt64.self, forKey: .fromOffset)
        self.token = try c.decodeIfPresent(String.self, forKey: .token)
        self.role = try c.decodeIfPresent(ClientRole.self, forKey: .role) ?? .desktop
    }
}

/// First JSON line a client sends on a forwarding connection before the stream
/// switches to raw TCP bytes.
public struct ForwardRequest: Codable, Sendable {
    public var forward: UInt16
    public var token: String?

    public init(forward: UInt16, token: String? = nil) {
        self.forward = forward
        self.token = token
    }
}

/// One JSON line sent by the daemon before a forwarding connection switches to
/// raw TCP bytes, or closes on failure.
public struct ForwardAck: Codable, Sendable {
    public var ok: Bool
    public var error: String?

    public init(ok: Bool, error: String? = nil) {
        self.ok = ok
        self.error = error
    }
}
