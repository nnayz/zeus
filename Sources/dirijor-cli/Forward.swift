import ArgumentParser
import DirijorCore
import DirijorMCP
import DirijorProtocol
import Foundation

#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#elseif canImport(Musl)
import Musl
#endif

struct Forward: ParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Forward local TCP ports through the Dirijor daemon."
    )

    @Argument(help: "Explicit localhost ports to forward.")
    var ports: [UInt16]

    @Option(help: "Daemon TCP host.")
    var host: String = "localhost"

    @Option(help: "Daemon TCP port.")
    var port: UInt16 = 48620

    @Option(help: "Remote access token. Defaults to remote.json for TCP.")
    var token: String?

    @Option(help: "Use the daemon unix socket instead of TCP.")
    var socket: String?

    func run() throws {
        guard !ports.isEmpty else {
            throw ValidationError("at least one port is required")
        }
        if socket != nil, host != "localhost" {
            throw ValidationError("--socket and --host are mutually exclusive")
        }

        let config = try ForwardConfig(
            host: host,
            daemonPort: port,
            token: socket == nil ? resolvedToken() : nil,
            socket: socket
        )
        let labels = try loadPortLabels(config: config)

        var listeners: [(port: UInt16, fd: Int32)] = []
        for port in Array(Set(ports)).sorted() {
            do {
                let fd = try Self.bindLocalhost(port: port)
                listeners.append((port, fd))
                let label = labels[Int(port)].map { " (\($0.process), session \"\($0.session)\")" } ?? ""
                print("forwarding localhost:\(port) -> mac:\(port)\(label)")
            } catch ForwardBindError.inUse {
                stderr("localhost:\(port) already in use -- skipping")
            } catch {
                stderr("localhost:\(port) bind failed (\(error)) -- skipping")
            }
        }

        guard !listeners.isEmpty else {
            throw ExitCode.failure
        }

        for listener in listeners {
            Thread.detachNewThread {
                Self.acceptLoop(listenerFD: listener.fd, port: listener.port, config: config)
            }
        }

        while true { pause() }
    }

    private func resolvedToken() throws -> String {
        if let token, !token.isEmpty { return token }
        if let token = RemoteConfig.load(from: DirijorPaths.remoteConfigFile)?.token, !token.isEmpty {
            return token
        }
        throw ValidationError("remote token required; pass --token or enable Remote in Settings")
    }

    private func loadPortLabels(config: ForwardConfig) throws -> [Int: ForwardLabel] {
        let conn: DaemonConn
        if let socket = config.socket {
            conn = try DaemonConn.connect(path: socket)
        } else {
            conn = try DaemonConn.connectTCP(host: config.host, port: config.daemonPort)
            let hello = HelloParams(build: "dirijor-cli/\(McpServer.serverVersion)", token: config.token)
            _ = try conn.request(Method.hello, params: hello)
        }
        defer { conn.close() }

        let result = try conn.request(Method.sessionList, params: JSONValue.object([:]))
        let list = try result.decoded(as: SessionListResult.self)
        var labels: [Int: ForwardLabel] = [:]
        for session in list.sessions {
            for port in session.listeningPorts ?? [] {
                labels[port.port] = ForwardLabel(process: port.processName, session: session.title)
            }
        }
        return labels
    }

    private static func bindLocalhost(port: UInt16) throws -> Int32 {
        let fd = posixSocket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { throw DaemonError.io("socket() failed") }
        var reuse: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &reuse, socklen_t(MemoryLayout<Int32>.size))

        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_addr.s_addr = inet_addr("127.0.0.1")
        addr.sin_port = port.bigEndian
        let rc = withUnsafePointer(to: &addr) { p in
            p.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                posixBind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard rc == 0 else {
            let err = errno
            close(fd)
            if err == EADDRINUSE { throw ForwardBindError.inUse }
            throw DaemonError.io(String(cString: strerror(err)))
        }
        guard listen(fd, 128) == 0 else {
            let err = errno
            close(fd)
            throw DaemonError.io("listen failed: \(String(cString: strerror(err)))")
        }
        return fd
    }

    private static func acceptLoop(listenerFD: Int32, port: UInt16, config: ForwardConfig) {
        while true {
            let clientFD = accept(listenerFD, nil, nil)
            if clientFD < 0 {
                if errno == EINTR { continue }
                stderr("localhost:\(port) accept failed")
                continue
            }
            Thread.detachNewThread {
                handleAccepted(clientFD: clientFD, port: port, config: config)
            }
        }
    }

    private static func handleAccepted(clientFD: Int32, port: UInt16, config: ForwardConfig) {
        do {
            let daemonFD = try config.openDaemon()
            let request = ForwardRequest(forward: port, token: config.socket == nil ? config.token : nil)
            var line = try JSONEncoder.dirijor.encode(request)
            line.append(0x0A)
            try writeAll(fd: daemonFD, data: line)

            guard let (ack, leftover) = readAck(fd: daemonFD, timeoutSeconds: 5) else {
                stderr("localhost:\(port): no forward ack from daemon; is it updated?")
                close(clientFD)
                close(daemonFD)
                return
            }
            guard ack.ok else {
                stderr("localhost:\(port): \(hint(for: ack.error))")
                close(clientFD)
                close(daemonFD)
                return
            }
            // Target bytes can coalesce into the same read as the ack line
            // (server-first protocols greet before the client sends anything).
            if !leftover.isEmpty {
                try writeAll(fd: clientFD, data: leftover)
            }

            let closer = PumpCloser(a: clientFD, b: daemonFD)
            Thread.detachNewThread {
                pump(from: clientFD, to: daemonFD, closer: closer)
            }
            Thread.detachNewThread {
                pump(from: daemonFD, to: clientFD, closer: closer)
            }
        } catch {
            stderr("localhost:\(port): forward setup failed (\(error))")
            close(clientFD)
        }
    }

    private static func readAck(fd: Int32, timeoutSeconds: Int) -> (ack: ForwardAck, leftover: Data)? {
        var tv = timeval(tv_sec: timeoutSeconds, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        defer {
            var zero = timeval(tv_sec: 0, tv_usec: 0)
            setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &zero, socklen_t(MemoryLayout<timeval>.size))
        }

        var buffer = Data()
        while !buffer.contains(0x0A), buffer.count < 4096 {
            var chunk = [UInt8](repeating: 0, count: 512)
            let n = read(fd, &chunk, chunk.count)
            if n <= 0 { return nil }
            buffer.append(contentsOf: chunk[0..<n])
        }
        guard let newline = buffer.firstIndex(of: 0x0A) else { return nil }
        let line = buffer.subdata(in: buffer.startIndex..<newline)
        guard let ack = try? JSONDecoder.dirijor.decode(ForwardAck.self, from: line) else {
            return nil
        }
        let leftover = buffer.subdata(in: buffer.index(after: newline)..<buffer.endIndex)
        return (ack, leftover)
    }

    private static func pump(from: Int32, to: Int32, closer: PumpCloser) {
        var buffer = [UInt8](repeating: 0, count: 64 * 1024)
        while true {
            let n = read(from, &buffer, buffer.count)
            if n == 0 { break }
            if n < 0 {
                if errno == EINTR { continue }
                break
            }
            do {
                try writeAll(fd: to, bytes: buffer, count: n)
            } catch {
                break
            }
        }
        closer.close()
    }

    private static func writeAll(fd: Int32, data: Data) throws {
        try data.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
            guard let base = raw.bindMemory(to: UInt8.self).baseAddress else { return }
            try writeAll(fd: fd, base: base, count: data.count)
        }
    }

    private static func writeAll(fd: Int32, bytes: [UInt8], count: Int) throws {
        try bytes.withUnsafeBufferPointer { buffer in
            guard let base = buffer.baseAddress else { return }
            try writeAll(fd: fd, base: base, count: count)
        }
    }

    private static func writeAll(fd: Int32, base: UnsafePointer<UInt8>, count: Int) throws {
        var offset = 0
        while offset < count {
            let n = write(fd, base + offset, count - offset)
            if n < 0 {
                if errno == EINTR { continue }
                throw DaemonError.io("write failed")
            }
            offset += n
        }
    }

    private static func hint(for error: String?) -> String {
        switch error {
        case "unauthorized":
            return "unauthorized; check --token"
        case "port_not_allowed":
            return "port_not_allowed; remote forwards are limited to session-tracked ports"
        case "connect_failed":
            return "connect_failed; daemon could not dial localhost target"
        case let error?:
            return error
        case nil:
            return "forward rejected"
        }
    }
}

private struct ForwardConfig: Sendable {
    var host: String
    var daemonPort: UInt16
    var token: String?
    var socket: String?

    func openDaemon() throws -> Int32 {
        if let socket {
            return try DaemonConn.openUDS(path: socket, timeout: 3)
        }
        return try DaemonConn.openTCP(host: host, port: daemonPort, timeout: 3)
    }
}

private struct ForwardLabel {
    var process: String
    var session: String
}

private enum ForwardBindError: Error {
    case inUse
}

private final class PumpCloser: @unchecked Sendable {
    private let lock = NSLock()
    private var closed = false
    private let a: Int32
    private let b: Int32

    init(a: Int32, b: Int32) {
        self.a = a
        self.b = b
    }

    func close() {
        lock.lock()
        guard !closed else {
            lock.unlock()
            return
        }
        closed = true
        lock.unlock()
        shutdown(a, SHUT_RDWR)
        shutdown(b, SHUT_RDWR)
        posixClose(a)
        posixClose(b)
    }
}

private func stderr(_ message: String) {
    FileHandle.standardError.write(Data((message + "\n").utf8))
}
