import ArgumentParser
import DirijorCore
import DirijorProtocol
import Foundation

/// `dirijor events subscribe|wait` — the piece that turns the daemon from
/// something you poll into something you can drive. Everything else in the CLI
/// answers "what is true now"; these two answer "tell me when it changes".
struct Events: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "events",
        abstract: "Stream or await daemon events.",
        subcommands: [EventsSubscribe.self, EventsWait.self],
        defaultSubcommand: EventsSubscribe.self
    )
}

struct EventsSubscribe: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "subscribe",
        abstract: "Stream events until interrupted.",
        discussion: """
            Filters are applied daemon-side, so a narrow subscription costs the \
            daemon nothing to serve and can't be starved by traffic you didn't \
            ask for. If this process stops reading, the daemon bounds its queue \
            and emits an `events.dropped` marker naming the missed seq range \
            rather than buffering without limit.

            Event kinds: \(EventName.all.joined(separator: ", ")).
            """)

    @OptionGroup var output: OutputOptions

    @Option(name: .long, help: "Only events for this session; repeatable.")
    var session: [String] = []

    @Option(name: .long, help: "Only these event kinds; repeatable.")
    var kind: [String] = []

    @Option(name: .long, help: "Replay buffered events with a higher seq first.")
    var sinceSeq: UInt64?

    @Option(name: .long, help: "Stop after this many events (default: never).")
    var count: Int?

    func validate() throws {
        for name in kind where !EventName.all.contains(name) {
            throw ValidationError(
                "unknown event kind \"\(name)\"; expected one of: "
                    + EventName.all.joined(separator: ", "))
        }
    }

    func run() throws {
        // Ids are resolved up front so `--session ab12` behaves like every other
        // command's target, and so a typo fails now rather than silently
        // matching nothing for the rest of the stream.
        var sessions: [SessionID] = []
        if !session.isEmpty {
            let known = try CLIClient.sessions().sessions
            sessions = try session.map { try CLIClient.resolve($0, in: known).id }
        }
        let params = EventsSubscribeParams(
            sinceSeq: sinceSeq,
            sessions: sessions.isEmpty ? nil : sessions,
            kinds: kind.isEmpty ? nil : kind)

        let conn = try CLIClient.connect()
        defer { conn.close() }
        var seen = 0
        do {
            try conn.stream(Method.eventsSubscribe, params: params) { name, seq, params in
                if output.json {
                    output.emit(.object([
                        "event": .string(name), "seq": .number(Double(seq)), "params": params,
                    ]))
                } else {
                    print(EventRendering.line(name: name, seq: seq, params: params))
                }
                // stdout may be a pipe into `head`; flushing per event is what
                // makes `dirijor events subscribe | head -5` terminate promptly
                // instead of sitting in a 4KB buffer.
                fflush(stdout)
                seen += 1
                return count.map { seen < $0 } ?? true
            }
        } catch let error as DaemonError {
            throw CLIClient.translate(error)
        }
    }
}

struct EventsWait: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "wait",
        abstract: "Block until one matching event, print it, and exit.",
        discussion: """
            Exits 2 when the timeout expires with nothing matching, so \
            `dirijor events wait --until needs-input --timeout 60` is a usable \
            shell condition.
            """)

    @OptionGroup var output: OutputOptions

    @Option(name: .long, help: "Scope to one session.")
    var session: String?

    @Option(name: .long, help: "Session status to wait for; repeatable. One of: \(SessionStatus.waitTargets.joined(separator: ", ")).")
    var until: [String] = []

    @Option(name: .long, help: "Event kind to wait for; repeatable.")
    var kind: [String] = []

    @Option(name: .long, help: "Seconds before giving up.")
    var timeout: Double = 600

    func validate() throws {
        guard !until.isEmpty || !kind.isEmpty else {
            throw ValidationError("pass at least one --until <status> or --kind <event>")
        }
        for name in kind where !EventName.all.contains(name) {
            throw ValidationError(
                "unknown event kind \"\(name)\"; expected one of: "
                    + EventName.all.joined(separator: ", "))
        }
        if !until.isEmpty, session == nil {
            throw ValidationError("--until needs --session (a status is a property of one session)")
        }
    }

    func run() throws {
        var sessionID: SessionID?
        if let session {
            sessionID = try CLIClient.resolve(session, in: CLIClient.sessions().sessions).id
        }
        let params = EventsWaitParams(
            sessionID: sessionID,
            until: until,
            kinds: kind.isEmpty ? nil : kind,
            timeoutMs: Int(timeout * 1000))
        let budget = timeout + 5
        let result = try CLIClient.withConn { conn in
            try conn.request(Method.eventsWait, params: params, readTimeout: budget)
                .decoded(as: EventsWaitResult.self)
        }

        if output.json {
            output.emit(encoding: result)
        } else if let event = result.event {
            print(EventRendering.line(name: event.name, seq: event.seq, params: event.params))
        } else if let session = result.session {
            print("\(session.id.rawValue)  \(session.status.label)  \(session.title)")
        } else {
            print("timed out")
        }
        if result.timedOut { throw CLIExit.timeout }
    }
}

/// Human-readable one-liners for streamed events. Deliberately lossy — anyone
/// who needs the whole payload passes `--json`.
enum EventRendering {
    static func line(name: String, seq: UInt64, params: JSONValue) -> String {
        let head = padColumn("\(seq)", 6) + padColumn(name, 22)
        switch name {
        case EventName.sessionStatus:
            let id = params["id"]?.stringValue ?? "?"
            let label = params["label"]?.stringValue ?? "?"
            let blocker = params["needsInput"]?["summary"]?.stringValue
            return head + "\(id)  \(label)" + (blocker.map { "  — \($0)" } ?? "")
        case EventName.sessionNeedsInput:
            let id = params["id"]?.stringValue ?? "?"
            let detail = params["needsInput"]
            let summary = detail?["summary"]?.stringValue ?? ""
            let risk = detail?["riskHint"]?.stringValue ?? "neutral"
            return head + "\(id)  [\(risk)] \(summary)"
        case EventName.sessionArtifact:
            let id = params["id"]?.stringValue ?? "?"
            let kind = params["kind"]?.stringValue ?? "?"
            if let url = params["url"]?.stringValue { return head + "\(id)  \(kind)  \(url)" }
            if case .number(let port)? = params["port"] {
                return head + "\(id)  port  localhost:\(Int(port))"
            }
            return head + "\(id)  \(kind)"
        case EventName.sessionOutput:
            return head + (params["id"]?.stringValue ?? "?")
        case EventName.sessionSpawned, EventName.sessionUpdated, EventName.sessionArchived:
            let id = params["id"]?.stringValue ?? "?"
            let title = params["title"]?.stringValue ?? ""
            return head + "\(id)  \(title)"
        case EventName.sessionRemoved:
            return head + (params["id"]?.stringValue ?? "?")
        case EventName.worktreeCreated, EventName.worktreeRemoved:
            return head + (params["path"]?.stringValue ?? "?")
        case EventName.eventsDropped:
            let dropped = params["dropped"].flatMap { value -> Int? in
                if case .number(let n) = value { return Int(n) }
                return nil
            }
            return head + "lost \(dropped ?? 0) events — re-read state"
        default:
            return head + CLISupport.encodeCompact(params)
        }
    }
}
