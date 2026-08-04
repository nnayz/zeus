import DirijorClient
import DirijorCore
import DirijorProtocol
import Darwin
import Foundation
import Testing

@testable import DirijorDaemonKit

/// Remote-access (TCP + token) tests. Each spins up an in-process ConnectionHub
/// bound to a temp UDS path AND a random localhost TCP port with a RemoteConfig,
/// then exercises token enforcement over both the control and data channels.
///
/// NOTE: PTY spawn may be unavailable in this sandbox (see the known
/// realPTYSessionEchoAndExit failure). The data-channel tests are written to
/// verify the attach handshake / token gate WITHOUT depending on PTY output:
/// the happy-path attach is skipped (with a recorded note) if spawn fails.

// MARK: - Harness

private struct RemoteHarness {
    let hub: ConnectionHub
    let registry: SessionRegistry
    let events: EventBus
    let socketPath: String
    let port: UInt16
    let token: String
    let dir: URL
}

private func makeHarness(token: String = "s3cr3t-remote-token") async throws -> RemoteHarness {
    // Keep the UDS path SHORT: sockaddr_un.sun_path caps at 104 bytes, and the
    // /var/folders temp dir alone nearly hits that — a long path makes
    // NWConnection(to: .unix) trap. Park the socket directly under /tmp.
    let shortID = String(UUID().uuidString.prefix(8))
    let dir = URL(fileURLWithPath: "/tmp/djr-\(shortID)", isDirectory: true)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

    let port = freeTCPPort()
    let remote = RemoteConfig(port: port, token: token)
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
    // Give both listeners a moment to reach .ready before connecting.
    try await Task.sleep(for: .milliseconds(150))
    return RemoteHarness(
        hub: hub, registry: registry, events: events, socketPath: config.socketPath,
        port: port, token: token, dir: dir)
}

@Test func localControlSocketIsOwnerOnly() async throws {
    let h = try await makeHarness()
    defer { Task { await teardown(h) } }
    var info = stat()
    #expect(stat(h.socketPath, &info) == 0)
    #expect(info.st_mode & 0o777 == 0o600)
}

private func teardown(_ h: RemoteHarness) async {
    await h.hub.stop()
    try? FileManager.default.removeItem(at: h.dir)
}

// MARK: - Control channel: token enforcement (raw NDJSON over TCP)

@Test func remoteWrongTokenAndMissingHelloAreUnauthorized() async throws {
    let h = try await makeHarness()
    defer { Task { await teardown(h) } }

    // (a) session.list before any hello → unauthorized.
    do {
        let client = try RawLineClient(host: "127.0.0.1", port: h.port)
        defer { client.close() }
        try client.sendLine(
            NDJSONBuffer.encode(.request(id: 1, method: Method.sessionList, params: nil)))
        let response = try #require(client.readMessage(timeout: 3))
        #expect(errorCode(of: response) == "unauthorized")
    }

    // (b) hello carrying the WRONG token → unauthorized.
    do {
        let client = try RawLineClient(host: "127.0.0.1", port: h.port)
        defer { client.close() }
        let hello = try JSONValue(encoding: HelloParams(build: "t", token: "not-the-token"))
        try client.sendLine(
            NDJSONBuffer.encode(.request(id: 1, method: Method.hello, params: hello)))
        let response = try #require(client.readMessage(timeout: 3))
        #expect(errorCode(of: response) == "unauthorized")
    }
}

@Test func remoteRightTokenAuthorizesControlChannel() async throws {
    let h = try await makeHarness()
    defer { Task { await teardown(h) } }

    let client = try RawLineClient(host: "127.0.0.1", port: h.port)
    defer { client.close() }

    // hello with the correct token succeeds.
    let hello = try JSONValue(encoding: HelloParams(build: "t", token: h.token))
    try client.sendLine(NDJSONBuffer.encode(.request(id: 1, method: Method.hello, params: hello)))
    let helloResp = try #require(client.readMessage(timeout: 3))
    #expect(isSuccess(helloResp))
    #expect(errorCode(of: helloResp) == nil)

    // session.list now works on the same authorized connection.
    try client.sendLine(NDJSONBuffer.encode(.request(id: 2, method: Method.sessionList, params: nil)))
    let listResp = try #require(client.readMessage(timeout: 3))
    #expect(isSuccess(listResp))
}

// MARK: - Control channel via DaemonClient

@Test func remoteDaemonClientConnectsWithRightToken() async throws {
    let h = try await makeHarness()
    defer { Task { await teardown(h) } }

    let client = DaemonClient(
        endpoint: .tcp(host: "127.0.0.1", port: h.port), build: "t", token: h.token)
    await client.connect()
    defer { Task { await client.shutdown() } }

    try await waitUntil(timeout: .seconds(5)) { await client.lastHello != nil }
    let list = try await client.sessions()
    #expect(list.sessions.isEmpty || !list.sessions.isEmpty)  // call succeeded
}

