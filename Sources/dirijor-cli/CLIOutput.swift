import ArgumentParser
import DirijorCore
import DirijorProtocol
import Foundation

/// Exit codes the resource/action commands agree on. Scripts branch on these,
/// so they are part of the CLI's contract: a timeout has to be distinguishable
/// from "the daemon said no" without parsing stderr.
enum CLIExit {
    /// Anything went wrong that isn't one of the specific cases below.
    static let failure = ExitCode(1)
    /// A `wait` reached its deadline without the condition holding.
    static let timeout = ExitCode(2)
    /// The daemon has no such session / worktree / record.
    static let notFound = ExitCode(3)
    /// The daemon socket isn't there or wouldn't talk.
    static let unreachable = ExitCode(4)
}

/// `--json` on every read command. Human output is the default because the CLI
/// is also read by people; `--json` is what makes it scriptable without them
/// having to parse a table.
struct OutputOptions: ParsableArguments {
    @Flag(name: .long, help: "Emit machine-readable JSON instead of a table.")
    var json = false

    /// Prints `value` as one compact JSON line. Compact rather than pretty so a
    /// stream of results stays line-oriented (`| jq -c`, `while read line`).
    func emit(_ value: JSONValue) {
        print(CLISupport.encodeCompact(value))
    }

    func emit<T: Encodable>(encoding value: T) {
        emit((try? JSONValue(encoding: value)) ?? .null)
    }
}

/// Connection + error translation shared by every daemon-backed command.
enum CLIClient {
    /// Connects or exits with `CLIExit.unreachable` and a one-line reason. The
    /// daemon being down is the single most common failure for a CLI that talks
    /// to a background process, and it deserves its own exit code.
    static func connect(socket: String? = nil) throws -> DaemonConn {
        do {
            return try DaemonConn.connect(path: socket ?? DaemonConn.socketPath())
        } catch {
            FileHandle.standardError.write(Data("dirijor: daemon unreachable (\(error))\n".utf8))
            throw CLIExit.unreachable
        }
    }

    /// Runs `body` on a fresh connection, mapping daemon errors onto exit codes.
    static func withConn<T>(socket: String? = nil, _ body: (DaemonConn) throws -> T) throws -> T {
        let conn = try connect(socket: socket)
        defer { conn.close() }
        do {
            return try body(conn)
        } catch let error as DaemonError {
            throw translate(error)
        }
    }

    static func translate(_ error: DaemonError) -> Error {
        switch error {
        case .timeout:
            FileHandle.standardError.write(Data("dirijor: timed out\n".utf8))
            return CLIExit.timeout
        case .control(let control):
            FileHandle.standardError.write(
                Data("dirijor: \(control.code): \(control.message)\n".utf8))
            return control.code == "not_found" ? CLIExit.notFound : CLIExit.failure
        case .io(let message):
            FileHandle.standardError.write(Data("dirijor: \(message)\n".utf8))
            return CLIExit.failure
        }
    }

    /// Fetches the full session list once (there is no single-record method —
    /// `session.list` is the daemon's only read of record state).
    static func sessions(socket: String? = nil) throws -> SessionListResult {
        try withConn(socket: socket) { conn in
            try conn.request(Method.sessionList, params: JSONValue.object([:]))
                .decoded(as: SessionListResult.self)
        }
    }

    /// Resolves a session by exact id, then by unique id prefix, then by unique
    /// case-insensitive title substring — typing eight characters of a generated
    /// id is the normal way people use this.
    static func resolve(_ needle: String, in sessions: [SessionRecord]) throws -> SessionRecord {
        if let exact = sessions.first(where: { $0.id.rawValue == needle }) { return exact }
        let byPrefix = sessions.filter { $0.id.rawValue.hasPrefix(needle) }
        if byPrefix.count == 1 { return byPrefix[0] }
        let lowered = needle.lowercased()
        let byTitle = sessions.filter { $0.title.lowercased().contains(lowered) }
        if byTitle.count == 1 { return byTitle[0] }
        let candidates = byPrefix.isEmpty ? byTitle : byPrefix
        if candidates.count > 1 {
            let ids = candidates.map { $0.id.rawValue }.joined(separator: ", ")
            FileHandle.standardError.write(
                Data("dirijor: \"\(needle)\" matches \(candidates.count) sessions: \(ids)\n".utf8))
            throw CLIExit.failure
        }
        FileHandle.standardError.write(Data("dirijor: no such session: \(needle)\n".utf8))
        throw CLIExit.notFound
    }
}

/// Left-pads to a column width, for the human table output.
func padColumn(_ text: String, _ width: Int) -> String {
    text.count >= width ? text : text + String(repeating: " ", count: width - text.count)
}

extension AgentKind {
    /// Parses the CLI/MCP vocabulary against the manifest catalog, so every
    /// shipped agent is spawnable by id, short label, or alias without this
    /// file knowing their names. Anything unrecognized becomes `.generic`,
    /// which is what makes `dirijor session spawn htop` work.
    ///
    /// Resolving through the catalog rather than a literal switch is what keeps
    /// a newly added manifest from silently degrading to a dumb terminal: a
    /// hardcoded list still *compiles* against every id, so the failure would
    /// be a first-class agent spawning with no detection and no status — not a
    /// build error. `shell` keeps its extra spellings here because they are
    /// login-shell synonyms, not agent aliases, and belong to no manifest.
    static func parse(_ raw: String) -> AgentKind {
        switch raw.lowercased() {
        case "shell", "sh", "bash", "zsh", "fish": return .shell
        default: break
        }
        if let descriptor = AgentCatalog.shared.resolve(name: raw) {
            return AgentKind(id: descriptor.id)
        }
        return .generic(command: raw)
    }

    /// Spawnable names for help text and error messages, straight from the
    /// catalog so it can never drift from what `parse` accepts.
    static var cliNames: [String] {
        AgentCatalog.shared.launchable.map(\.id) + ["shell"]
    }
}
