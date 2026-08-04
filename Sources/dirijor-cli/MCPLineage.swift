import DirijorCore
import DirijorProtocol
import Foundation

/// The spawn graph, read from `SessionRecord.parent` — which `spawn_agent`
/// already stamps with the calling session's id, so the graph costs nothing to
/// maintain.
///
/// Reads across the graph are open: an agent inspecting a sibling's screen is
/// harmless and often the point. Writes are not. A write to a session that
/// isn't your parent or your own child is a message between strangers, and the
/// receiving agent has no way to tell it apart from its user talking — so those
/// get a provenance header. Relation is computed once per tool call and
/// returned to the caller, which teaches the model the shape of the fleet
/// without it having to ask.
struct SessionLineage {
    enum Relation: String {
        case caller = "self"
        case parent
        case child
        case ancestor
        case descendant
        case sibling
        case unrelated

        /// Your parent and your direct children are the delegation channel —
        /// both ends already know who the other is, and extra framing just
        /// confuses an agent mid-task. Everyone else gets attributed.
        var deliversVerbatim: Bool { self == .parent || self == .child }
    }

    let records: [SessionRecord]
    /// The session this MCP server is running inside, from `DIRIJOR_SESSION_ID`.
    /// nil when the tools are driven from outside Dirijor (a plain shell, a
    /// script) — such a caller has no place in the graph, so it is trusted and
    /// its writes land verbatim, exactly as before this layer existed.
    let caller: SessionID?

    private let byID: [SessionID: SessionRecord]
    private let childrenByParent: [SessionID: [SessionRecord]]

    init(records: [SessionRecord], caller: SessionID?) {
        self.records = records
        self.caller = caller
        self.byID = Dictionary(records.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        self.childrenByParent = Dictionary(
            grouping: records.compactMap { $0.parent == nil ? nil : $0 },
            by: { $0.parent! }
        ).mapValues { $0.sorted { $0.createdAt < $1.createdAt } }
    }

    func record(_ id: SessionID) -> SessionRecord? { byID[id] }

    var callerRecord: SessionRecord? { caller.flatMap { byID[$0] } }

    func children(of id: SessionID) -> [SessionRecord] { childrenByParent[id] ?? [] }

    /// Breadth-first descendants. Parent ids come off disk and a corrupted or
    /// hand-edited state file could describe a cycle; the visited set means a
    /// bad record degrades to a short answer instead of hanging the daemon call.
    func descendants(of id: SessionID) -> [SessionRecord] {
        var seen: Set<SessionID> = [id]
        var queue = children(of: id)
        var out: [SessionRecord] = []
        while let next = queue.first {
            queue.removeFirst()
            guard seen.insert(next.id).inserted else { continue }
            out.append(next)
            queue.append(contentsOf: children(of: next.id))
        }
        return out
    }

    /// Walk to the root, nearest ancestor first. Same cycle guard as above.
    func ancestors(of id: SessionID) -> [SessionRecord] {
        var seen: Set<SessionID> = [id]
        var out: [SessionRecord] = []
        var cursor = byID[id]?.parent
        while let current = cursor, seen.insert(current).inserted {
            guard let record = byID[current] else { break }
            out.append(record)
            cursor = record.parent
        }
        return out
    }

    func relation(to target: SessionID) -> Relation {
        guard let caller else { return .unrelated }
        if caller == target { return .caller }
        if byID[caller]?.parent == target { return .parent }
        if byID[target]?.parent == caller { return .child }
        if ancestors(of: caller).contains(where: { $0.id == target }) { return .ancestor }
        if descendants(of: caller).contains(where: { $0.id == target }) { return .descendant }
        if let mine = byID[caller]?.parent, byID[target]?.parent == mine { return .sibling }
        return .unrelated
    }

    /// Attribution for a cross-session write. Returns the text unchanged when
    /// the relation delivers verbatim or when there is no caller to attribute
    /// it to; otherwise prefixes one line naming the sender so the receiving
    /// agent can reply to that id instead of guessing who asked.
    func frame(_ text: String, relation: Relation) -> String {
        guard !relation.deliversVerbatim, let caller else { return text }
        let title = callerRecord?.title
        let who = title.map { "id:\(caller.rawValue) (\($0))" } ?? "id:\(caller.rawValue)"
        return "[message from \(who), channel: dirijor — reply with send_prompt to that id]\n\n\(text)"
    }

    /// Snapshot of the graph as it stands right now.
    static func current(sessions: [SessionRecord]) -> SessionLineage {
        SessionLineage(records: sessions, caller: CLISupport.sessionID())
    }

    /// Stated once, returned from `whoami`, so an agent learns the rules from
    /// the tool surface instead of discovering them by being refused.
    static let writePolicy = """
        Reads are open across all sessions. Writes to your parent or your direct \
        children are delivered verbatim; writes to anyone else are prefixed with a \
        provenance header naming you, so the receiving agent knows an unrelated \
        session is talking to it. You cannot send_prompt to yourself, and \
        release_agent refuses to kill you or any of your ancestors.
        """
}

// MARK: - Structured child → parent reports

/// The shape of a delegate's report back to whoever spawned it. Freeform text
/// through `send_prompt` makes the parent re-derive structure the child already
/// knew; these fields carry it across intact, and the empty ones are dropped so
/// a one-line "done" stays one line.
struct ChildReport {
    var status: String
    var summary: String
    var details: String?
    var blockers: [String] = []
    var questions: [String] = []
    var nextSteps: [String] = []
    var changedPaths: [String] = []
    var artifacts: [String] = []
    var proof: [String] = []

    /// Rendered for a terminal, not a parser: the parent is a language model
    /// reading its own screen, so plain labelled sections beat JSON.
    func rendered(from sender: SessionRecord?, senderID: SessionID?) -> String {
        var lines: [String] = []
        let who: String
        switch (senderID, sender?.title) {
        case let (id?, title?): who = "id:\(id.rawValue) (\(title))"
        case let (id?, nil): who = "id:\(id.rawValue)"
        default: who = "an unidentified session"
        }
        lines.append("[report from \(who) · status: \(status)]")
        lines.append("")
        lines.append("Summary: \(summary)")
        if let details, !details.isEmpty {
            lines.append("")
            lines.append(details)
        }
        appendSection(&lines, "Blockers", blockers)
        appendSection(&lines, "Questions", questions)
        appendSection(&lines, "Next steps", nextSteps)
        appendSection(&lines, "Changed", changedPaths)
        appendSection(&lines, "Artifacts", artifacts)
        appendSection(&lines, "Proof", proof)
        return lines.joined(separator: "\n")
    }

    private func appendSection(_ lines: inout [String], _ title: String, _ items: [String]) {
        let kept = items.filter { !$0.isEmpty }
        guard !kept.isEmpty else { return }
        lines.append("")
        lines.append("\(title):")
        lines.append(contentsOf: kept.map { "- \($0)" })
    }

    static let statuses = ["update", "done", "blocked", "failed"]
}
