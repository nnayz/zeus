import DirijorCore
import DirijorProtocol
import Foundation

/// Fleet-shaped tools: who am I, who did I spawn, are they finished, what did
/// they find, and how do I hand a result back. All of it derives from
/// `SessionLineage`, which reads the existing `SessionRecord.parent` — there is
/// no new daemon state behind any of this.
extension MCPBridge {

    // MARK: - Shared argument helpers

    static func optStrings(_ args: JSONValue, _ key: String) -> [String] {
        guard case .array(let items)? = args[key] else { return [] }
        return items.compactMap(\.stringValue).filter { !$0.isEmpty }
    }

    /// Resolves an explicit `session_ids` subset against the caller's actual
    /// children. Naming a session that isn't yours is a mistake worth surfacing
    /// loudly rather than silently ignoring — a typo'd id would otherwise look
    /// like a child that finished instantly.
    static func resolveChildSubset(
        _ args: JSONValue, lineage: SessionLineage, caller: SessionID
    ) throws -> [SessionRecord] {
        let all = lineage.children(of: caller)
        let requested = optStrings(args, "session_ids")
        guard !requested.isEmpty else { return all }
        var out: [SessionRecord] = []
        for raw in requested {
            let id = SessionID(rawValue: raw)
            guard let match = all.first(where: { $0.id == id }) else {
                throw BridgeError(
                    message:
                        "\(raw) is not one of your children — you can only coordinate sessions you spawned. Call list_children to see them."
                )
            }
            out.append(match)
        }
        return out
    }

    static func requireCaller(_ lineage: SessionLineage) throws -> SessionID {
        guard let caller = lineage.caller else {
            throw BridgeError(
                message:
                    "This tool needs to know which session is calling it, and DIRIJOR_SESSION_ID is unset — run it from a session hosted by Dirijor."
            )
        }
        return caller
    }

    /// `compact` plus the fields that only matter when you are reasoning about
    /// a session as a unit of delegated work.
    static func detailed(_ record: SessionRecord, relation: SessionLineage.Relation? = nil)
        -> JSONValue
    {
        guard case .object(var obj) = compact(record) else { return compact(record) }
        if let relation { obj["relation"] = .string(relation.rawValue) }
        if let branch = record.gitBranch { obj["branch"] = .string(branch) }
        if let worktree = record.worktreePath { obj["worktree"] = .string(worktree) }
        obj["created_at"] = .string(ISO8601DateFormatter().string(from: record.createdAt))
        if record.isArchived { obj["archived"] = .bool(true) }
        return .object(obj)
    }

    // MARK: - whoami

    /// Answers "who am I, who spawned me, what did I spawn" from one daemon
    /// round trip, so an agent never has to grep its own environment or guess
    /// at the fleet from `list_agents` output.
    static func whoami(_ args: JSONValue) throws -> JSONValue {
        let lineage = SessionLineage.current(sessions: try fetchSessions())
        guard let caller = lineage.caller else {
            return .object([
                "hosted": .bool(false),
                "note": .string(
                    "Not running inside a Dirijor session (DIRIJOR_SESSION_ID is unset). Lineage tools are unavailable and writes to other sessions are unrestricted and unattributed."
                ),
            ])
        }
        guard let record = lineage.record(caller) else {
            throw BridgeError(
                message:
                    "DIRIJOR_SESSION_ID is \(caller.rawValue) but the daemon has no such session — the record may have been removed."
            )
        }
        var obj: [String: JSONValue] = [
            "hosted": .bool(true),
            "session": detailed(record, relation: .caller),
            "children": .array(lineage.children(of: caller).map { detailed($0, relation: .child) }),
            "descendant_count": .number(Double(lineage.descendants(of: caller).count)),
            "write_policy": .string(SessionLineage.writePolicy),
        ]
        if let parentID = record.parent, let parent = lineage.record(parentID) {
            obj["parent"] = detailed(parent, relation: .parent)
        }
        let ancestors = lineage.ancestors(of: caller)
        if !ancestors.isEmpty {
            // Nearest first, so `ancestors[0] == parent` and the last entry is
            // the root of the delegation chain.
            obj["ancestors"] = .array(ancestors.map { detailed($0, relation: .ancestor) })
        }
        return .object(obj)
    }