@Test func remoteDaemonClientWrongTokenNeverEstablishes() async throws {
    let h = try await makeHarness()
    defer { Task { await teardown(h) } }

    let client = DaemonClient(
        endpoint: .tcp(host: "127.0.0.1", port: h.port), build: "t", token: "wrong")
    await client.connect()
    defer { Task { await client.shutdown() } }

    // Give it well past a round-trip; hello is rejected so it never establishes.
    try await Task.sleep(for: .milliseconds(1500))
    let hello = await client.lastHello
    #expect(hello == nil)
}

// MARK: - UDS path: unaffected by remote gating

@Test func udsClientIgnoresRemoteGatingWithoutToken() async throws {
    let h = try await makeHarness()
    defer { Task { await teardown(h) } }

    // Same daemon, but over the unix socket with NO token: must work as before.
    let client = DaemonClient(
        endpoint: .unixSocket(path: h.socketPath), build: "t", token: nil)
    await client.connect()
    defer { Task { await client.shutdown() } }

    try await waitUntil(timeout: .seconds(5)) { await client.lastHello != nil }
    _ = try await client.sessions()
}

// MARK: - Data channel: token enforcement

@Test func remoteDataChannelRejectsWrongToken() async throws {
    let h = try await makeHarness()
    defer { Task { await teardown(h) } }

    // Wrong token: attach handshake is rejected and the transport cancelled, so
    // the chunk stream finishes without ever yielding a frame. No PTY needed —
    // rejection happens before session lookup.
    let attach = SessionAttachment(
        endpoint: .tcp(host: "127.0.0.1", port: h.port),
        sessionID: SessionID(rawValue: "s_nonexistent"),
        token: "wrong-token")
    try? await attach.connect()

    let frames = await collectChunks(attach.chunks, atMost: 4, within: .seconds(2))
    #expect(frames.isEmpty)
}

@Test func remoteDataChannelAttachesWithRightToken() async throws {
    let h = try await makeHarness()
    defer { Task { await teardown(h) } }

    // Need a live session to attach to. PTY spawn may be unavailable here.
    let record: SessionRecord
    do {
        record = try await h.registry.spawn(SessionSpawnParams(kind: .shell, cwd: "/tmp"))
    } catch {
        Issue.record("PTY spawn unavailable in sandbox (\(error)); skipping right-token attach body")
        return
    }
    let attach = SessionAttachment(
        endpoint: .tcp(host: "127.0.0.1", port: h.port),
        sessionID: record.id,
        token: h.token)
    try await attach.connect()

    // Authorized attach: the daemon replays (REPLAY_BEGIN/END) and sends an
    // initial grid frame, so we receive at least one chunk.
    let chunks = await collectChunks(attach.chunks, atMost: 1, within: .seconds(4))
    #expect(!chunks.isEmpty)

    // Synchronous cleanup so the spawned PTY tree is reaped before we exit.
    await attach.close()
    try? await h.registry.kill(sessionID: record.id)
}

// MARK: - Helpers

/// Collects up to `max` chunks from a stream, stopping early at `deadline`.
private func collectChunks(
    _ stream: AsyncStream<SessionAttachment.TerminalChunk>,
    atMost max: Int, within timeout: Duration
) async -> [SessionAttachment.TerminalChunk] {
    let collected = Collected()
    let collector = Task {
        var out: [SessionAttachment.TerminalChunk] = []
        for await chunk in stream {
            out.append(chunk)
            await collected.set(out)
            if out.count >= max { break }
        }
        await collected.set(out)
    }
    try? await Task.sleep(for: timeout)
    collector.cancel()
    return await collected.get()
}

private actor Collected {
    private var value: [SessionAttachment.TerminalChunk] = []
    func set(_ v: [SessionAttachment.TerminalChunk]) { value = v }
    func get() -> [SessionAttachment.TerminalChunk] { value }
}

private func errorCode(of message: ControlMessage) -> String? {
    if case .response(_, .failure(let err)) = message { return err.code }
    return nil
}

private func isSuccess(_ message: ControlMessage) -> Bool {
    if case .response(_, .success) = message { return true }
    return false
}

/// Picks an unused localhost TCP port by binding to :0 and reading it back.
private func freeTCPPort() -> UInt16 {
    let fd = socket(AF_INET, SOCK_STREAM, 0)
    defer { close(fd) }
    var addr = sockaddr_in()
    addr.sin_family = sa_family_t(AF_INET)
    addr.sin_addr.s_addr = inet_addr("127.0.0.1")
    addr.sin_port = 0
    _ = withUnsafePointer(to: &addr) { p in
        p.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
        }
    }
    var len = socklen_t(MemoryLayout<sockaddr_in>.size)
    _ = withUnsafeMutablePointer(to: &addr) { p in
        p.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(fd, $0, &len) }
    }
    return UInt16(bigEndian: addr.sin_port)
}

