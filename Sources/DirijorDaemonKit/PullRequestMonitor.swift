import DirijorCore
import Foundation

/// Polls GitHub (via the `gh` CLI) for the state of every PR URL captured as a
/// session artifact: open/merged/closed, draft, review decision, mergeability,
/// CI checks, comment counts, and +/- line stats. Results land on
/// `SessionRecord.pullRequests`; one shared per-URL cache dedupes PRs that
/// appear in several sessions. Silently inert when `gh` isn't installed.
public actor PullRequestMonitor {
    /// Poll cadence. PR state moves at human speed; two minutes keeps status
    /// useful without making `gh` a permanent daemon workload.
    private let interval: Duration
    /// A cached URL is not refetched within this window even across ticks.
    private let refreshTTL: TimeInterval
    /// Network fetches per tick are capped so a screen full of PR links can't
    /// turn one sweep into a minute of serial gh calls.
    static let maxFetchesPerTick = 2
    /// Review-thread resolution needs a separate GraphQL subprocess. Refresh
    /// it much less often than the main PR state.
    private let threadRefreshTTL: TimeInterval

    private let registry: SessionRegistry
    private let ghPath: String?
    private var loopTask: Task<Void, Never>?
    private var cache: [String: PullRequestStatus] = [:]
    private var lastAttemptAt: [String: Date] = [:]
    private var lastThreadAttemptAt: [String: Date] = [:]
    private var loggedMissingGh = false

    public init(
        registry: SessionRegistry,
        interval: Duration = .seconds(120),
        refreshTTL: TimeInterval = 115,
        threadRefreshTTL: TimeInterval = 1800,
        ghPath: String? = LoginEnvironment.resolve("gh")
    ) {
        self.registry = registry
        self.interval = interval
        self.refreshTTL = refreshTTL
        self.threadRefreshTTL = threadRefreshTTL
        self.ghPath = ghPath
    }

    public func start() {
        guard loopTask == nil else { return }
        loopTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                try? await Task.sleep(for: self.interval)
                guard !Task.isCancelled else { return }
                await self.tick()
            }
        }
        DaemonLog.shared.log(
            "pull-request monitor started (gh: \(ghPath ?? "not found"), every \(interval))")
    }

    public func stop() {
        loopTask?.cancel()
        loopTask = nil
    }

    /// One sweep. Internal so tests can drive it without the timer.
    func tick() async {
        guard let ghPath else {
            if !loggedMissingGh {
                loggedMissingGh = true
                DaemonLog.shared.log("pull-request monitor idle: gh not on login PATH")
            }
            return
        }

        // PR URLs worth polling: terminals currently attached to a client, or
        // records viewed recently. Merely being a live restored process is not
        // evidence that anyone is looking at its PR pill.
        let now = Date()
        var attached: Set<SessionID> = []
        for entry in await registry.liveSessionsSnapshot()
        where await entry.session.sinkCount > 0 {
            attached.insert(entry.id)
        }
        var wanted: [SessionID: [String]] = [:]
        for record in await registry.list().sessions {
            let recentlySeen = record.lastSeenAt.map { now.timeIntervalSince($0) < 600 } ?? false
            guard attached.contains(record.id) || recentlySeen else { continue }
            let urls = (record.artifacts ?? [])
                .filter { $0.kind == .pullRequest }
                .map(\.url)
            if !urls.isEmpty { wanted[record.id] = urls }
        }
        guard !wanted.isEmpty else { return }

        // Refresh stale cache entries, oldest-attempt first under the cap.
        let stale = Set(wanted.values.flatMap { $0 })
            .filter { url in
                guard let last = lastAttemptAt[url] else { return true }
                return now.timeIntervalSince(last) >= refreshTTL
            }
            .sorted { (lastAttemptAt[$0] ?? .distantPast) < (lastAttemptAt[$1] ?? .distantPast) }
        for url in stale.prefix(Self.maxFetchesPerTick) {
            lastAttemptAt[url] = now
            let refreshThreads =
                lastThreadAttemptAt[url].map { now.timeIntervalSince($0) >= threadRefreshTTL }
                ?? true
            if refreshThreads { lastThreadAttemptAt[url] = now }
            if var status = Self.fetch(
                url: url, ghPath: ghPath, includeThreads: refreshThreads)
            {
                if !refreshThreads, let previous = cache[url] {
                    status.resolvedThreads = previous.resolvedThreads
                    status.totalThreads = previous.totalThreads
                }
                cache[url] = status
            }
        }

        for (id, urls) in wanted {
            let statuses = urls.compactMap { cache[$0] }
            await registry.applyPullRequestStatuses(sessionID: id, statuses: statuses)
        }
    }

    // MARK: gh shell-out

    /// `gh pr view <url> --json …` plus a GraphQL round-trip for review-thread
    /// resolution (which `pr view` can't report). Returns nil on any failure
    /// (bad auth, deleted PR, offline) — the last cached status, if any, stays
    /// in effect. Thread counts are best-effort: their failure doesn't sink
    /// the whole fetch.
    static func fetch(
        url: String,
        ghPath: String,
        timeout: TimeInterval = 15,
        includeThreads: Bool = true
    ) -> PullRequestStatus? {
        let fields =
            "number,title,state,isDraft,reviewDecision,mergeable,mergeStateStatus,"
            + "additions,deletions,changedFiles,comments,reviews,statusCheckRollup"
        guard let data = runGh(["pr", "view", url, "--json", fields], ghPath: ghPath, timeout: timeout),
            var status = parse(data, url: url, now: Date())
        else { return nil }

        if includeThreads,
            let (owner, repo, number) = prCoordinates(url: url),
            let threadData = runGh(
                [
                    "api", "graphql",
                    "-f",
                    "query=query($owner:String!,$name:String!,$number:Int!){"
                        + "repository(owner:$owner,name:$name){pullRequest(number:$number){"
                        + "reviewThreads(first:100){totalCount nodes{isResolved}}}}}",
                    "-f", "owner=\(owner)", "-f", "name=\(repo)", "-F", "number=\(number)",
                ], ghPath: ghPath, timeout: timeout),
            let threads = parseThreads(threadData)
        {
            status.resolvedThreads = threads.resolved
            status.totalThreads = threads.total
        }
        return status
    }

    /// Runs gh with a watchdog so a hung network call can't wedge the sweep.
    private static func runGh(_ args: [String], ghPath: String, timeout: TimeInterval) -> Data? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: ghPath)
        process.arguments = args
        var env = ProcessInfo.processInfo.environment
        env["GH_PROMPT_DISABLED"] = "1"
        env["GH_NO_UPDATE_NOTIFIER"] = "1"
        process.environment = env

        let stdout = Pipe()
        process.standardOutput = stdout
        process.standardError = Pipe()
        do {
            try process.run()
        } catch {
            return nil
        }
        let watchdog = DispatchWorkItem { [weak process] in
            if process?.isRunning == true { process?.terminate() }
        }
        DispatchQueue.global().asyncAfter(deadline: .now() + timeout, execute: watchdog)
        let data = stdout.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        watchdog.cancel()

        guard process.terminationReason == .exit, process.terminationStatus == 0 else {
            return nil
        }
        return data
    }

    /// `github.com/owner/repo/pull/123` → (owner, repo, 123).
    static func prCoordinates(url: String) -> (owner: String, repo: String, number: Int)? {
        let parts = url.split(separator: "/").map(String.init)
        guard let pullIdx = parts.firstIndex(of: "pull"), pullIdx >= 2, pullIdx + 1 < parts.count,
            let number = Int(parts[pullIdx + 1].prefix { $0.isNumber })
        else { return nil }
        return (parts[pullIdx - 2], parts[pullIdx - 1], number)
    }

    // MARK: Parsing

    private struct GhPRView: Decodable {
        struct Comment: Decodable {}
        struct Review: Decodable {}
        /// statusCheckRollup mixes CheckRun (name/status/conclusion/detailsUrl,
        /// optionally workflowName) and StatusContext (context/state/targetUrl)
        /// entries.
        struct Check: Decodable {
            var name: String?
            var context: String?
            var workflowName: String?
            var status: String?
            var conclusion: String?
            var state: String?
            var detailsUrl: String?
            var targetUrl: String?
        }

        var number: Int
        var title: String?
        var state: String
        var isDraft: Bool?
        var reviewDecision: String?
        var mergeable: String?
        var mergeStateStatus: String?
        var additions: Int?
        var deletions: Int?
        var changedFiles: Int?
        var comments: [Comment]?
        var reviews: [Review]?
        var statusCheckRollup: [Check]?
    }

    /// Decodes the reviewThreads GraphQL response into (resolved, total).
    /// `first:100` bounds the node page; `totalCount` stays exact beyond it.
    static func parseThreads(_ data: Data) -> (resolved: Int, total: Int)? {
        struct Response: Decodable {
            struct Threads: Decodable {
                struct Node: Decodable { var isResolved: Bool }
                var totalCount: Int
                var nodes: [Node]
            }
            struct PR: Decodable { var reviewThreads: Threads }
            struct Repo: Decodable { var pullRequest: PR? }
            struct DataBox: Decodable { var repository: Repo? }
            var data: DataBox?
        }
        guard let threads = try? JSONDecoder().decode(Response.self, from: data)
            .data?.repository?.pullRequest?.reviewThreads
        else { return nil }
        return (threads.nodes.count { $0.isResolved }, threads.totalCount)
    }

    /// Decodes one `gh pr view --json` payload. Static + input-driven so tests
    /// can feed canned JSON without a subprocess.
    static func parse(_ data: Data, url: String, now: Date) -> PullRequestStatus? {
        guard let view = try? JSONDecoder().decode(GhPRView.self, from: data) else {
            return nil
        }
        let checks: [PRCheck] = (view.statusCheckRollup ?? []).map { check in
            // CheckRun reports conclusion once COMPLETED; StatusContext only
            // has state. Either way one word decides the bucket. An empty
            // conclusion means still running — fall through to status.
            let verdict = [check.conclusion, check.state, check.status]
                .compactMap { $0 }
                .first { !$0.isEmpty } ?? ""
            let result: String
            switch verdict {
            case "SUCCESS", "NEUTRAL", "SKIPPED":
                result = "pass"
            case "FAILURE", "ERROR", "CANCELLED", "TIMED_OUT", "ACTION_REQUIRED",
                "STARTUP_FAILURE":
                result = "fail"
            default:
                result = "pending"
            }
            let base = check.name ?? check.context ?? "check"
            let name = check.workflowName.map { "\($0) / \(base)" } ?? base
            return PRCheck(
                name: name,
                result: result,
                detail: verdict.isEmpty ? nil : verdict,
                url: check.detailsUrl ?? check.targetUrl)
        }
        let passed = checks.count { $0.result == "pass" }
        let failed = checks.count { $0.result == "fail" }
        let pending = checks.count { $0.result == "pending" }
        return PullRequestStatus(
            url: url,
            number: view.number,
            title: view.title,
            state: view.state,
            isDraft: view.isDraft ?? false,
            reviewDecision: (view.reviewDecision?.isEmpty ?? true) ? nil : view.reviewDecision,
            mergeable: view.mergeable,
            mergeStateStatus: view.mergeStateStatus,
            additions: view.additions ?? 0,
            deletions: view.deletions ?? 0,
            changedFiles: view.changedFiles ?? 0,
            commentCount: view.comments?.count ?? 0,
            reviewCount: view.reviews?.count ?? 0,
            checksPassed: passed,
            checksFailed: failed,
            checksPending: pending,
            checks: checks.isEmpty ? nil : checks,
            fetchedAt: now
        )
    }
}
