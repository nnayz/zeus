import Foundation
import Network
import DirijorCore
import DirijorProtocol

/// The app-side daemon client: one control connection to `dirijord`, with
/// request/response correlation, an event fan-out, and automatic reconnect.
///
/// The `NWConnection` is created, started, and torn down entirely inside this
/// actor; its `Network.framework` callbacks hop back onto the actor via
/// `Task { await … }`, so the non-`Sendable` connection never crosses an
/// isolation boundary.
public actor DaemonClient {

    // MARK: Configuration

    private let endpoint: DaemonEndpoint
    private let build: String
    private let token: String?

    /// Dedicated queue for all `NWConnection` callbacks.
    private let queue: DispatchQueue

    // MARK: Connection state

    private var connection: NWConnection?
    /// Bumped for every connection attempt; stale callbacks are ignored.
    private var connectionGeneration = 0
    /// True once the connection is `.ready` (safe to send).
    private var isEstablished = false
    private var helloResult: HelloResult?

    private var lifecycleTask: Task<Void, Never>?
    /// Per-connection idle heartbeat. Sends a cheap `hello` every
    /// `heartbeatInterval`s so a silently-dropped link (mobile/Tailscale) is
    /// detected promptly; on failure the connection is dropped and the lifecycle
    /// loop reconnects. Reset on every attempt via the connection generation.
    private var heartbeatTask: Task<Void, Never>?
    private static let heartbeatInterval: Duration = .seconds(25)
    private var backoffSeconds = 0.5
    private static let maxBackoffSeconds = 8.0

    private var ndjson = NDJSONBuffer()

    // Handshake / lifecycle continuations for the current attempt.
    private var readyContinuation: CheckedContinuation<Void, Error>?
    private var closedContinuation: CheckedContinuation<Error?, Never>?
    private var closeAlreadyHappened = false
    private var storedCloseError: Error?

    // MARK: Request correlation

    private struct PendingRequest {
        let continuation: CheckedContinuation<JSONValue, Error>
        var timeout: Task<Void, Never>?
    }
    private var nextRequestID: UInt64 = 0
    private var pending: [UInt64: PendingRequest] = [:]

    // MARK: Events

    private var wantEvents = false
    private var lastSeq: UInt64 = 0
    private var eventConsumers: [UUID: AsyncStream<EventEnvelope>.Continuation] = [:]

    // MARK: Connection-state fan-out

    private var currentState: ClientConnectionState = .disconnected(nil)
    private var stateConsumers: [UUID: AsyncStream<ClientConnectionState>.Continuation] = [:]

    public typealias EventEnvelope = (name: String, seq: UInt64, params: JSONValue)

    // MARK: Init

    public init(
        endpoint: DaemonEndpoint = .default,
        build: String,
        token: String? = nil
    ) {
        self.endpoint = endpoint
        self.build = build
        self.token = token
        self.queue = DispatchQueue(label: "dev.dirijor.client.control")
    }

    // MARK: Public API — lifecycle

    /// Most recent successful handshake result, if any.
    public var lastHello: HelloResult? { helloResult }

    /// Starts the connect/reconnect loop (idempotent). Observe progress via
    /// `connectionState`.
    public func connect() {
        guard lifecycleTask == nil else { return }
        lifecycleTask = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                await self.runOnce()
                if Task.isCancelled { break }
                await self.backoffSleep()
            }
        }
    }

    /// Permanently stops the client: cancels the loop, fails all pending
    /// requests, and finishes every stream. Safe to call from `deinit` sites via
    /// an explicit `await`.
    public func shutdown() {
        lifecycleTask?.cancel()
        lifecycleTask = nil
        heartbeatTask?.cancel()
        heartbeatTask = nil
        isEstablished = false
        connectionGeneration += 1
        connection?.cancel()
        connection = nil
        failPending(with: ControlError(code: "disconnected", message: "client shut down"))
        helloResult = nil
        emit(.disconnected(nil))
        for (_, c) in eventConsumers { c.finish() }
        eventConsumers.removeAll()
        for (_, c) in stateConsumers { c.finish() }
        stateConsumers.removeAll()
    }

    /// A fresh stream of connection-state transitions. The current state is
    /// delivered immediately on subscription. Multiple subscribers are supported.
    public var connectionState: AsyncStream<ClientConnectionState> {
        let id = UUID()
        let current = currentState
        return AsyncStream(bufferingPolicy: .bufferingNewest(16)) { continuation in
            continuation.yield(current)
            stateConsumers[id] = continuation
            continuation.onTermination = { [weak self] _ in
                Task { await self?.removeStateConsumer(id) }
            }
        }
    }

    // MARK: Public API — requests

    /// Sends a request and returns the raw result. Throws `ControlError` on a
    /// daemon `.failure`, on disconnect (`code: "disconnected"`), or on timeout
    /// (`code: "timeout"`).
    ///
    /// - Parameter timeout: optional per-request deadline. Used by long-poll
    ///   methods (e.g. `events.wait`) whose server-side timeout exceeds any
    ///   default.
    public func request<P: Encodable>(
        _ method: String,
        params: P?,
        timeout: Duration? = nil
    ) async throws -> JSONValue {
        let id = allocateRequestID()
        let paramsValue = try params.map { try JSONValue(encoding: $0) }
        let message = ControlMessage.request(id: id, method: method, params: paramsValue)
        let data = try NDJSONBuffer.encode(message)

        return try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<JSONValue, Error>) in
            guard isEstablished, let connection else {
                continuation.resume(throwing: ControlError(code: "disconnected", message: "not connected to daemon"))
                return
            }
            var entry = PendingRequest(continuation: continuation)
            if let timeout {
                entry.timeout = Task { [weak self] in
                    try? await Task.sleep(for: timeout)
                    if Task.isCancelled { return }
                    await self?.timeoutRequest(id)
                }
            }
            pending[id] = entry
            connection.send(content: data, completion: .contentProcessed { [weak self] error in
                guard let error else { return }
                Task { await self?.connectionDidFail(error) }
            })
        }
    }

    /// Convenience: no params.
    public func request(_ method: String, timeout: Duration? = nil) async throws -> JSONValue {
        try await request(method, params: JSONValue?.none, timeout: timeout)
    }

    /// Sends a request and decodes the result into `R`.
    public func request<P: Encodable, R: Decodable>(
        _ method: String,
        params: P?,
        as type: R.Type,
        timeout: Duration? = nil
    ) async throws -> R {
        let value = try await request(method, params: params, timeout: timeout)
        return try value.decoded(as: R.self)
    }

    /// Convenience: decoded result, no params.
    public func request<R: Decodable>(_ method: String, as type: R.Type, timeout: Duration? = nil) async throws -> R {
        try await request(method, params: JSONValue?.none, as: type, timeout: timeout)
    }

    // MARK: Public API — events

    /// A fan-out stream of daemon events. The first subscriber triggers
    /// `events.subscribe` (with `sinceSeq = lastSeq` for gapless resume across
    /// reconnects). Multiple subscribers each receive every event.
    public func events() -> AsyncStream<EventEnvelope> {
        let id = UUID()
        return AsyncStream(bufferingPolicy: .unbounded) { continuation in
            eventConsumers[id] = continuation
            continuation.onTermination = { [weak self] _ in
                Task { await self?.removeEventConsumer(id) }
            }
            if !wantEvents {
                wantEvents = true
                if isEstablished { sendSubscribe() }
            }
        }
    }

    // MARK: Public API — typed wrappers

    public func sessions() async throws -> SessionListResult {
        try await request(Method.sessionList, as: SessionListResult.self)
    }

    /// Spawns a session; the daemon returns the full SessionRecord.
    public func spawn(_ params: SessionSpawnParams) async throws -> SessionID {
        try await request(Method.sessionSpawn, params: params, as: SessionRecord.self).id
    }

    public func kill(_ sessionID: SessionID) async throws {
        _ = try await request(Method.sessionKill, params: SessionIDParams(sessionID: sessionID))
    }

    public func remove(_ sessionID: SessionID) async throws {
        _ = try await request(Method.sessionRemove, params: SessionIDParams(sessionID: sessionID))
    }

    public func rename(_ sessionID: SessionID, title: String) async throws {
        _ = try await request(Method.sessionRename, params: SessionRenameParams(sessionID: sessionID, title: title))
    }

    /// Resumes a previously-exited session, returning the (same) session id.
    public func resume(_ sessionID: SessionID) async throws -> SessionID {
        try await request(Method.sessionResume, params: SessionIDParams(sessionID: sessionID), as: SessionIDParams.self).sessionID
    }

    /// Reopens the most recently closed session (browser-style ⌘⇧T): first-class
    /// agents resume their conversation; shells respawn in the same folder.
    public func reopenLastClosed() async throws -> SessionRecord {
        try await request(Method.sessionReopenLast, as: SessionRecord.self)
    }

    /// Past conversations discovered by scanning the agents' on-disk transcript
    /// stores — survives daemon restart, unlike `reopenLastClosed`.
    public func history() async throws -> SessionHistoryResult {
        try await request(Method.sessionHistory, as: SessionHistoryResult.self)
    }

    /// Resumes a conversation discovered via `history()` into a fresh session.
    public func resumeFromHistory(_ entry: HistoryEntry) async throws -> SessionRecord {
        try await request(
            Method.sessionResumeFromHistory,
            params: ResumeFromHistoryParams(entry: entry),
            as: SessionRecord.self)
    }

    public func markSeen(_ sessionID: SessionID) async throws {
        _ = try await request(Method.sessionMarkSeen, params: SessionIDParams(sessionID: sessionID))
    }

    /// Report whether the app is frontmost so the daemon can slow its status
    /// tick while backgrounded. Best-effort: an old daemon lacking the method
    /// just rejects it, which callers ignore.
    public func setActive(_ active: Bool) async throws {
        _ = try await request(Method.clientSetActive, params: ClientActiveParams(active: active))
    }

    public func configureGovernor(_ settings: GovernorSettingsParams) async throws {
        _ = try await request(Method.governorConfigure, params: settings)
    }

    public func agentReadiness() async throws -> AgentReadinessResult {
        try await request(Method.agentReadiness, as: AgentReadinessResult.self)
    }

    /// Freezes the session's process tree (SIGSTOP). Manual counterpart to the
    /// governor's idle hibernation.
    public func hibernate(_ sessionID: SessionID) async throws {
        _ = try await request(Method.sessionHibernate, params: SessionIDParams(sessionID: sessionID))
    }

    /// Thaws a hibernated session (SIGCONT). Selecting a tab already auto-wakes
    /// via the data-channel attach; this is the explicit control.
    public func wake(_ sessionID: SessionID) async throws {
        _ = try await request(Method.sessionWake, params: SessionIDParams(sessionID: sessionID))
    }

    /// Kills the session's whole process tree but keeps its record so the
    /// conversation can be revived later via `resume`.
    public func archive(_ sessionID: SessionID) async throws {
        _ = try await request(Method.sessionArchive, params: SessionIDParams(sessionID: sessionID))
    }

    /// Brings an archived record back into the normal list without respawning.
    public func unarchive(_ sessionID: SessionID) async throws {
        _ = try await request(Method.sessionUnarchive, params: SessionIDParams(sessionID: sessionID))
    }

    /// Every worktree across all known project roots, joined with any session
    /// running inside it, plus dirty/merged/age and a suggest-only stale flag.
    public func worktreeOverview() async throws -> [WorktreeOverviewEntry] {
        try await request(Method.worktreeOverview, params: WorktreeOverviewParams(), as: WorktreeOverviewResult.self).entries
    }

    /// Convenience overload mirroring the daemon's `worktree.remove` params.
    public func removeWorktree(repoPath: String, worktreePath: String, force: Bool = false) async throws {
        _ = try await request(
            Method.worktreeRemove,
            params: WorktreeRemoveParams(repoPath: repoPath, worktreePath: worktreePath, force: force))
    }

    public func sendText(_ sessionID: SessionID, text: String, submit: Bool = true) async throws {
        _ = try await request(Method.sessionSendText, params: SendTextParams(sessionID: sessionID, text: text, submit: submit))
    }

    public func resize(_ sessionID: SessionID, cols: Int, rows: Int) async throws {
        _ = try await request(Method.sessionResize, params: ResizeParams(sessionID: sessionID, cols: cols, rows: rows))
    }

    /// Claims geometry ownership of a session in the hand-off model: `.desktop`
    /// makes the Mac reclaim, `.mobile` makes the phone (re)claim.
    public func setSessionOwner(_ sessionID: SessionID, role: ClientRole) async throws {
        _ = try await request(
            Method.sessionSetOwner,
            params: SessionSetOwnerParams(sessionID: sessionID, role: role))
    }

    public func readScreen(_ sessionID: SessionID) async throws -> ReadScreenResult {
        try await request(Method.sessionReadScreen, params: SessionIDParams(sessionID: sessionID), as: ReadScreenResult.self)
    }

    public func readDiff(
        _ sessionID: SessionID, base: SessionDiffBase = .defaultBranch
    ) async throws -> SessionReadDiffResult {
        try await request(
            Method.sessionReadDiff,
            params: SessionReadDiffParams(sessionID: sessionID, base: base),
            as: SessionReadDiffResult.self)
    }

    public func readScrollback(_ sessionID: SessionID) async throws -> ReadScrollbackResult {
        try await request(
            Method.sessionReadScrollback, params: SessionIDParams(sessionID: sessionID),
            as: ReadScrollbackResult.self)
    }

    public func readScrollbackCells(
        _ sessionID: SessionID, firstRow: Int, maxRows: Int
    ) async throws -> ReadScrollbackCellsResult {
        try await request(
            Method.sessionReadScrollbackCells,
            params: ReadScrollbackCellsParams(sessionID: sessionID, firstRow: firstRow, maxRows: maxRows),
            as: ReadScrollbackCellsResult.self)
    }

    public func worktrees(repoPath: String) async throws -> [WorktreeInfo] {
        let value = try await request(Method.worktreeList, params: WorktreeListParams(repoPath: repoPath))
        if let array = try? value.decoded(as: [WorktreeInfo].self) { return array }
        struct Wrapper: Decodable { let worktrees: [WorktreeInfo] }
        return try value.decoded(as: Wrapper.self).worktrees
    }

    public func createWorktree(_ params: WorktreeCreateParams) async throws -> WorktreeInfo {
        let value = try await request(Method.worktreeCreate, params: params)
        if let info = try? value.decoded(as: WorktreeInfo.self) { return info }
        struct Wrapper: Decodable { let worktree: WorktreeInfo }
        return try value.decoded(as: Wrapper.self).worktree
    }

    public func removeWorktree(_ params: WorktreeRemoveParams) async throws {
        _ = try await request(Method.worktreeRemove, params: params)
    }

    public func addProject(root: String) async throws -> Project {
        let value = try await request(Method.projectAdd, params: ProjectAddParams(root: root))
        if let project = try? value.decoded(as: Project.self) { return project }
        struct Wrapper: Decodable { let project: Project }
        return try value.decoded(as: Wrapper.self).project
    }

    /// Long-poll wait. Uses a per-request timeout comfortably above the
    /// server-side wait so the request itself never expires first.
    public func eventsWait(_ params: EventsWaitParams) async throws -> EventsWaitResult {
        let slack = Duration.milliseconds(params.timeoutMs + 10_000)
        return try await request(Method.eventsWait, params: params, as: EventsWaitResult.self, timeout: slack)
    }

    public func prepareShutdown() async throws {
        _ = try await request(Method.daemonPrepareShutdown)
    }

    /// Asks the daemon to persist state and EXIT — used to replace a stale
    /// daemon binary. The connection drops right after; that's expected, so
    /// errors are swallowed.
    public func requestDaemonShutdown() async {
        _ = try? await request(Method.daemonShutdown)
    }

    // MARK: Connection lifecycle

    private func runOnce() async {
        connectionGeneration += 1
        let generation = connectionGeneration
        closeAlreadyHappened = false
        storedCloseError = nil
        readyContinuation = nil
        closedContinuation = nil
        ndjson = NDJSONBuffer()
        emit(.connecting)

        let connection = endpoint.makeConnection()
        self.connection = connection
        connection.stateUpdateHandler = { [weak self] state in
            Task { await self?.handleState(state, generation: generation) }
        }

        do {
            try await awaitReady(connection)          // sets isEstablished on .ready
            startReceiveLoop(generation: generation)
            let hello = try await performHello()
            helloResult = hello
            backoffSeconds = 0.5                        // stable connection → fast future reconnect
            emit(.connected(hello))
            if wantEvents { sendSubscribe() }
            startHeartbeat(generation: generation)
            let dropError = await awaitClosed()
            teardown(error: dropError)
        } catch {
            teardown(error: error)
        }
    }

    private func backoffSleep() async {
        let seconds = backoffSeconds
        backoffSeconds = min(backoffSeconds * 2, Self.maxBackoffSeconds)
        try? await Task.sleep(for: .seconds(seconds))
    }

    private func awaitReady(_ connection: NWConnection) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            readyContinuation = continuation
            // Start only after the continuation is stored, so the `.ready`
            // callback (which hops via Task) can never race ahead of it.
            connection.start(queue: queue)
        }
    }

    private func awaitClosed() async -> Error? {
        if closeAlreadyHappened { return storedCloseError }
        return await withCheckedContinuation { (continuation: CheckedContinuation<Error?, Never>) in
            closedContinuation = continuation
        }
    }

    private func performHello() async throws -> HelloResult {
        try await request(Method.hello, params: HelloParams(build: build, token: token), as: HelloResult.self)
    }

    private func handleState(_ state: NWConnection.State, generation: Int) {
        guard generation == connectionGeneration else { return }
        switch state {
        case .ready:
            isEstablished = true
            if let continuation = readyContinuation {
                readyContinuation = nil
                continuation.resume()
            }
        case .waiting:
            // Still trying to connect (e.g. socket not up yet); NW keeps retrying.
            emit(.connecting)
        case .failed(let error):
            connectionDidFail(error)
        case .cancelled:
            connectionDidFail(nil)
        default:
            break
        }
    }

    private func startReceiveLoop(generation: Int) {
        guard generation == connectionGeneration, let connection else { return }
        connection.receive(minimumIncompleteLength: 1, maximumLength: 1 << 16) { [weak self] data, _, isComplete, error in
            Task { await self?.handleReceived(data: data, isComplete: isComplete, error: error, generation: generation) }
        }
    }

    private func handleReceived(data: Data?, isComplete: Bool, error: NWError?, generation: Int) {
        guard generation == connectionGeneration else { return }
        if let data, !data.isEmpty {
            do {
                let messages = try ndjson.append(data)
                for message in messages { route(message) }
            } catch {
                connectionDidFail(error)
                connection?.cancel()
                return
            }
        }
        if let error {
            connectionDidFail(error)
            connection?.cancel()
            return
        }
        if isComplete {
            connectionDidFail(nil)
            connection?.cancel()
            return
        }
        startReceiveLoop(generation: generation)
    }

    /// Marks the connection failed exactly once: unblocks in-flight requests and
    /// hands the error to whichever lifecycle waiter is pending (ready → throw,
    /// closed → return, neither → stash for the next `awaitClosed`).
    private func connectionDidFail(_ error: Error?) {
        isEstablished = false
        failPending(with: ControlError(code: "disconnected", message: "connection lost"))
        if let continuation = readyContinuation {
            readyContinuation = nil
            continuation.resume(throwing: error ?? ControlError(code: "disconnected", message: "connection cancelled"))
        } else if let continuation = closedContinuation {
            closedContinuation = nil
            continuation.resume(returning: error)
        } else if !closeAlreadyHappened {
            closeAlreadyHappened = true
            storedCloseError = error
        }
    }

    private func teardown(error: Error?) {
        isEstablished = false
        helloResult = nil
        heartbeatTask?.cancel()
        heartbeatTask = nil
        connection?.cancel()
        connection = nil
        failPending(with: ControlError(code: "disconnected", message: "connection lost"))
        emit(.disconnected(error))
    }

    /// Starts the idle heartbeat for this connection generation. A stalled
    /// heartbeat request (or a send error) cancels the connection, and the
    /// lifecycle loop's reconnect machinery takes over.
    private func startHeartbeat(generation: Int) {
        heartbeatTask?.cancel()
        heartbeatTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: Self.heartbeatInterval)
                guard let self, !Task.isCancelled else { return }
                if await self.heartbeatTick(generation: generation) { return }
            }
        }
    }

    /// One heartbeat round-trip. Returns true when the loop should stop.
    private func heartbeatTick(generation: Int) async -> Bool {
        guard generation == connectionGeneration, isEstablished else { return true }
        do {
            _ = try await request(
                Method.hello, params: HelloParams(build: build, token: token),
                timeout: .seconds(10))
            return false
        } catch {
            // Only act if we're still the current generation — a reconnect may
            // have already superseded us.
            guard generation == connectionGeneration else { return true }
            connection?.cancel()   // → .cancelled → connectionDidFail → reconnect
            return true
        }
    }

    // MARK: Routing

    private func route(_ message: ControlMessage) {
        switch message {
        case .response(let id, let result):
            resolve(id: id, result: result)
        case .event(let name, let seq, let params):
            routeEvent(name: name, seq: seq, params: params)
        case .request:
            break   // the daemon does not send requests to clients
        }
    }

    private func resolve(id: UInt64, result: Result<JSONValue, ControlError>) {
        guard let entry = pending.removeValue(forKey: id) else { return }
        entry.timeout?.cancel()
        switch result {
        case .success(let value): entry.continuation.resume(returning: value)
        case .failure(let error): entry.continuation.resume(throwing: error)
        }
    }

    private func routeEvent(name: String, seq: UInt64, params: JSONValue) {
        if seq > lastSeq { lastSeq = seq }   // monotonic
        for (_, continuation) in eventConsumers {
            continuation.yield((name: name, seq: seq, params: params))
        }
    }

    // MARK: Request bookkeeping

    private func allocateRequestID() -> UInt64 {
        nextRequestID += 1
        return nextRequestID
    }

    private func timeoutRequest(_ id: UInt64) {
        guard let entry = pending.removeValue(forKey: id) else { return }
        entry.continuation.resume(throwing: ControlError(code: "timeout", message: "request \(id) timed out"))
    }

    private func failPending(with error: ControlError) {
        guard !pending.isEmpty else { return }
        let entries = pending
        pending.removeAll()
        for (_, entry) in entries {
            entry.timeout?.cancel()
            entry.continuation.resume(throwing: error)
        }
    }

    // MARK: Events / state bookkeeping

    private func sendSubscribe() {
        let since: UInt64? = lastSeq == 0 ? nil : lastSeq
        Task { [weak self] in
            _ = try? await self?.request(Method.eventsSubscribe, params: EventsSubscribeParams(sinceSeq: since))
        }
    }

    private func removeEventConsumer(_ id: UUID) {
        eventConsumers[id] = nil
    }

    private func removeStateConsumer(_ id: UUID) {
        stateConsumers[id] = nil
    }

    private func emit(_ state: ClientConnectionState) {
        currentState = state
        for (_, continuation) in stateConsumers {
            continuation.yield(state)
        }
    }
}
