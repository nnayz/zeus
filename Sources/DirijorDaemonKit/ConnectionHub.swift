import DirijorCore
import DirijorGit
import DirijorProtocol
import Darwin
import Foundation
import Network

/// Everything a connection handler needs.
struct DaemonServices: Sendable {
    let registry: SessionRegistry
    let events: EventBus
    let statusEngine: StatusEngine
    let browserPool: BrowserPool
    let governor: ResourceGovernor
    let build: String
    /// SHA-256 of the daemon executable, reported in hello for staleness checks.
    let executableHash: String?
    /// Late-bound graceful-shutdown hook (Daemon binds it in start(); the hub is
    /// constructed in Daemon.init where `self` isn't capturable yet).
    let shutdownRequest: ShutdownRequestBox
    /// Remote-access config. Present ⇒ a TCP listener is opened and connections
    /// arriving on it must present `remote.token`. nil ⇒ UDS-only (default).
    let remote: RemoteConfig?
}

/// A tiny late-binding box for the daemon.shutdown handler.
public final class ShutdownRequestBox: @unchecked Sendable {
    private let lock = NSLock()
    private var handler: (@Sendable () -> Void)?

    public init() {}

    public func bind(_ handler: @escaping @Sendable () -> Void) {
        lock.lock()
        self.handler = handler
        lock.unlock()
    }

    func fire() {
        lock.lock()
        let handler = self.handler
        lock.unlock()
        handler?()
    }
}

/// Listens on the unix control socket and hands each connection to a
/// ServerConnection (control NDJSON or data-channel frames, decided by the
/// first line).
public actor ConnectionHub {
    private var listener: NWListener?
    /// Optional TCP listener for remote (Tailscale) access. Only opened when the
    /// daemon has a RemoteConfig; every connection on it is token-gated.
    private var tcpListener: NWListener?
    private let socketPath: String
    private let services: DaemonServices
    private var connections: [UUID: ServerConnection] = [:]

    init(socketPath: String, services: DaemonServices) {
        self.socketPath = socketPath
        self.services = services
    }

    public func start() throws {
        // Remove a stale socket file from a previous run.
        try? FileManager.default.removeItem(atPath: socketPath)

        let params = NWParameters.tcp
        params.requiredLocalEndpoint = NWEndpoint.unix(path: socketPath)
        params.allowLocalEndpointReuse = true
        let listener = try NWListener(using: params)
        self.listener = listener

        listener.newConnectionHandler = { [weak self] connection in
            Task { await self?.adopt(connection, isRemote: false) }
        }
        let socketPath = socketPath
        listener.stateUpdateHandler = { state in
            if case .ready = state {
                // Network.framework follows the process umask; enforce the
                // local trust boundary explicitly once the socket exists.
                _ = chmod(socketPath, S_IRUSR | S_IWUSR)
            }
            DaemonLog.shared.log("listener state: \(state)")
        }
        listener.start(queue: DispatchQueue(label: "dirijor.hub"))
        DaemonLog.shared.log("listening on \(socketPath)")

        // Second listener for remote access over Tailscale. It binds one exact
        // local address, never 0.0.0.0, so LAN traffic cannot bypass Tailscale.
        // Legacy configs without bindHost fail closed to loopback.
        if let remote = services.remote {
            guard let port = NWEndpoint.Port(rawValue: remote.port) else {
                DaemonLog.shared.log("remote: invalid port \(remote.port); skipping TCP listener")
                return
            }
            let tcpParams = NWParameters.tcp
            tcpParams.allowLocalEndpointReuse = true
            let bindHost = remote.bindHost ?? "127.0.0.1"
            tcpParams.requiredLocalEndpoint = .hostPort(
                host: NWEndpoint.Host(bindHost), port: port)
            if let tcp = tcpParams.defaultProtocolStack.internetProtocol as? NWProtocolTCP.Options {
                tcp.noDelay = true
            }
            let tcpListener = try NWListener(using: tcpParams)
            self.tcpListener = tcpListener
            tcpListener.newConnectionHandler = { [weak self] connection in
                Task { await self?.adopt(connection, isRemote: true) }
            }
            tcpListener.stateUpdateHandler = { state in
                DaemonLog.shared.log("remote listener state: \(state)")
            }
            tcpListener.start(queue: DispatchQueue(label: "dirijor.hub.remote"))
            DaemonLog.shared.log("remote listening on \(bindHost):\(remote.port)")
        }
    }

    public func stop() {
        listener?.cancel()
        listener = nil
        tcpListener?.cancel()
        tcpListener = nil
        for connection in connections.values {
            Task { await connection.close() }
        }
        connections.removeAll()
    }

    private func adopt(_ nwConnection: NWConnection, isRemote: Bool) {
        let id = UUID()
        let connection = ServerConnection(
            nwConnection: nwConnection, services: services, isRemote: isRemote
        ) {
            [weak self] in
            Task { await self?.forget(id) }
        }
        connections[id] = connection
        Task { await connection.start() }
    }

    private func forget(_ id: UUID) {
        connections.removeValue(forKey: id)
    }
}

