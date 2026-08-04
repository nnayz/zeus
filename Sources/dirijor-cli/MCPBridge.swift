import DirijorCore
import DirijorProtocol
import Foundation

/// Maps MCP tool calls to daemon control requests. Called from the MCP stdio
/// server's tool handler. Synchronous by nature (uses DaemonConn); the caller
/// runs it off the cooperative pool.
enum MCPBridge {
    struct BridgeError: Error, CustomStringConvertible {
        var message: String
        var description: String { message }
    }

    /// Dispatches one tool call. Throws on bad arguments or daemon errors (the
    /// MCP server turns thrown errors into isError results).
    static func handle(tool: String, args: JSONValue) throws -> JSONValue {
        switch tool {
        case "spawn_agent": return try spawnAgent(args)
        case "list_agents": return try listAgents(filter: nil)
        case "get_status": return try getStatus(args)
        case "send_prompt": return try sendPrompt(args)
        case "wait_for_agent": return try waitForAgent(args)
        case "read_output": return try readOutput(args)
        case "get_artifacts": return try getArtifacts(args)
        case "create_worktree": return try createWorktree(args)
        case "list_worktrees": return try listWorktrees(args)
        case "remove_worktree": return try removeWorktree(args)
        case "release_agent": return try releaseAgent(args)
        case "test_run": return try testRun(args)
        case "whoami": return try whoami(args)
        case "list_children": return try listChildren(args)
        case "wait_for_children": return try waitForChildren(args)
        case "summarize_children": return try summarizeChildren(args)
        case "report_to_parent": return try reportToParent(args)
        case "browser": return try browser(args)
        default:
            throw BridgeError(message: "unknown tool: \(tool)")
        }
    }

    // MARK: - Connection

    static func withConn<T>(readTimeout: TimeInterval = 3, _ body: (DaemonConn) throws -> T) throws -> T {
        let conn = try DaemonConn.connect()
        defer { conn.close() }
        return try body(conn)
    }

    // MARK: - Argument helpers

    static func requireString(_ args: JSONValue, _ key: String) throws -> String {
        guard let value = args[key]?.stringValue, !value.isEmpty else {
            throw BridgeError(message: "missing required argument: \(key)")
        }
        return value
    }

    static func optNumber(_ args: JSONValue, _ key: String) -> Double? {
        if case .number(let n)? = args[key] { return n }
        return nil
    }

    static func optBool(_ args: JSONValue, _ key: String) -> Bool? {
        if case .bool(let b)? = args[key] { return b }
        return nil
    }

    // MARK: - Tools

    private static func spawnAgent(_ args: JSONValue) throws -> JSONValue {
        let kindStr = try requireString(args, "kind")
        // Resolved against the manifest catalog (id, shortLabel, or alias), so
        // an agent added as a file drop is spawnable by name from MCP without
        // touching this switch — which is what it used to be.
        guard let descriptor = AgentCatalog.shared.resolve(name: kindStr) else {
            let known = AgentCatalog.shared.ordered.map(\.shortLabel).joined(separator: ", ")
            throw BridgeError(message: "invalid kind: \(kindStr) (known: \(known))")
        }
        let kind = AgentKind(id: descriptor.id)
        let cwd = try requireString(args, "cwd")
        let params = SessionSpawnParams(
            kind: kind,
            cwd: cwd,
            newWorktree: optBool(args, "worktree"),
            title: args["name"]?.stringValue,
            initialPrompt: args["prompt"]?.stringValue,
            parent: CLISupport.sessionID(),  // parent = the calling agent, for free attribution
            host: args["host"]?.stringValue
        )
        return try withConn { try $0.request(Method.sessionSpawn, params: params) }
    }

