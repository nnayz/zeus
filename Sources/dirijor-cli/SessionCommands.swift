import ArgumentParser
import DirijorCore
import DirijorProtocol
import Foundation

/// `dirijor session <action> [target] [options]`.
///
/// The resource/action shape exists so the surface can grow without the top
/// level turning into a flat pile of verbs, and so `--help` at each level tells
/// you what you can do to that resource.
struct Session: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "session",
        abstract: "Inspect and drive agent sessions.",
        subcommands: [
            SessionList.self, SessionGet.self, SessionRead.self, SessionSend.self,
            SessionWait.self, SessionSpawn.self, SessionRelease.self, SessionArchive.self,
        ],
        defaultSubcommand: SessionList.self
    )
}

// MARK: - list

struct SessionList: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "list", abstract: "List sessions.")

    @OptionGroup var output: OutputOptions

    @Option(name: .long, help: "Only sessions whose status starts with this (idle, working, needsInput, exited).")
    var status: String?

    @Flag(name: .long, help: "Include archived sessions.")
    var all = false

    func run() throws {
        try SessionListing.run(includeArchived: all, statusPrefix: status, output: output)
    }
}

/// The listing body lives outside the command so the top-level `status` alias
/// can reuse it. A `ParsableCommand` cannot be constructed by hand — its
/// property wrappers are only populated by the parser — so "alias" has to mean
/// a shared function, not a delegated `run()`.
enum SessionListing {
    static func run(includeArchived: Bool, statusPrefix: String?, output: OutputOptions) throws {
        var sessions = try CLIClient.sessions().sessions.sorted { $0.createdAt < $1.createdAt }
        if !includeArchived { sessions = sessions.filter { !$0.isArchived } }
        if let statusPrefix {
            sessions = sessions.filter { $0.status.label.hasPrefix(statusPrefix) }
        }
        if output.json {
            output.emit(
                .object([
                    "sessions": .array(sessions.map { (try? JSONValue(encoding: $0)) ?? .null })
                ]))
            return
        }
        SessionRendering.table(sessions)
    }
}

// MARK: - get

struct SessionGet: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "get", abstract: "Show one session in full.")

    @OptionGroup var output: OutputOptions

    @Argument(help: "Session id, id prefix, or a unique part of its title.")
    var session: String

    func run() throws {
        let record = try CLIClient.resolve(session, in: CLIClient.sessions().sessions)
        if output.json {
            output.emit(encoding: record)
            return
        }
        SessionRendering.detail(record)
    }
}

// MARK: - read

struct SessionRead: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "read", abstract: "Read a session's terminal output.")

    @OptionGroup var output: OutputOptions

    @Argument(help: "Session id, id prefix, or a unique part of its title.")
    var session: String

    @Option(name: .long, help: "\"screen\" (the live viewport) or \"scrollback\" (full history).")
    var source: String = "screen"

    @Option(name: .long, help: "Return only the last N lines.")
    var lines: Int?

    func run() throws {
        let record = try CLIClient.resolve(session, in: CLIClient.sessions().sessions)
        let params = SessionIDParams(sessionID: record.id)

        var text: [String]
        var cols = 0
        var rows = 0
        switch source {
        case "screen":
            let screen = try CLIClient.withConn { conn in
                try conn.request(Method.sessionReadScreen, params: params)
                    .decoded(as: ReadScreenResult.self)
            }
            text = screen.text.components(separatedBy: "\n")
            cols = screen.cols
            rows = screen.rows
        case "scrollback":
            let scrollback = try CLIClient.withConn { conn in
                try conn.request(Method.sessionReadScrollback, params: params)
                    .decoded(as: ReadScrollbackResult.self)
            }
            text = scrollback.lines
            cols = scrollback.cols
            rows = scrollback.rows
        default:
            throw ValidationError("--source must be \"screen\" or \"scrollback\"")
        }
        if let lines, lines > 0, text.count > lines {
            text = Array(text.suffix(lines))
        }

        if output.json {
            output.emit(.object([
                "id": .string(record.id.rawValue),
                "source": .string(source),
                "cols": .number(Double(cols)),
                "rows": .number(Double(rows)),
                "lines": .array(text.map { .string($0) }),
            ]))
            return
        }
        for line in text { print(line) }
    }
}

// MARK: - send