// MARK: - Per-connection handler

actor ServerConnection {
    private let transport: NWTransport
    private let services: DaemonServices
    /// True for connections arriving on the remote (TCP) listener. Remote
    /// connections are token-gated: hello must come first and carry a valid
    /// token before any other method runs. Local (UDS) connections are trusted
    /// and behave exactly as before (no gating, no token checks).
    private let isRemote: Bool
    private let onClose: @Sendable () -> Void

    private var mode: Mode = .undecided
    private var ndjson = NDJSONBuffer()
    private var frames = FrameCodec()
    private var lineAccumulator = Data()
    private var subscriptionTask: Task<Void, Never>?
    private var dataSink: DataChannelSink?
    private var forwarder: PortForwarder?
    private var attachedSession: AgentSession?
    /// Role declared in this connection's AttachRequest; governs how its later
    /// resize frames are arbitrated (desktop is geometry-authoritative).
    private var attachedRole: ClientRole = .desktop
    private var helloDone = false
    private var clientBuild: String?
    /// This connection's last-reported app-active state (`client.set_active`).
    /// Held so a dropped link releases its status-engine active count exactly
    /// once, and so repeated same-value reports don't double-count.
    private var reportedActive = false

    private enum Mode {
        case undecided
        case control
        case data
        case forward
    }

    init(
        nwConnection: NWConnection, services: DaemonServices, isRemote: Bool,
        onClose: @escaping @Sendable () -> Void
    ) {
        self.transport = NWTransport(connection: nwConnection)
        self.services = services
        self.isRemote = isRemote
        self.onClose = onClose
    }

    func start() async {
        transport.start()
        do {
            while let chunk = try await transport.receive() {
                try await ingest(chunk)
            }
        } catch {
            DaemonLog.shared.log("connection error: \(error)")
        }
        await close()
    }

    func close() async {
        subscriptionTask?.cancel()
        subscriptionTask = nil
        await forwarder?.cancel()
        forwarder = nil
        if let sink = dataSink, let session = attachedSession {
            await session.detach(sinkID: sink.sinkID)
        }
        dataSink = nil
        if reportedActive {
            reportedActive = false
            await services.statusEngine.clientResignedActive()
        }
        transport.cancel()
        onClose()
    }

    // MARK: Ingest

    private func ingest(_ data: Data) async throws {
        switch mode {
        case .undecided:
            lineAccumulator.append(data)
            guard let newline = lineAccumulator.firstIndex(of: 0x0A) else {
                if lineAccumulator.count > 1 << 20 {
                    throw ControlError.badRequest("first line too large")
                }
                return
            }
            let firstLine = lineAccumulator.subdata(in: lineAccumulator.startIndex..<newline)
            let rest = lineAccumulator.subdata(
                in: lineAccumulator.index(after: newline)..<lineAccumulator.endIndex)
            lineAccumulator = Data()

            if let attach = try? JSONDecoder.dirijor.decode(AttachRequest.self, from: firstLine) {
                mode = .data
                try await beginDataChannel(attach)
                if !rest.isEmpty { try await ingest(rest) }
            } else if let forward = try? JSONDecoder.dirijor.decode(
                ForwardRequest.self, from: firstLine)
            {
                mode = .forward
                try await beginForward(forward)
                if !rest.isEmpty { try await ingest(rest) }
            } else {
                mode = .control
                var replay = firstLine
                replay.append(0x0A)
                replay.append(rest)
                try await ingestControl(replay)
            }

        case .control:
            try await ingestControl(data)

        case .data:
            for frame in try frames.append(data) {
                await handleDataFrame(frame)
            }

        case .forward:
            guard let forwarder else { throw PortForwarderError.connectFailed }
            try await forwarder.write(data)
        }
    }

    private func ingestControl(_ data: Data) async throws {
        for message in try ndjson.append(data) {
            guard case .request(let id, let method, let params) = message else { continue }
            await handleRequest(id: id, method: method, params: params)
        }
    }

    // MARK: Data channel

    private func beginDataChannel(_ attach: AttachRequest) async throws {
        // Remote data channels are token-gated too, validated before we look up
        // the session (so an invalid token learns nothing about what exists).
        if isRemote {
            let expected = services.remote?.token
            guard let expected,
                let presented = attach.token,
                timingSafeEqual(Data(presented.utf8), Data(expected.utf8))
            else {
                transport.cancel()
                throw ControlError.unauthorized()
            }
        }
        guard let session = await services.registry.session(attach.attach) else {
            transport.cancel()
            throw ControlError.notFound(attach.attach.rawValue)
        }
        let sink = DataChannelSink(transport: transport)
        dataSink = sink
        attachedSession = session
        attachedRole = attach.role
        DaemonLog.shared.log("data channel attached to \(attach.attach) role=\(attach.role.rawValue)")
        await session.attach(sink, fromOffset: attach.fromOffset, role: attach.role)
    }

    // MARK: Port forwarding

    private func beginForward(_ request: ForwardRequest) async throws {
        // Remote forwards are token-gated before any session lookup or target
        // dial, so invalid tokens learn nothing about ports or processes.
        if isRemote {
            let expected = services.remote?.token
            guard let expected,
                let presented = request.token,
                timingSafeEqual(Data(presented.utf8), Data(expected.utf8))
            else {
                sendForwardAck(ForwardAck(ok: false, error: "unauthorized"))
                try? await Task.sleep(for: .milliseconds(100))
                transport.cancel()
                throw ControlError.unauthorized()
            }
        }

        if isRemote, services.remote?.forwardAnyPort != true {
            let allowed = await PortForwarder.isPortForwardable(
                request.forward, registry: services.registry)
            guard allowed else {
                sendForwardAck(ForwardAck(ok: false, error: "port_not_allowed"))
                try? await Task.sleep(for: .milliseconds(100))
                transport.cancel()
                throw ControlError.unauthorized()
            }
        }

        let forwarder = PortForwarder(port: request.forward, client: transport)
        self.forwarder = forwarder
        do {
            try await forwarder.dial()
        } catch {
            sendForwardAck(ForwardAck(ok: false, error: "connect_failed"))
            try? await Task.sleep(for: .milliseconds(100))
            transport.cancel()
            throw error
        }

        sendForwardAck(ForwardAck(ok: true))
        await forwarder.startPumping()
        DaemonLog.shared.log("port forward connected to localhost:\(request.forward)")
    }

    private func sendForwardAck(_ ack: ForwardAck) {
        guard var data = try? JSONEncoder.dirijor.encode(ack) else { return }
        data.append(0x0A)
        transport.send(data)
    }

    private func handleDataFrame(_ frame: Frame) async {
        guard let session = attachedSession else { return }
        switch frame.type {
        case .input:
            await session.write(frame.payload)
        case .resize:
            if let (cols, rows) = frame.resizePayload {
                await session.resize(cols: Int(cols), rows: Int(rows), role: attachedRole)
            }
        case .scroll:
            if let s = frame.scrollPayload {
                await session.scroll(
                    dir: s.dir, lines: Int(s.lines), col: Int(s.col), row: Int(s.row))
            }
        case .ping:
            dataSink?.deliver(Frame(type: .pong))
        default:
            break
        }
    }

    // MARK: Control dispatch

    private func handleRequest(id: UInt64, method: String, params: JSONValue?) async {
        do {
            let result = try await dispatch(method: method, params: params)
            await send(.response(id: id, result: .success(result)))
        } catch let error as ControlError {
            await send(.response(id: id, result: .failure(error)))
            // A remote peer that failed authorization gets its response flushed,
            // then the connection is closed — no unauthenticated retries. The
            // brief delay lets the fire-and-forget send reach the socket before
            // cancel() tears the transport down (same reason daemon.shutdown
            // delays its exit).
            if isRemote, error.code == "unauthorized" {
                try? await Task.sleep(for: .milliseconds(100))
                await close()
            }
        } catch {
            await send(.response(id: id, result: .failure(.internalError("\(error)"))))
        }
    }

    private func dispatch(method: String, params: JSONValue?) async throws -> JSONValue {
        // Remote connections are gated: no method other than hello runs until a
        // valid token has been presented. Local (UDS) connections skip this
        // entirely — byte-identical to the pre-remote behavior.
        if isRemote, !helloDone, method != Method.hello {
            throw ControlError.unauthorized()
        }

        switch method {
        case Method.hello:
            let hello: HelloParams = try decode(params)
            guard hello.proto == WireVersion.current else {
                throw ControlError.versionMismatch(
                    "daemon speaks \(WireVersion.current), client sent \(hello.proto)")
            }
            // Remote peers must present the shared token (constant-time compared)
            // before the connection is considered authorized.
            if isRemote {
                let expected = services.remote?.token
                guard let expected,
                    let presented = hello.token,
                    timingSafeEqual(Data(presented.utf8), Data(expected.utf8))
                else {
                    throw ControlError.unauthorized()
                }
            }
            helloDone = true
            clientBuild = hello.build
            return try JSONValue(
                encoding: HelloResult(
                    proto: WireVersion.current, build: services.build,
                    pid: ProcessInfo.processInfo.processIdentifier,
                    executableHash: services.executableHash))

        case Method.sessionSpawn:
            let spawn: SessionSpawnParams = try decode(params)
            let record = try await services.registry.spawn(spawn)
            return try JSONValue(encoding: record)

        case Method.sessionList, Method.stateSnapshot:
            return try JSONValue(encoding: await services.registry.list())

        case Method.governorConfigure:
            let settings: GovernorSettingsParams = try decode(params)
            await services.governor.configure(settings)
            return .object([:])

        case Method.agentReadiness:
            return try JSONValue(encoding: AgentReadiness.inspect())

        case Method.sessionKill:
            let p: SessionIDParams = try decode(params)
            try await services.registry.kill(sessionID: p.sessionID)
            return .object([:])

        case Method.sessionRemove:
            let p: SessionIDParams = try decode(params)
            try await services.registry.remove(sessionID: p.sessionID)
            return .object([:])

        case Method.sessionReopenLast:
            let record = try await services.registry.reopenLastClosed()
            return try JSONValue(encoding: record)

        case Method.sessionHistory:
            let tracked = await services.registry.trackedAgentSessionIDs()
            let entries = HistoryScanner.scan(excluding: tracked)
            return try JSONValue(encoding: SessionHistoryResult(entries: entries))

        case Method.sessionResumeFromHistory:
            let p: ResumeFromHistoryParams = try decode(params)
            let record = try await services.registry.resumeFromHistory(p.entry)
            return try JSONValue(encoding: record)

        case Method.sessionRename:
            let p: SessionRenameParams = try decode(params)
            try await services.registry.rename(sessionID: p.sessionID, title: p.title)
            return .object([:])

        case Method.sessionResume:
            let p: SessionIDParams = try decode(params)
            let record = try await services.registry.resume(sessionID: p.sessionID)
            return try JSONValue(encoding: record)

        case Method.sessionMigrate:
            let p: SessionMigrateParams = try decode(params)
            let result = try await services.registry.migrate(
                sessionID: p.sessionID, targetHostID: p.targetHost)
            return try JSONValue(encoding: result)

        case Method.hostSyncPrefs:
            let p: HostSyncPrefsParams = try decode(params)
            let result = try await services.registry.syncPrefs(hostID: p.host)
            return try JSONValue(encoding: result)

        case Method.hostLocateRepo:
            let p: HostLocateRepoParams = try decode(params)
            let result = try await services.registry.locateRepo(p)
            return try JSONValue(encoding: result)

        case Method.sessionSendText:
            let p: SendTextParams = try decode(params)
            guard let session = await services.registry.session(p.sessionID) else {
                throw ControlError.notFound(p.sessionID.rawValue)
            }
            await session.sendText(p.text, submit: p.submit)
            return .object([:])

        case Method.sessionResize:
            let p: ResizeParams = try decode(params)
            guard let session = await services.registry.session(p.sessionID) else {
                throw ControlError.notFound(p.sessionID.rawValue)
            }
            await session.resize(cols: p.cols, rows: p.rows)
            return .object([:])

        case Method.sessionSetOwner:
            let p: SessionSetOwnerParams = try decode(params)
            try await services.registry.setSessionOwner(sessionID: p.sessionID, role: p.role)
            return .object([:])

        case Method.sessionReadScreen:
            let p: SessionIDParams = try decode(params)
            guard let session = await services.registry.session(p.sessionID) else {
                throw ControlError.notFound(p.sessionID.rawValue)
            }
            let snapshot = await session.makeSnapshot()
            return try JSONValue(
                encoding: ReadScreenResult(
                    text: snapshot.lines.joined(separator: "\n"),
                    cols: snapshot.cols, rows: snapshot.rows))

        case Method.sessionReadScrollback:
            let p: SessionIDParams = try decode(params)
            guard let session = await services.registry.session(p.sessionID) else {
                throw ControlError.notFound(p.sessionID.rawValue)
            }
            return try JSONValue(encoding: await session.readScrollback())

        case Method.sessionReadScrollbackCells:
            let p: ReadScrollbackCellsParams = try decode(params)
            guard let session = await services.registry.session(p.sessionID) else {
                throw ControlError.notFound(p.sessionID.rawValue)
            }
            return try JSONValue(
                encoding: await session.readScrollbackCells(firstRow: p.firstRow, maxRows: p.maxRows))

        case Method.sessionReadDiff:
            let p: SessionReadDiffParams = try decode(params)
            return try JSONValue(
                encoding: try await services.registry.readDiff(
                    sessionID: p.sessionID, base: p.base ?? .defaultBranch))

        case Method.sessionMarkSeen:
            let p: SessionIDParams = try decode(params)
            try await services.registry.markSeen(sessionID: p.sessionID)
            return .object([:])

        case Method.sessionHibernate:
            let p: SessionRefParams = try decode(params)
            try await services.registry.hibernate(sessionID: p.sessionID, reason: .manual)
            return .object([:])

        case Method.sessionWake:
            let p: SessionRefParams = try decode(params)
            try await services.registry.wakeSession(sessionID: p.sessionID)
            return .object([:])

        case Method.sessionArchive:
            let p: SessionRefParams = try decode(params)
            try await services.registry.archive(sessionID: p.sessionID)
            return .object([:])

        case Method.sessionUnarchive:
            let p: SessionRefParams = try decode(params)
            try await services.registry.unarchive(sessionID: p.sessionID)
            return .object([:])

        case Method.projectAdd:
            let p: ProjectAddParams = try decode(params)
            let project = await services.registry.addProject(root: p.root)
            return try JSONValue(encoding: project)

        case Method.testRun:
            let p: TestRunParams = try decode(params)
            return try await services.browserPool.run(p)

        case Method.browserAct:
            let p: BrowserParams = try decode(params)
            return try await services.browserPool.browse(p)

        case Method.worktreeCreate:
            let p: WorktreeCreateParams = try decode(params)
            let info = try GitWorktrees.create(
                repoPath: p.repoPath, branch: p.branch, base: p.base)
            await services.events.publish(
                name: EventName.worktreeCreated,
                encoding: WorktreeEvent(
                    repoPath: p.repoPath, path: info.path, branch: info.branch))
            return try JSONValue(encoding: info)

        case Method.worktreeList:
            let p: WorktreeListParams = try decode(params)
            return try JSONValue(encoding: GitWorktrees.list(repoPath: p.repoPath))

        case Method.worktreeRemove:
            let p: WorktreeRemoveParams = try decode(params)
            try GitWorktrees.remove(
                repoPath: p.repoPath, worktreePath: p.worktreePath, force: p.force)
            await services.events.publish(
                name: EventName.worktreeRemoved,
                encoding: WorktreeEvent(repoPath: p.repoPath, path: p.worktreePath))
            return .object([:])

        case Method.worktreeOverview:
            let result = await WorktreeOverviewBuilder.build(registry: services.registry)
            return try JSONValue(encoding: result)

        case Method.eventsSubscribe:
            let p: EventsSubscribeParams = (try? decode(params)) ?? EventsSubscribeParams()
            try await beginEventSubscription(
                sinceSeq: p.sinceSeq,
                filter: EventBus.Filter(
                    sessions: p.sessions.map(Set.init), kinds: p.kinds.map(Set.init)))
            return .object(["subscribed": .bool(true)])

        case Method.eventsWait:
            let p: EventsWaitParams = try decode(params)
            return try await waitForEvent(p)

        case Method.hookReport:
            let p: HookReportParams = try decode(params)
            await services.statusEngine.ingestHookReport(p)
            return .object([:])

        case Method.clientSetActive:
            let p: ClientActiveParams = try decode(params)
            // Idempotent per connection: only the true→false / false→true edges
            // touch the engine's reference count.
            if p.active != reportedActive {
                reportedActive = p.active
                if p.active {
                    await services.statusEngine.clientBecameActive()
                } else {
                    await services.statusEngine.clientResignedActive()
                }
            }
            return .object([:])

        case Method.daemonPrepareShutdown:
            DaemonLog.shared.log(
                "daemon.prepare_shutdown from \(clientBuild ?? "pre-hello client")\(isRemote ? " (remote)" : "")")
            await services.registry.prepareShutdown()
            return .object([:])

        case Method.daemonShutdown:
            // Ack first (the handler delays briefly so this response can flush),
            // then the daemon persists and exits; the client sees the drop and
            // relaunches the fresh binary.
            DaemonLog.shared.log(
                "daemon.shutdown from \(clientBuild ?? "pre-hello client")\(isRemote ? " (remote)" : "")")
            services.shutdownRequest.fire()
            return .object([:])

        default:
            throw ControlError.badRequest("unknown method \(method)")
        }
    }

    private func decode<T: Decodable>(_ params: JSONValue?) throws -> T {
        guard let params else { throw ControlError.badRequest("missing params") }
        do {
            return try params.decoded()
        } catch {
            throw ControlError.badRequest("bad params for \(T.self): \(error)")
        }
    }

    // MARK: Events

    /// Pumps the subscription onto the socket with `sendAwaiting`, so the socket
    /// itself is the backpressure signal: while a peer isn't draining, this loop
    /// stops pulling and the overflow is absorbed by the bus's bounded
    /// per-subscriber queue (which reports the hole via `events.dropped`). With
    /// fire-and-forget `send` the queue would instead grow inside NWConnection,
    /// unbounded and invisible — which is the failure mode a dead `dirijor
    /// events subscribe` on a laptop that slept would produce.
    private func beginEventSubscription(sinceSeq: UInt64?, filter: EventBus.Filter) async throws {
        subscriptionTask?.cancel()
        let stream = await services.events.subscribe(sinceSeq: sinceSeq, filter: filter)
        subscriptionTask = Task { [weak self] in
            for await event in stream {
                guard !Task.isCancelled, let self else { break }
                do {
                    try await self.sendEventAwaiting(event)
                } catch {
                    break  // peer is gone; the stream terminates with this task
                }
            }
        }
    }

    /// `nonisolated` on purpose: awaiting the socket write must not hold this
    /// connection's actor, or a stalled event stream would also stall the
    /// request/response traffic multiplexed on the same connection.
    private nonisolated func sendEventAwaiting(_ event: EventBus.Event) async throws {
        let data = try NDJSONBuffer.encode(
            .event(name: event.name, seq: event.seq, params: event.params))
        try await transport.sendAwaiting(data)
    }

    /// One-shot long poll. Two match modes, OR'd: a status target on a session
    /// (`until`) and an event-name target (`kinds`). Both are checked against
    /// the same live subscription, so no caller has to poll.
    private func waitForEvent(_ params: EventsWaitParams) async throws -> JSONValue {
        let untilTargets = params.until
        let kinds = Set(params.kinds ?? [])
        // Neither match mode set means the call can only ever time out. Say so
        // now rather than holding a connection open for ten minutes to return
        // "timedOut" that the caller can't act on.
        guard !untilTargets.isEmpty || !kinds.isEmpty else {
            throw ControlError.badRequest("events.wait needs `until` statuses or `kinds`")
        }

        func matches(_ record: SessionRecord) -> Bool {
            guard !untilTargets.isEmpty else { return false }
            if let scope = params.sessionID, record.id != scope { return false }
            return untilTargets.contains { record.status.satisfies(waitTarget: $0) }
        }

        // Subscribe before the pre-check so a transition landing between the
        // two is buffered rather than lost.
        let stream = await services.events.subscribe(
            filter: EventBus.Filter(
                sessions: params.sessionID.map { [$0] },
                // A status wait needs session.updated; a kinds wait needs its
                // own names. Nil (everything) only when neither narrows it.
                kinds: kinds.isEmpty
                    ? (untilTargets.isEmpty ? nil : [EventName.sessionUpdated])
                    : kinds.union(untilTargets.isEmpty ? [] : [EventName.sessionUpdated])))

        if let scope = params.sessionID, let record = await services.registry.record(scope),
            matches(record)
        {
            return try JSONValue(encoding: EventsWaitResult(session: record, timedOut: false))
        }

        let deadline = ContinuousClock.now + .milliseconds(params.timeoutMs)

        while ContinuousClock.now < deadline {
            let remaining = deadline - ContinuousClock.now
            // The iterator is made inside the closure because AsyncStream's
            // iterator is a thin handle onto shared storage — a fresh one
            // resumes where the last left off, and it keeps the mutation out
            // of the escaping @Sendable closure.
            let next = await withTimeout(remaining) {
                var iterator = stream.makeAsyncIterator()
                return await iterator.next()
            }
            guard case .some(.some(let event)) = next else { break }

            if kinds.contains(event.name) {
                var record: SessionRecord?
                if let id = event.sessionID { record = await services.registry.record(id) }
                return try JSONValue(
                    encoding: EventsWaitResult(
                        session: record, timedOut: false,
                        event: EventEnvelope(
                            name: event.name, seq: event.seq, params: event.params)))
            }
            guard event.name == EventName.sessionUpdated,
                let record = try? event.params.decoded(as: SessionRecord.self)
            else { continue }
            if matches(record) {
                return try JSONValue(encoding: EventsWaitResult(session: record, timedOut: false))
            }
        }

        // Timed out. A session-scoped wait still reports the session's final
        // state (callers render it); an unknown session is a caller bug.
        guard let scope = params.sessionID else {
            return try JSONValue(encoding: EventsWaitResult(session: nil, timedOut: true))
        }
        guard let record = await services.registry.record(scope) else {
            throw ControlError.notFound(scope.rawValue)
        }
        return try JSONValue(encoding: EventsWaitResult(session: record, timedOut: true))
    }

    private func send(_ message: ControlMessage) async {
        guard let data = try? NDJSONBuffer.encode(message) else { return }
        transport.send(data)
    }
}

