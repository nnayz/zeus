import DirijorCore
import DirijorProtocol
import Foundation
import Testing

@testable import DirijorDaemonKit

#if canImport(Darwin)
import Darwin
#endif

// MARK: - Harness

private struct PortForwardHarness {
    let hub: ConnectionHub
    let registry: SessionRegistry
    let socketPath: String
    let port: UInt16
    let token: String
    let dir: URL
}

private func makePortForwardHarness(
    token: String = "s3cr3t-remote-token",
    forwardAnyPort: Bool? = true
) async throws -> PortForwardHarness {
    let shortID = String(UUID().uuidString.prefix(8))
    let dir = URL(fileURLWithPath: "/tmp/djf-\(shortID)", isDirectory: true)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

    let port = freeTCPPort()
    let remote = RemoteConfig(port: port, token: token, forwardAnyPort: forwardAnyPort)
    let config = DaemonConfig(
        socketPath: dir.appendingPathComponent("d.sock").path,
        cliPath: "/usr/bin/true",
        injectDir: dir,
        logsDir: dir,
        stateFile: dir.appendingPathComponent("state.json"),
        remote: remote
    )
    let events = EventBus()
    let registry = SessionRegistry(config: config, events: events)
    let statusEngine = StatusEngine()
    await statusEngine.bind(registry: registry)
    await registry.bind(statusEngine: statusEngine)
    let services = DaemonServices(
        registry: registry, events: events, statusEngine: statusEngine,
        browserPool: BrowserPool(config: config), governor: ResourceGovernor(registry: registry),
        build: "test", executableHash: nil, shutdownRequest: ShutdownRequestBox(),
        remote: remote)
    let hub = ConnectionHub(socketPath: config.socketPath, services: services)
    try await hub.start()
    try await Task.sleep(for: .milliseconds(150))
    return PortForwardHarness(
        hub: hub, registry: registry, socketPath: config.socketPath,
        port: port, token: token, dir: dir)
}

private func teardown(_ h: PortForwardHarness) async {
    await h.hub.stop()
    try? FileManager.default.removeItem(at: h.dir)
}

// MARK: - Tests

@Test func remoteForwardRoundTrip() async throws {
    let h = try await makePortForwardHarness()
    defer { Task { await teardown(h) } }
    let echo = try EchoServer()
    defer { echo.stop() }

    let client = try RawForwardClient.tcp(host: "127.0.0.1", port: h.port)
    defer { client.close() }
    try client.sendForward(port: echo.port, token: h.token)
    let ack = try #require(try client.readAck(timeout: 3))
    #expect(ack.ok)

    try client.write(Data("hello".utf8))
    let echoed = try #require(try client.readBytes(count: 5, timeout: 3))
    #expect(String(decoding: echoed, as: UTF8.self) == "hello")

    client.close()
    try await waitUntil(timeout: .seconds(3)) { echo.sawEOF() }
}

@Test func remoteForwardWrongTokenRejectsBeforeDial() async throws {
    let h = try await makePortForwardHarness()
    defer { Task { await teardown(h) } }
    let echo = try EchoServer()
    defer { echo.stop() }

    let client = try RawForwardClient.tcp(host: "127.0.0.1", port: h.port)
    defer { client.close() }
    try client.sendForward(port: echo.port, token: "wrong-token")
    let ack = try #require(try client.readAck(timeout: 3))
    #expect(ack.ok == false)
    #expect(ack.error == "unauthorized")

    try await Task.sleep(for: .milliseconds(200))
    #expect(echo.acceptedCount() == 0)
}

@Test func remoteForwardRejectsUntrackedPort() async throws {
    let h = try await makePortForwardHarness(forwardAnyPort: false)
    defer { Task { await teardown(h) } }
    let echo = try EchoServer()
    defer { echo.stop() }

    let client = try RawForwardClient.tcp(host: "127.0.0.1", port: h.port)
    defer { client.close() }
    try client.sendForward(port: echo.port, token: h.token)
    let ack = try #require(try client.readAck(timeout: 3))
    #expect(ack.ok == false)
    #expect(ack.error == "port_not_allowed")
    #expect(echo.acceptedCount() == 0)
}

@Test func remoteForwardReportsConnectFailed() async throws {
    let h = try await makePortForwardHarness()
    defer { Task { await teardown(h) } }
    let deadPort = freeTCPPort()

    let client = try RawForwardClient.tcp(host: "127.0.0.1", port: h.port)
    defer { client.close() }
    try client.sendForward(port: deadPort, token: h.token)
    let ack = try #require(try client.readAck(timeout: 5))
    #expect(ack.ok == false)
    #expect(ack.error == "connect_failed")
}