struct SessionSend: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "send", abstract: "Type text into a session and (by default) press Enter.")

    @OptionGroup var output: OutputOptions

    @Argument(help: "Session id, id prefix, or a unique part of its title.")
    var session: String

    @Argument(parsing: .remaining, help: "Text to send. Omit to read it from stdin.")
    var text: [String] = []

    @Flag(name: .long, help: "Type without pressing Enter (e.g. to fill a picker).")
    var noSubmit = false

    func run() throws {
        let record = try CLIClient.resolve(session, in: CLIClient.sessions().sessions)
        var body = text.joined(separator: " ")
        if body.isEmpty {
            // Reading from stdin is what makes `… | dirijor session send s_x` work,
            // which is how a prompt longer than a shell line actually gets sent.
            body = String(decoding: CLISupport.readStdin(), as: UTF8.self)
                .trimmingCharacters(in: .newlines)
        }
        guard !body.isEmpty else { throw ValidationError("nothing to send") }

        _ = try CLIClient.withConn { conn in
            try conn.request(
                Method.sessionSendText,
                params: SendTextParams(sessionID: record.id, text: body, submit: !noSubmit))
        }
        if output.json {
            output.emit(.object(["ok": .bool(true), "id": .string(record.id.rawValue)]))
        } else {
            print("sent \(body.count) chars to \(record.id.rawValue)")
        }
    }
}

// MARK: - wait

struct SessionWait: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "wait",
        abstract: "Block until a session reaches a status, then print it.",
        discussion: """
            Backed by the daemon's event stream, not polling: the call returns \
            within a tick of the transition. Exits 2 on timeout, so \
            `dirijor session wait s_x --until done; or echo late` works.
            """)

    @OptionGroup var output: OutputOptions

    @Argument(help: "Session id, id prefix, or a unique part of its title.")
    var session: String

    @Option(name: .long, help: "Status to wait for; repeatable. One of: \(SessionStatus.waitTargets.joined(separator: ", ")).")
    var until: [String] = ["done"]

    @Option(name: .long, help: "Seconds before giving up.")
    var timeout: Double = 600

    func run() throws {
        let record = try CLIClient.resolve(session, in: CLIClient.sessions().sessions)
        let params = EventsWaitParams(
            sessionID: record.id, until: until, timeoutMs: Int(timeout * 1000))
        // The socket read has to outlive the daemon's own long poll, or the CLI
        // would report a timeout the daemon never had.
        let budget = timeout + 5
        let result = try CLIClient.withConn { conn in
            try conn.request(Method.eventsWait, params: params, readTimeout: budget)
                .decoded(as: EventsWaitResult.self)
        }

        if output.json {
            output.emit(encoding: result)
        } else if let session = result.session {
            print("\(session.id.rawValue)  \(session.status.label)  \(session.title)")
        }
        if result.timedOut { throw CLIExit.timeout }
    }
}

// MARK: - spawn

struct SessionSpawn: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "spawn", abstract: "Open a new session (tab) in Dirijor.")

    @OptionGroup var output: OutputOptions

    @Argument(help: "Agent to run: \(AgentKind.cliNames.joined(separator: ", ")), or any command.")
    var kind: String

    @Option(name: .long, help: "Working directory (default: the current one).")
    var cwd: String?

    @Option(name: .long, help: "Session title.")
    var title: String?

    @Option(name: .long, help: "Prompt to send once the agent is ready.")
    var prompt: String?

    @Flag(name: .long, help: "Create a fresh git worktree off cwd and run there.")
    var worktree = false

    @Option(name: .long, help: "Branch name for --worktree.")
    var branch: String?

    @Option(name: .long, help: "Host id from hosts.json to spawn on remotely.")
    var host: String?

    func run() throws {
        let params = SessionSpawnParams(
            kind: AgentKind.parse(kind),
            cwd: cwd ?? FileManager.default.currentDirectoryPath,
            newWorktree: worktree ? true : nil,
            worktreeBranch: branch,
            title: title,
            initialPrompt: prompt,
            // Attribution comes free when the CLI runs inside a Dirijor session.
            parent: CLISupport.sessionID(),
            host: host)
        // Spawning shells out to git (worktree) and ssh (remote); the default
        // 3s RPC budget is not enough for either.
        let record = try CLIClient.withConn { conn in
            try conn.request(Method.sessionSpawn, params: params, readTimeout: 60)
                .decoded(as: SessionRecord.self)
        }
        if output.json {
            output.emit(encoding: record)
        } else {
            print("\(record.id.rawValue)  \(record.kind.shortLabel)  \(record.title)")
        }
    }
}

// MARK: - release / archive

struct SessionRelease: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "release",
        abstract: "Kill a session's process tree, keeping the record.",
        discussion: "Add --remove to drop the record too (the tab disappears).")

    @OptionGroup var output: OutputOptions

    @Argument(help: "Session id, id prefix, or a unique part of its title.")
    var session: String

    @Flag(name: .long, help: "Also forget the record, not just the process.")
    var remove = false

    func run() throws {
        let record = try CLIClient.resolve(session, in: CLIClient.sessions().sessions)
        _ = try CLIClient.withConn { conn in
            try conn.request(
                remove ? Method.sessionRemove : Method.sessionKill,
                params: SessionIDParams(sessionID: record.id))
        }
        if output.json {
            output.emit(.object(["ok": .bool(true), "id": .string(record.id.rawValue)]))
        } else {
            print("released \(record.id.rawValue)")
        }
    }
}