/// Constant-time byte comparison for secrets (tokens). Runs in time dependent
/// only on the lengths, never on where the first mismatch is, so a remote peer
/// can't time-probe the token byte by byte. Unequal lengths fail immediately —
/// length is not itself a secret.
func timingSafeEqual(_ a: Data, _ b: Data) -> Bool {
    guard a.count == b.count else { return false }
    return a.withUnsafeBytes { (pa: UnsafeRawBufferPointer) -> Bool in
        b.withUnsafeBytes { (pb: UnsafeRawBufferPointer) -> Bool in
            var diff: UInt8 = 0
            for i in 0..<pa.count { diff |= pa[i] ^ pb[i] }
            return diff == 0
        }
    }
}

/// Runs an async operation with a timeout; nil result means timed out.
func withTimeout<T: Sendable>(
    _ duration: Duration, _ operation: @escaping @Sendable () async -> T
) async -> T? {
    await withTaskGroup(of: T?.self) { group in
        group.addTask { await operation() }
        group.addTask {
            try? await Task.sleep(for: duration)
            return nil
        }
        let first = await group.next() ?? nil
        group.cancelAll()
        return first
    }
}

// MARK: - Worktree overview

/// Builds `worktree.overview`: every worktree of every known project root,
/// joined with the session (if any) running inside it, plus git hygiene bits
/// (dirty / merged / age) and a suggest-only stale flag. Never deletes.
enum WorktreeOverviewBuilder {
    static func build(registry: SessionRegistry) async -> WorktreeOverviewResult {
        let listing = await registry.list()

        // Join sessions by worktree path (fallback cwd); a live session wins
        // over an exited one that shares the path.
        var sessionByPath: [String: SessionRecord] = [:]
        for record in listing.sessions {
            let path = record.worktreePath ?? record.cwd
            if let existing = sessionByPath[path],
                existing.status.isRunning || !record.status.isRunning
            {
                continue
            }
            sessionByPath[path] = record
        }

        var entries: [WorktreeOverviewEntry] = []
        var seenPaths: Set<String> = []
        for project in listing.projects.sorted(by: { $0.root < $1.root }) {
            guard GitWorktrees.isRepository(project.root),
                let worktrees = try? GitWorktrees.list(repoPath: project.root)
            else { continue }

            let defaultBranch = defaultBranch(in: project.root)
            let mergedBranches = mergedBranches(in: project.root, into: defaultBranch)

            for worktree in worktrees where !worktree.isBare {
                guard seenPaths.insert(worktree.path).inserted else { continue }
                let isMain = worktree.path == project.root
                let dirty = isDirty(at: worktree.path)
                let merged =
                    worktree.branch.map { $0 != defaultBranch && mergedBranches.contains($0) }
                    ?? false
                let age = ageDays(of: worktree.path)
                let record = sessionByPath[worktree.path]
                let sessionAlive = record?.status.isRunning ?? false
                entries.append(
                    WorktreeOverviewEntry(
                        path: worktree.path,
                        branch: worktree.branch,
                        projectRoot: project.root,
                        sessionID: record?.id,
                        sessionStatus: record?.status,
                        dirty: dirty,
                        merged: merged,
                        ageDays: age,
                        staleSuggestion: !isMain && !sessionAlive && merged && !dirty && age > 7
                    ))
            }
        }
        return WorktreeOverviewResult(entries: entries)
    }