@Test func udsForwardRoundTripWithoutToken() async throws {
    let h = try await makePortForwardHarness()
    defer { Task { await teardown(h) } }
    let echo = try EchoServer()
    defer { echo.stop() }

    let client = try RawForwardClient.uds(path: h.socketPath)
    defer { client.close() }
    try client.sendForward(port: echo.port, token: nil)
    let ack = try #require(try client.readAck(timeout: 3))
    #expect(ack.ok)

    try client.write(Data("local".utf8))
    let echoed = try #require(try client.readBytes(count: 5, timeout: 3))
    #expect(String(decoding: echoed, as: UTF8.self) == "local")
}

@Test func forwardPipelinedHandshakePayloadReachesTarget() async throws {
    let h = try await makePortForwardHarness()
    defer { Task { await teardown(h) } }
    let echo = try EchoServer()
    defer { echo.stop() }

    let client = try RawForwardClient.tcp(host: "127.0.0.1", port: h.port)
    defer { client.close() }
    let request = ForwardRequest(forward: echo.port, token: h.token)
    var payload = try JSONEncoder.dirijor.encode(request)
    payload.append(0x0A)
    payload.append(Data("pipe".utf8))
    try client.write(payload)

    let ack = try #require(try client.readAck(timeout: 3))
    #expect(ack.ok)
    let echoed = try #require(try client.readBytes(count: 4, timeout: 3))
    #expect(String(decoding: echoed, as: UTF8.self) == "pipe")
}