    static func compact(_ record: SessionRecord) -> JSONValue {
        var obj: [String: JSONValue] = [
            "id": .string(record.id.rawValue),
            "kind": .string(record.kind.shortLabel),
            "title": .string(record.title),
            "status": .string(record.status.label),
            "cwd": .string(record.cwd),
        ]
        if let parent = record.parent { obj["parent"] = .string(parent.rawValue) }
        if let host = record.host { obj["host"] = .string(host) }
        return .object(obj)
    }

    static func fetchSessions() throws -> [SessionRecord] {
        let result = try withConn { conn -> JSONValue in
            try conn.request(Method.sessionList, params: JSONValue.object([:]))
        }
        return try result.decoded(as: SessionListResult.self).sessions
    }

    private static func listAgents(filter: SessionID?) throws -> JSONValue {
        var sessions = try fetchSessions()
        if let filter { sessions = sessions.filter { $0.id == filter } }
        return .object(["agents": .array(sessions.map(compact))])
    }

    private static func getStatus(_ args: JSONValue) throws -> JSONValue {
        let id = SessionID(rawValue: try requireString(args, "session_id"))
        let sessions = try fetchSessions()
        guard let record = sessions.first(where: { $0.id == id }) else {
            throw BridgeError(message: "no such session: \(id.rawValue)")
        }
        return compact(record)
    }

    static func sendPrompt(_ args: JSONValue) throws -> JSONValue {
        let id = SessionID(rawValue: try requireString(args, "session_id"))
        let text = try requireString(args, "text")
        let submit: Bool = { if case .bool(let b)? = args["submit"] { return b }; return true }()

        let lineage = SessionLineage.current(sessions: try fetchSessions())
        let relation = lineage.relation(to: id)
        guard relation != .caller else {
            throw BridgeError(
                message:
                    "send_prompt cannot target the calling session (\(id.rawValue)) — that types into your own terminal and would feed your output back to yourself. Just answer normally."
            )
        }
        let delivered = lineage.frame(text, relation: relation)

        let params = SendTextParams(sessionID: id, text: delivered, submit: submit)
        _ = try withConn { try $0.request(Method.sessionSendText, params: params) }
        return .object([
            "ok": .bool(true),
            "relation": .string(relation.rawValue),
            "attributed": .bool(delivered != text),
        ])
    }

    /// Backed by the daemon's `events.wait` long poll (never a poll loop): the
    /// tool returns within a status tick of the transition.
    ///
    /// The target string is passed through verbatim now that the daemon resolves
    /// the alias vocabulary itself (`SessionStatus.satisfies(waitTarget:)`). The
    /// local switch this replaced silently rewrote anything it didn't recognize
    /// to "idle", so `until: "blocked"` waited for the opposite of what it said.
    private static func waitForAgent(_ args: JSONValue) throws -> JSONValue {
        let id = SessionID(rawValue: try requireString(args, "session_id"))
        let until = [args["until"]?.stringValue ?? "done"]
        let timeoutS = optNumber(args, "timeout_s") ?? 600
        let timeoutMs = Int(timeoutS * 1000)
        let params = EventsWaitParams(sessionID: id, until: until, timeoutMs: timeoutMs)
        // Give the socket read a margin beyond the daemon-side long-poll timeout.
        let readTimeout = timeoutS + 5
        return try withConn(readTimeout: readTimeout) {
            try $0.request(Method.eventsWait, params: params, readTimeout: readTimeout)
        }
    }

    private static func readOutput(_ args: JSONValue) throws -> JSONValue {
        let id = SessionID(rawValue: try requireString(args, "session_id"))
        let mode = args["mode"]?.stringValue ?? "screen"
        let params = SessionIDParams(sessionID: id)
        // v1: both "screen" and "tail" map to the daemon's screen snapshot.
        var result = try withConn { try $0.request(Method.sessionReadScreen, params: params) }
        if case .object(var obj) = result {
            if mode == "tail" {
                obj["note"] = .string("tail mode returns the current screen in v1")
            }
            result = .object(obj)
        }
        return result
    }