/// Minimal blocking NDJSON-over-TCP client for precise handshake assertions.
private final class RawLineClient {
    private let fd: Int32
    private var buffer = Data()

    init(host: String, port: UInt16) throws {
        // Retry connect briefly in case the listener isn't accepting yet.
        var connected: Int32 = -1
        var sock: Int32 = -1
        for _ in 0..<40 {
            sock = socket(AF_INET, SOCK_STREAM, 0)
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
        fd = sock
    }

    func sendLine(_ data: Data) throws {
        var payload = data
        _ = payload.withUnsafeMutableBytes { raw in
            write(fd, raw.baseAddress, raw.count)
        }
    }

    /// Reads one newline-terminated JSON message, or nil on timeout/EOF.
    func readMessage(timeout seconds: Int) -> ControlMessage? {
        var tv = timeval(tv_sec: seconds, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        while !buffer.contains(0x0A) {
            var chunk = [UInt8](repeating: 0, count: 4096)
            let n = read(fd, &chunk, chunk.count)
            if n <= 0 { return nil }
            buffer.append(contentsOf: chunk[0..<n])
        }
        guard let nl = buffer.firstIndex(of: 0x0A) else { return nil }
        let line = buffer.subdata(in: buffer.startIndex..<nl)
        buffer.removeSubrange(buffer.startIndex...nl)
        return try? JSONDecoder.dirijor.decode(ControlMessage.self, from: line)
    }

    func close() { Darwin.close(fd) }

    enum RawError: Error { case connectFailed }
}

// MARK: - Event subscription over the control channel

/// End-to-end proof that a filtered subscription is honoured *on the wire*, not
/// just inside the bus: the connection handler has to pass the client's filter
/// through, and the daemon must never send an event the client excluded.
@Test func eventSubscriptionPushesOnlyTheRequestedKinds() async throws {
    let h = try await makeHarness()
    defer { Task { await teardown(h) } }

    let client = try RawLineClient(host: "127.0.0.1", port: h.port)
    defer { client.close() }
    let hello = try JSONValue(encoding: HelloParams(build: "t", token: h.token))
    try client.sendLine(NDJSONBuffer.encode(.request(id: 1, method: Method.hello, params: hello)))
    #expect(client.readMessage(timeout: 3) != nil)

    let subscribe = try JSONValue(
        encoding: EventsSubscribeParams(kinds: [EventName.sessionStatus]))
    try client.sendLine(
        NDJSONBuffer.encode(.request(id: 2, method: Method.eventsSubscribe, params: subscribe)))
    guard case .response(let ackID, .success(let ack))? = client.readMessage(timeout: 3) else {
        Issue.record("no subscribe ack")
        return
    }
    #expect(ackID == 2)
    #expect(ack["subscribed"] == .bool(true))

    let watched = SessionID(rawValue: "s_watched")
    await h.events.publish(
        name: EventName.sessionOutput, params: .string("noise"), sessionID: watched)
    await h.events.publish(
        name: EventName.sessionStatus, params: .string("wanted"), sessionID: watched)

    guard case .event(let name, _, let params)? = client.readMessage(timeout: 3) else {
        Issue.record("no event delivered")
        return
    }
    // The excluded `session.output` was published FIRST, so receiving the
    // status event first is what proves the filter ran server-side rather than
    // the client simply reading ahead.
    #expect(name == EventName.sessionStatus)
    #expect(params == .string("wanted"))
}

/// Session-scoped subscriptions are the shape a script uses ("watch this one
/// agent"), so the scoping has to hold across the connection too.
@Test func eventSubscriptionScopesToRequestedSessions() async throws {
    let h = try await makeHarness()
    defer { Task { await teardown(h) } }

    let client = try RawLineClient(host: "127.0.0.1", port: h.port)
    defer { client.close() }
    let hello = try JSONValue(encoding: HelloParams(build: "t", token: h.token))
    try client.sendLine(NDJSONBuffer.encode(.request(id: 1, method: Method.hello, params: hello)))
    #expect(client.readMessage(timeout: 3) != nil)

    let mine = SessionID(rawValue: "s_mine")
    let theirs = SessionID(rawValue: "s_theirs")
    let subscribe = try JSONValue(encoding: EventsSubscribeParams(sessions: [mine]))
    try client.sendLine(
        NDJSONBuffer.encode(.request(id: 2, method: Method.eventsSubscribe, params: subscribe)))
    #expect(client.readMessage(timeout: 3) != nil)

    await h.events.publish(name: EventName.sessionStatus, params: .string("no"), sessionID: theirs)
    await h.events.publish(name: EventName.worktreeCreated, params: .string("also no"))
    await h.events.publish(name: EventName.sessionStatus, params: .string("yes"), sessionID: mine)

    guard case .event(_, _, let params)? = client.readMessage(timeout: 3) else {
        Issue.record("no event delivered")
        return
    }
    #expect(params == .string("yes"))
}
