import ArgumentParser
import DirijorCore
import DirijorMCP
import DirijorProtocol
import Foundation

struct Ports: ParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "List listening ports tracked for active sessions."
    )

    @Option(help: "Connect to the daemon over TCP at this host.")
    var host: String?

    @Option(help: "Daemon TCP port when using --host.")
    var port: UInt16 = 48620

    @Option(help: "Remote access token. Defaults to remote.json when using --host.")
    var token: String?

    @Option(help: "Daemon unix socket path when not using --host.")
    var socket: String = DaemonConn.socketPath()

    func run() throws {
        let conn: DaemonConn
        if let host {
            let token = try resolvedToken()
            conn = try DaemonConn.connectTCP(host: host, port: port)
            let hello = HelloParams(build: "dirijor-cli/\(McpServer.serverVersion)", token: token)
            _ = try conn.request(Method.hello, params: hello)
        } else {
            conn = try DaemonConn.connect(path: socket)
        }
        defer { conn.close() }

        let result = try conn.request(Method.sessionList, params: JSONValue.object([:]))
        let list = try result.decoded(as: SessionListResult.self)
        let rows = list.sessions.flatMap { session in
            (session.listeningPorts ?? []).map { port in
                PortRow(port: port.port, process: port.processName, session: session.title)
            }
        }
        .sorted {
            if $0.port != $1.port { return $0.port < $1.port }
            return $0.session < $1.session
        }

        guard !rows.isEmpty else {
            print("No listening ports tracked.")
            return
        }

        let portWidth = max(4, rows.map { String($0.port).count }.max() ?? 4)
        let processWidth = max(7, rows.map(\.process.count).max() ?? 7)
        let header = pad("PORT", portWidth) + "  " + pad("PROCESS", processWidth) + "  SESSION"
        print(header)
        print(String(repeating: "-", count: header.count))
        for row in rows {
            print(
                pad(String(row.port), portWidth) + "  "
                    + pad(row.process, processWidth) + "  "
                    + row.session)
        }
    }

    private func resolvedToken() throws -> String {
        if let token, !token.isEmpty { return token }
        if let token = RemoteConfig.load(from: DirijorPaths.remoteConfigFile)?.token, !token.isEmpty {
            return token
        }
        throw ValidationError("remote token required; pass --token or enable Remote in Settings")
    }

    private func pad(_ s: String, _ width: Int) -> String {
        s.count >= width ? s : s + String(repeating: " ", count: width - s.count)
    }
}

private struct PortRow {
    var port: Int
    var process: String
    var session: String
}