    // MARK: - list_children

    static func listChildren(_ args: JSONValue) throws -> JSONValue {
        let lineage = SessionLineage.current(sessions: try fetchSessions())
        let caller = try requireCaller(lineage)
        let recursive = optBool(args, "recursive") ?? false
        let includeExited = optBool(args, "include_exited") ?? true

        var rows = recursive ? lineage.descendants(of: caller) : lineage.children(of: caller)
        if !includeExited { rows = rows.filter { $0.status.isRunning } }

        let items = rows.map { record -> JSONValue in
            let relation: SessionLineage.Relation =
                record.parent == caller ? .child : .descendant
            return detailed(record, relation: relation)
        }
        return .object([
            "children": .array(items),
            "count": .number(Double(items.count)),
        ])
    }

    // MARK: - wait_for_children

    /// Blocks until every named child settles. "Settled" deliberately includes
    /// `needsInput`: a delegate stuck on a permission prompt is finished as far
    /// as the parent is concerned — it needs intervention, and waiting out the
    /// full timeout to discover that wastes the whole budget.
    static func waitForChildren(_ args: JSONValue) throws -> JSONValue {
        let initial = SessionLineage.current(sessions: try fetchSessions())
        let caller = try requireCaller(initial)
        let targets = try resolveChildSubset(args, lineage: initial, caller: caller)
        guard !targets.isEmpty else {
            return .object([
                "settled": .bool(true),
                "children": .array([]),
                "note": .string("You have no child sessions to wait for."),
            ])
        }
        let mode = args["until"]?.stringValue ?? "settled"
        let timeoutS = optNumber(args, "timeout_s") ?? 600
        let deadline = Date().addingTimeInterval(timeoutS)
        let wanted = Set(targets.map(\.id))

        var latest: [SessionRecord] = targets
        var allSettled = false

        /// Re-reads the children and decides whether the wait is over. A record
        /// that vanished mid-wait (removed from the sidebar) can never settle,
        /// so its absence counts as terminal rather than spinning.
        func reassess() throws -> Bool {
            latest = try fetchSessions().filter { wanted.contains($0.id) }
            allSettled = latest.count != wanted.count
                || latest.allSatisfy { hasReached(mode, $0.status) }
            return allSettled
        }

        if try !reassess() {
            // Event-driven, not a 1s poll loop: N waiting parents used to mean N
            // full `session.list` dumps every second for the whole timeout. The
            // subscription is filtered to these children's status transitions
            // server-side, so the daemon only speaks when something moved, and
            // the answer lands within a status tick instead of up to a second
            // late. The list re-read stays — it is the authoritative record, and
            // it now happens per transition rather than per second.
            let conn = try DaemonConn.connect()
            defer { conn.close() }
            let subscribe = EventsSubscribeParams(
                sessions: Array(wanted),
                kinds: [EventName.sessionStatus, EventName.sessionRemoved])
            do {
                try conn.stream(Method.eventsSubscribe, params: subscribe, deadline: deadline) {
                    _, _, _ in try !reassess()
                }
            } catch DaemonError.timeout {
                // Deadline reached; `allSettled` already reflects the last read.
            }
        }

        var out: [String: JSONValue] = [
            "settled": .bool(allSettled),
            "timed_out": .bool(!allSettled),
            "children": .array(latest.map { detailed($0, relation: .child) }),
            "waited_for": .string(mode),
        ]
        // Idle detection reads an agent TUI's prompt state; a plain shell
        // sitting at its prompt stays `working` forever. Waiting on one is a
        // guaranteed timeout, so say so instead of letting the caller conclude
        // the delegate is wedged.
        let shells = latest.filter { $0.effectiveKind == .shell && !hasReached(mode, $0.status) }
        if !shells.isEmpty {
            out["note"] = .string(
                "These children are plain shells, which never report idle: "
                    + shells.map { $0.id.rawValue }.joined(separator: ", ")
                    + ". Wait on agent sessions, or use until:\"exited\" for shells."
            )
        }
        return .object(out)
    }