    private static func getArtifacts(_ args: JSONValue) throws -> JSONValue {
        let id = SessionID(rawValue: try requireString(args, "session_id"))
        let sessions = try fetchSessions()
        guard let record = sessions.first(where: { $0.id == id }) else {
            throw BridgeError(message: "no such session: \(id.rawValue)")
        }
        var prByURL: [String: PullRequestStatus] = [:]
        for status in record.pullRequests ?? [] where prByURL[status.url] == nil {
            prByURL[status.url] = status
        }
        let artifacts: [JSONValue] = (record.artifacts ?? []).map { artifact in
            var obj: [String: JSONValue] = [
                "kind": .string(artifact.kind.rawValue),
                "url": .string(artifact.url),
            ]
            if artifact.kind == .pullRequest, let pr = prByURL[artifact.url] {
                obj["pr"] = prJSON(pr)
            }
            return .object(obj)
        }
        let ports: [JSONValue] = (record.listeningPorts ?? []).map { port in
            .object([
                "port": .number(Double(port.port)),
                "process": .string(port.processName),
            ])
        }
        return .object([
            "artifacts": .array(artifacts),
            "listeningPorts": .array(ports),
        ])
    }

    /// GitHub stats the daemon's PR monitor polled via `gh`. `overall` is the
    /// derived rollup (ready / conflicts / checks failing / merged / …); the
    /// raw fields keep GitHub's own vocabulary.
    private static func prJSON(_ pr: PullRequestStatus) -> JSONValue {
        let runs: [JSONValue] = (pr.checks ?? []).map { check in
            var run: [String: JSONValue] = [
                "name": .string(check.name),
                "result": .string(check.result),
            ]
            if let detail = check.detail { run["detail"] = .string(detail) }
            return .object(run)
        }
        let checks: JSONValue = .object([
            "passed": .number(Double(pr.checksPassed)),
            "failed": .number(Double(pr.checksFailed)),
            "pending": .number(Double(pr.checksPending)),
            "runs": .array(runs),
        ])
        var obj: [String: JSONValue] = [
            "number": .number(Double(pr.number)),
            "state": .string(pr.state),
            "overall": .string(pr.overall),
            "draft": .bool(pr.isDraft),
            "additions": .number(Double(pr.additions)),
            "deletions": .number(Double(pr.deletions)),
            "changed_files": .number(Double(pr.changedFiles)),
            "comments": .number(Double(pr.commentCount)),
            "reviews": .number(Double(pr.reviewCount)),
            "checks": checks,
            "fetched_at": .string(ISO8601DateFormatter().string(from: pr.fetchedAt)),
        ]
        if let total = pr.totalThreads {
            obj["review_threads"] = .object([
                "resolved": .number(Double(pr.resolvedThreads ?? 0)),
                "total": .number(Double(total)),
            ])
        }
        if let title = pr.title { obj["title"] = .string(title) }
        if let decision = pr.reviewDecision { obj["review_decision"] = .string(decision) }
        if let mergeable = pr.mergeable { obj["mergeable"] = .string(mergeable) }
        if let mergeState = pr.mergeStateStatus { obj["merge_state"] = .string(mergeState) }
        return .object(obj)
    }

    private static func createWorktree(_ args: JSONValue) throws -> JSONValue {
        let repo = try requireString(args, "repo")
        let params = WorktreeCreateParams(repoPath: repo, branch: args["branch"]?.stringValue)
        return try withConn { try $0.request(Method.worktreeCreate, params: params) }
    }

    private static func listWorktrees(_ args: JSONValue) throws -> JSONValue {
        let repo = try requireString(args, "repo")
        let params = WorktreeListParams(repoPath: repo)
        return try withConn { try $0.request(Method.worktreeList, params: params) }
    }

    private static func removeWorktree(_ args: JSONValue) throws -> JSONValue {
        let repo = try requireString(args, "repo")
        let path = try requireString(args, "path")
        let params = WorktreeRemoveParams(
            repoPath: repo, worktreePath: path, force: optBool(args, "force") ?? false)
        _ = try withConn { try $0.request(Method.worktreeRemove, params: params) }
        return .object(["ok": .bool(true)])
    }