@Test func forwardProtocolDisambiguatesAttachAndForward() throws {
    let attachLine = Data(#"{"attach":"s_abc","token":"t"}"#.utf8)
    let forwardLine = Data(#"{"forward":3000,"token":"t"}"#.utf8)

    #expect((try? JSONDecoder.dirijor.decode(AttachRequest.self, from: attachLine)) != nil)
    #expect((try? JSONDecoder.dirijor.decode(ForwardRequest.self, from: attachLine)) == nil)
    #expect((try? JSONDecoder.dirijor.decode(ForwardRequest.self, from: forwardLine)) != nil)
    #expect((try? JSONDecoder.dirijor.decode(AttachRequest.self, from: forwardLine)) == nil)
}

@Test func remoteConfigDecodesLegacyWithoutForwardAnyPort() throws {
    let data = Data(#"{"port":48620,"token":"t"}"#.utf8)
    let config = try JSONDecoder.dirijor.decode(RemoteConfig.self, from: data)
    #expect(config.port == 48620)
    #expect(config.token == "t")
    #expect(config.forwardAnyPort == nil)
}

// MARK: - Helpers

private final class EchoServer: @unchecked Sendable {
    private let fd: Int32
    let port: UInt16
    private let lock = NSLock()
    private var accepted = 0
    private var eof = false

    init() throws {
        let sock = Darwin.socket(AF_INET, SOCK_STREAM, 0)
        guard sock >= 0 else { throw RawError.socketFailed }
        var reuse: Int32 = 1
        setsockopt(sock, SOL_SOCKET, SO_REUSEADDR, &reuse, socklen_t(MemoryLayout<Int32>.size))
        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_addr.s_addr = inet_addr("127.0.0.1")
        addr.sin_port = 0
        let bindResult = withUnsafePointer(to: &addr) { p in
            p.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(sock, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard bindResult == 0 else {
            Darwin.close(sock)
            throw RawError.bindFailed
        }
        var len = socklen_t(MemoryLayout<sockaddr_in>.size)
        _ = withUnsafeMutablePointer(to: &addr) { p in
            p.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(sock, $0, &len) }
        }
        guard listen(sock, 16) == 0 else {
            Darwin.close(sock)
            throw RawError.bindFailed
        }
        fd = sock
        port = UInt16(bigEndian: addr.sin_port)
        Thread.detachNewThread { [weak self] in self?.acceptLoop() }
    }

    func stop() {
        Darwin.close(fd)
    }

    func acceptedCount() -> Int {
        lock.lock()
        defer { lock.unlock() }
        return accepted
    }

    func sawEOF() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return eof
    }

    private func acceptLoop() {
        while true {
            let client = accept(fd, nil, nil)
            if client < 0 { return }
            lock.lock()
            accepted += 1
            lock.unlock()
            Thread.detachNewThread { [weak self] in self?.echo(client) }
        }
    }

    private func echo(_ client: Int32) {
        var buffer = [UInt8](repeating: 0, count: 4096)
        while true {
            let n = read(client, &buffer, buffer.count)
            if n == 0 {
                lock.lock()
                eof = true
                lock.unlock()
                break
            }
            if n < 0 { break }
            buffer.withUnsafeBufferPointer { raw in
                guard let base = raw.baseAddress else { return }
                var offset = 0
                while offset < n {
                    let wrote = write(client, base + offset, n - offset)
                    if wrote <= 0 { break }
                    offset += wrote
                }
            }
        }
        Darwin.close(client)
    }
}

private final class RawForwardClient {
    private let fd: Int32
    private var buffer = Data()

    private init(fd: Int32) {
        self.fd = fd
    }

    static func tcp(host: String, port: UInt16) throws -> RawForwardClient {
        var connected: Int32 = -1
        var sock: Int32 = -1
        for _ in 0..<40 {
            sock = Darwin.socket(AF_INET, SOCK_STREAM, 0)
            var addr = sockaddr_in()
            addr.sin_family = sa_family_t(AF_INET)
            addr.sin_port = port.bigEndian
            addr.sin_addr.s_addr = inet_addr(host)
            connected = withUnsafePointer(to: &addr) { p in
                p.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    connect(sock, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
                }
            }
            if connected == 0 { break }
            Darwin.close(sock)
            usleep(50_000)
        }
        guard connected == 0 else { throw RawError.connectFailed }
        return RawForwardClient(fd: sock)
    }

    static func uds(path: String) throws -> RawForwardClient {
        let sock = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard sock >= 0 else { throw RawError.socketFailed }
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let capacity = MemoryLayout.size(ofValue: addr.sun_path)
        let pathBytes = Array(path.utf8)
        guard pathBytes.count < capacity else {
            Darwin.close(sock)
            throw RawError.connectFailed
        }
        withUnsafeMutablePointer(to: &addr.sun_path) { rawPtr in
            rawPtr.withMemoryRebound(to: CChar.self, capacity: capacity) { dst in
                for (i, b) in pathBytes.enumerated() { dst[i] = CChar(bitPattern: b) }
                dst[pathBytes.count] = 0
            }
        }
        let rc = withUnsafePointer(to: &addr) { p in
            p.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(sock, $0, socklen_t(MemoryLayout<sockaddr_un>.stride))
            }
        }
        guard rc == 0 else {
            Darwin.close(sock)
            throw RawError.connectFailed
        }
        return RawForwardClient(fd: sock)
    }

    func sendForward(port: UInt16, token: String?) throws {
        let request = ForwardRequest(forward: port, token: token)
        var line = try JSONEncoder.dirijor.encode(request)
        line.append(0x0A)
        try write(line)
    }

    func write(_ data: Data) throws {
        try data.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
            guard let base = raw.bindMemory(to: UInt8.self).baseAddress else { return }
            var offset = 0
            while offset < data.count {
                let n = Darwin.write(fd, base + offset, data.count - offset)
                if n < 0 {
                    if errno == EINTR { continue }
                    throw RawError.writeFailed
                }
                offset += n
            }
        }
    }

    func readAck(timeout seconds: Int) throws -> ForwardAck? {
        while !buffer.contains(0x0A) {
            guard let data = try readSome(timeout: seconds), !data.isEmpty else { return nil }
            buffer.append(data)
        }
        guard let newline = buffer.firstIndex(of: 0x0A) else { return nil }
        let line = buffer.subdata(in: buffer.startIndex..<newline)
        buffer.removeSubrange(buffer.startIndex...newline)
        return try JSONDecoder.dirijor.decode(ForwardAck.self, from: line)
    }

    func readBytes(count: Int, timeout seconds: Int) throws -> Data? {
        while buffer.count < count {
            guard let data = try readSome(timeout: seconds), !data.isEmpty else { return nil }
            buffer.append(data)
        }
        let out = buffer.subdata(in: buffer.startIndex..<buffer.startIndex + count)
        buffer.removeSubrange(buffer.startIndex..<buffer.startIndex + count)
        return out
    }

    func close() {
        Darwin.close(fd)
    }

    private func readSome(timeout seconds: Int) throws -> Data? {
        var pfd = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
        let pr = poll(&pfd, 1, Int32(seconds * 1000))
        guard pr > 0 else { return nil }
        var chunk = [UInt8](repeating: 0, count: 4096)
        let n = read(fd, &chunk, chunk.count)
        if n <= 0 { return nil }
        return Data(chunk[0..<n])
    }
}

private func freeTCPPort() -> UInt16 {
    let fd = Darwin.socket(AF_INET, SOCK_STREAM, 0)
    defer { Darwin.close(fd) }
    var addr = sockaddr_in()
    addr.sin_family = sa_family_t(AF_INET)
    addr.sin_addr.s_addr = inet_addr("127.0.0.1")
    addr.sin_port = 0
    _ = withUnsafePointer(to: &addr) { p in
        p.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
        }
    }
    var len = socklen_t(MemoryLayout<sockaddr_in>.size)
    _ = withUnsafeMutablePointer(to: &addr) { p in
        p.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(fd, $0, &len) }
    }
    return UInt16(bigEndian: addr.sin_port)
}

private enum RawError: Error {
    case socketFailed
    case bindFailed
    case connectFailed
    case writeFailed
}