    private static func hasReached(_ mode: String, _ status: SessionStatus) -> Bool {
        switch mode {
        case "exited":
            if case .exited = status { return true }
            return false
        case "done":
            switch status {
            case .idle, .exited: return true
            default: return false
            }
        default:  // "settled"
            switch status {
            case .idle, .needsInput, .exited: return true
            case .starting, .working, .unknown: return false
            }
        }
    }

    // MARK: - summarize_children

    /// Collects each child's own latest visible evidence. It does not
    /// interpret: the screen tail is what that agent actually printed, so a
    /// parent synthesising results is reading primary output rather than a
    /// summary of a summary.
    static func summarizeChildren(_ args: JSONValue) throws -> JSONValue {
        let lineage = SessionLineage.current(sessions: try fetchSessions())
        let caller = try requireCaller(lineage)
        let targets = try resolveChildSubset(args, lineage: lineage, caller: caller)
        let rows = max(1, min(Int(optNumber(args, "rows") ?? 14), 60))

        let items = targets.map { record -> JSONValue in
            guard case .object(var obj) = detailed(record, relation: .child) else {
                return detailed(record, relation: .child)
            }
            if let tail = try? screenTail(record.id, rows: rows) {
                obj["screen_tail"] = .string(tail)
            } else {
                obj["screen_tail"] = .null
                obj["screen_note"] = .string("no readable screen (session may have exited)")
            }
            if let artifacts = record.artifacts, !artifacts.isEmpty {
                obj["artifacts"] = .array(artifacts.map { .string($0.url) })
            }
            return .object(obj)
        }
        return .object([
            "children": .array(items),
            "count": .number(Double(items.count)),
        ])
    }

    /// Last `rows` non-blank lines of a session's rendered screen. Blank-line
    /// stripping matters: a TUI pads its viewport, so a naive tail is mostly
    /// whitespace and the useful output scrolls out of the budget.
    private static func screenTail(_ id: SessionID, rows: Int) throws -> String {
        let result = try withConn {
            try $0.request(Method.sessionReadScreen, params: SessionIDParams(sessionID: id))
        }
        let text = try result.decoded(as: ReadScreenResult.self).text
        let lines =
            text
            .split(separator: "\n", omittingEmptySubsequences: false)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        return lines.suffix(rows).joined(separator: "\n")
    }

    // MARK: - report_to_parent

    /// Hands a structured result up the delegation chain. Every child may write
    /// to its own parent, so this never needs approval — the parent is the one
    /// session that asked for this work.
    static func reportToParent(_ args: JSONValue) throws -> JSONValue {
        let lineage = SessionLineage.current(sessions: try fetchSessions())
        let caller = try requireCaller(lineage)
        guard let record = lineage.record(caller) else {
            throw BridgeError(message: "no session record for \(caller.rawValue)")
        }
        guard let parentID = record.parent else {
            throw BridgeError(
                message:
                    "This session has no parent — it was started by the user, not delegated by another agent, so there is nobody to report to. Answer in your own terminal instead."
            )
        }
        guard lineage.record(parentID) != nil else {
            throw BridgeError(
                message: "Your parent session (\(parentID.rawValue)) is gone; the report has nowhere to land."
            )
        }

        let status = args["status"]?.stringValue ?? "update"
        guard ChildReport.statuses.contains(status) else {
            throw BridgeError(
                message:
                    "invalid status: \(status) — expected one of \(ChildReport.statuses.joined(separator: ", "))"
            )
        }
        let report = ChildReport(
            status: status,
            summary: try requireString(args, "summary"),
            details: args["details"]?.stringValue,
            blockers: optStrings(args, "blockers"),
            questions: optStrings(args, "questions"),
            nextSteps: optStrings(args, "next_steps"),
            changedPaths: optStrings(args, "changed_paths"),
            artifacts: optStrings(args, "artifacts"),
            proof: optStrings(args, "proof")
        )
        let rendered = report.rendered(from: record, senderID: caller)
        let submit = optBool(args, "submit") ?? true
        _ = try withConn {
            try $0.request(
                Method.sessionSendText,
                params: SendTextParams(sessionID: parentID, text: rendered, submit: submit))
        }
        return .object([
            "ok": .bool(true),
            "parent": .string(parentID.rawValue),
            "status": .string(status),
            "delivered": .string(rendered),
        ])
    }
}