struct SessionArchive: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "archive",
        abstract: "Kill the tree but keep the conversation revivable.")

    @OptionGroup var output: OutputOptions

    @Argument(help: "Session id, id prefix, or a unique part of its title.")
    var session: String

    @Flag(name: .long, help: "Bring an archived session back into the list instead.")
    var undo = false

    func run() throws {
        let record = try CLIClient.resolve(session, in: CLIClient.sessions().sessions)
        _ = try CLIClient.withConn { conn in
            try conn.request(
                undo ? Method.sessionUnarchive : Method.sessionArchive,
                params: SessionIDParams(sessionID: record.id))
        }
        if output.json {
            output.emit(.object(["ok": .bool(true), "id": .string(record.id.rawValue)]))
        } else {
            print("\(undo ? "unarchived" : "archived") \(record.id.rawValue)")
        }
    }
}

// MARK: - artifacts

struct Artifacts: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "artifacts",
        abstract: "PRs, previews, links, and listening ports a session produced.")

    @OptionGroup var output: OutputOptions

    @Argument(help: "Session id, id prefix, or a unique part of its title.")
    var session: String

    func run() throws {
        let record = try CLIClient.resolve(session, in: CLIClient.sessions().sessions)
        let artifacts = record.artifacts ?? []
        let ports = record.listeningPorts ?? []

        if output.json {
            output.emit(.object([
                "id": .string(record.id.rawValue),
                "artifacts": .array(artifacts.map { (try? JSONValue(encoding: $0)) ?? .null }),
                "listeningPorts": .array(ports.map { (try? JSONValue(encoding: $0)) ?? .null }),
                "pullRequests": .array(
                    (record.pullRequests ?? []).map { (try? JSONValue(encoding: $0)) ?? .null }),
            ]))
            return
        }
        if artifacts.isEmpty, ports.isEmpty {
            print("No artifacts for \(record.id.rawValue).")
            return
        }
        var prByURL: [String: PullRequestStatus] = [:]
        for pr in record.pullRequests ?? [] where prByURL[pr.url] == nil { prByURL[pr.url] = pr }
        for artifact in artifacts {
            if let pr = prByURL[artifact.url] {
                print("\(padColumn(artifact.kind.rawValue, 12))  \(artifact.url)  [\(pr.overall)]")
            } else {
                print("\(padColumn(artifact.kind.rawValue, 12))  \(artifact.url)")
            }
        }
        for port in ports {
            print("\(padColumn("port", 12))  localhost:\(port.port)  (\(port.processName))")
        }
    }
}

// MARK: - shared rendering

enum SessionRendering {
    static func table(_ sessions: [SessionRecord]) {
        guard !sessions.isEmpty else {
            print("No active sessions.")
            return
        }
        let idWidth = max(4, sessions.map { $0.id.rawValue.count }.max() ?? 4)
        let statusWidth = max(6, sessions.map { $0.status.label.count }.max() ?? 6)
        let header = padColumn("ID", idWidth) + "  K  " + padColumn("STATUS", statusWidth) + "  TITLE"
        print(header)
        print(String(repeating: "─", count: header.count))
        for session in sessions {
            print(
                padColumn(session.id.rawValue, idWidth) + "  "
                    + session.kind.glyph + "  "
                    + padColumn(session.status.label, statusWidth) + "  "
                    + session.title)
        }
    }

    static func detail(_ record: SessionRecord) {
        print("id        \(record.id.rawValue)")
        print("title     \(record.title)")
        print("kind      \(record.effectiveKind.shortLabel)")
        print("status    \(record.status.label)")
        print("cwd       \(record.cwd)")
        if let branch = record.gitBranch { print("branch    \(branch)") }
        if let worktree = record.worktreePath { print("worktree  \(worktree)") }
        if let host = record.host { print("host      \(host)") }
        if let parent = record.parent { print("parent    \(parent.rawValue)") }
        if let needsInput = record.needsInput {
            print("blocked   \(needsInput.kind.rawValue): \(needsInput.summary)")
            if let tool = needsInput.toolName { print("  tool    \(tool)") }
            print("  risk    \(needsInput.riskHint.rawValue)")
        }
        if let memory = record.memoryBytes {
            print("memory    \(memory / 1_048_576) MB")
        }
        print("resume    \(record.resumability.rawValue)")
    }
}