    /// Repo's default branch: `origin/HEAD` symbolic ref, falling back to "main".
    static func defaultBranch(in repoPath: String) -> String {
        guard let ref = runGit(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"], in: repoPath),
            let short = ref.split(separator: "/").last, !short.isEmpty
        else { return "main" }
        return String(short)
    }

    /// Branch names fully merged into `branch` (empty when the branch is unknown).
    static func mergedBranches(in repoPath: String, into branch: String) -> Set<String> {
        guard
            let output = runGit(
                ["branch", "--merged", branch, "--format=%(refname:short)"], in: repoPath)
        else { return [] }
        return Set(output.split(separator: "\n").map(String.init))
    }

    static func isDirty(at path: String) -> Bool {
        guard let output = runGit(["status", "--porcelain"], in: path) else { return false }
        return !output.isEmpty
    }

    static func ageDays(of path: String) -> Int {
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: path) else { return 0 }
        let created = (attrs[.creationDate] ?? attrs[.modificationDate]) as? Date
        guard let created else { return 0 }
        return max(0, Int(Date().timeIntervalSince(created) / 86_400))
    }

    /// Local git shell-out (mirrors GitWorktreesImpl's pattern) — kept here so
    /// DirijorGit's frozen public API stays untouched. nil on any failure.
    private static func runGit(_ args: [String], in directory: String) -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/git")
        process.arguments = args
        process.currentDirectoryURL = URL(fileURLWithPath: directory)
        let stdout = Pipe()
        process.standardOutput = stdout
        process.standardError = Pipe()
        do { try process.run() } catch { return nil }
        let data = stdout.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else { return nil }
        return String(decoding: data, as: UTF8.self)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

// MARK: - Transport

/// Thin sendable wrapper around NWConnection: async receive, fire-and-forget
/// send with a pending-bytes counter for backpressure decisions.
final class NWTransport: @unchecked Sendable {
    private let connection: NWConnection
    private let pending = PendingCounter()