    private static func releaseAgent(_ args: JSONValue) throws -> JSONValue {
        let id = SessionID(rawValue: try requireString(args, "session_id"))

        // Killing yourself or the session that delegated to you is never the
        // intent — it destroys the conversation waiting on this result. Every
        // other target stays allowed, so existing orchestration is unaffected.
        let lineage = SessionLineage.current(sessions: try fetchSessions())
        switch lineage.relation(to: id) {
        case .caller:
            throw BridgeError(
                message:
                    "release_agent cannot terminate the calling session (\(id.rawValue)) — you would be killing the process running this tool."
            )
        case .parent, .ancestor:
            throw BridgeError(
                message:
                    "\(id.rawValue) is the session that spawned you; releasing it would kill the conversation waiting on your result. Use report_to_parent to hand your work back instead."
            )
        default:
            break
        }

        _ = try withConn { try $0.request(Method.sessionKill, params: SessionIDParams(sessionID: id)) }
        return .object(["ok": .bool(true)])
    }

    /// One step of this session's own browser. The session id is never a
    /// parameter: it comes from the environment, so an agent cannot reach into
    /// another agent's browser even by accident, and isolation costs the model
    /// nothing to think about.
    private static func browser(_ args: JSONValue) throws -> JSONValue {
        guard let sessionID = CLISupport.sessionID() else {
            throw BridgeError(
                message:
                    "The browser is scoped to a Dirijor session and DIRIJOR_SESSION_ID is unset — run this from a session hosted by Dirijor."
            )
        }
        let action = try requireString(args, "action")
        // Assigned field by field rather than through one 19-argument
        // initializer: that many optionals in a single call blows up Swift's
        // type checker ("unable to type-check in reasonable time").
        var params = BrowserParams(sessionID: sessionID, action: action)
        params.url = args["url"]?.stringValue
        params.ref = args["ref"]?.stringValue
        params.selector = args["selector"]?.stringValue
        params.text = args["text"]?.stringValue
        params.key = args["key"]?.stringValue
        params.value = args["value"]?.stringValue
        params.what = args["what"]?.stringValue
        params.ms = optNumber(args, "ms")
        params.state = args["state"]?.stringValue
        params.direction = args["direction"]?.stringValue
        params.amount = optNumber(args, "amount")
        params.button = args["button"]?.stringValue
        params.double = optBool(args, "double")
        params.full = optBool(args, "full")
        params.annotate = optBool(args, "annotate")
        params.engine = args["engine"]?.stringValue
        params.profile = args["profile"]?.stringValue
        // Page loads and slow selectors routinely outrun the default 3s RPC
        // budget; the sidecar caps each action well inside this.
        let timeout: TimeInterval = 60
        return try withConn(readTimeout: timeout) {
            try $0.request(Method.browserAct, params: params, readTimeout: timeout)
        }
    }

    private static func testRun(_ args: JSONValue) throws -> JSONValue {
        let url = try requireString(args, "url")
        guard case .array(let steps)? = args["steps"] else {
            throw BridgeError(message: "missing required argument: steps (array of step objects)")
        }
        var engines: [String]?
        if case .array(let e)? = args["engines"] { engines = e.compactMap(\.stringValue) }
        let params = TestRunParams(
            url: url, engines: engines, steps: steps,
            observe: args["observe"]?.stringValue,
            baseline: args["baseline"]?.stringValue,
            profile: args["profile"]?.stringValue,
            auth: args["auth"])
        // Browser runs across engines are slow relative to other RPCs; give the
        // socket read a generous margin.
        let timeout: TimeInterval = 180
        return try withConn(readTimeout: timeout) {
            try $0.request(Method.testRun, params: params, readTimeout: timeout)
        }
    }
}