    init(connection: NWConnection) {
        self.connection = connection
    }

    func start() {
        connection.start(queue: DispatchQueue(label: "dirijor.conn"))
    }

    func cancel() {
        connection.cancel()
    }

    var pendingBytes: Int { pending.value }

    func send(_ data: Data) {
        pending.add(data.count)
        connection.send(
            content: data,
            completion: .contentProcessed { [pending] _ in
                pending.add(-data.count)
            })
    }

    func sendAwaiting(_ data: Data) async throws {
        guard !data.isEmpty else { return }
        pending.add(data.count)
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            connection.send(
                content: data,
                completion: .contentProcessed { [pending] error in
                    pending.add(-data.count)
                    if let error {
                        continuation.resume(throwing: error)
                    } else {
                        continuation.resume()
                    }
                })
        }
    }

    /// Returns nil on clean EOF.
    func receive() async throws -> Data? {
        try await withCheckedThrowingContinuation { continuation in
            connection.receive(minimumIncompleteLength: 1, maximumLength: 256 << 10) {
                data, _, isComplete, error in
                if let error {
                    continuation.resume(throwing: error)
                } else if let data, !data.isEmpty {
                    continuation.resume(returning: data)
                } else if isComplete {
                    continuation.resume(returning: nil)
                } else {
                    continuation.resume(returning: Data())
                }
            }
        }
    }
}

private final class PendingCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0

    var value: Int {
        lock.lock()
        defer { lock.unlock() }
        return count
    }

    func add(_ delta: Int) {
        lock.lock()
        count += delta
        lock.unlock()
    }
}

/// SessionOutputSink over a data-channel transport.
final class DataChannelSink: SessionOutputSink, @unchecked Sendable {
    let sinkID = UUID()
    private let transport: NWTransport

    init(transport: NWTransport) {
        self.transport = transport
    }

    var pendingBytes: Int { transport.pendingBytes }

    func deliver(_ frame: Frame) {
        transport.send(FrameCodec.encode(frame))
    }

    func closeSink() {
        transport.cancel()
    }
}
