import Foundation
import Network
import DirijorCore
import DirijorProtocol

/// One binary data connection to a single attached session. Opens a *separate*
/// `NWConnection`, sends one NDJSON `AttachRequest` line, then switches to binary
/// `FrameCodec` framing for the terminal byte stream.
///
/// The connection is confined to this actor; its `Network.framework` callbacks
/// hop back on via `Task { await … }`.
public actor SessionAttachment {

    /// A decoded terminal event from the daemon's data channel.
    public enum TerminalChunk: Sendable {
        /// Start of a replayed history range at `offset`.
        case replayBegin(offset: UInt64)
        /// Live or replayed PTY output at `offset`.
        case output(offset: UInt64, bytes: Data)
        /// End of replay; `offset` is the log tail the live stream continues from.
        case replayEnd(offset: UInt64)
        /// A screen update from the daemon's authoritative grid emulator.
        case grid(GridUpdate)
        /// Program modes that gate local scrollback browsing: `altScreen` (on the
        /// alternate buffer, no scrollback) and `mouseReporting` (wheel events go
        /// to the program). Emitted on attach and whenever the value changes.
        case modes(altScreen: Bool, mouseReporting: Bool)
    }

    // MARK: Configuration

    private let endpoint: DaemonEndpoint
    private let sessionID: SessionID
    private let fromOffset: UInt64?
    private let token: String?
    private let role: ClientRole
    private let queue: DispatchQueue

    // MARK: Stream (immutable, read without hopping onto the actor)

    /// Decoded chunks. Buffering is `.unbounded`: the renderer consumes fast and
    /// the daemon applies backpressure. Finishes when the connection closes.
    public nonisolated let chunks: AsyncStream<TerminalChunk>
    private let chunkContinuation: AsyncStream<TerminalChunk>.Continuation

    // MARK: Connection

    private var connection: NWConnection?
    private var codec = FrameCodec()
    private var generation = 0
    private var closed = false
    private var readyContinuation: CheckedContinuation<Void, Error>?

    // MARK: Keepalive
    //
    // Mobile links (Tailscale over cellular/Wi-Fi) drop silently: the socket
    // looks alive but no bytes flow. We poke it — after ~20s with no frame we
    // send a `.ping` (the daemon answers `.pong`); if ~30s pass with no frame of
    // any kind we fail the connection, which finishes `chunks` and the caller
    // re-attaches (replay makes the result correct).
    private var lastReceivedAt = Date()
    private var keepaliveTask: Task<Void, Never>?
    private static let pingAfter: TimeInterval = 20
    private static let deadAfter: TimeInterval = 30

    // MARK: Init

    public init(
        endpoint: DaemonEndpoint,
        sessionID: SessionID,
        fromOffset: UInt64? = nil,
        token: String? = nil,
        role: ClientRole = .desktop
    ) {
        self.endpoint = endpoint
        self.sessionID = sessionID
        self.fromOffset = fromOffset
        self.token = token
        self.role = role
        self.queue = DispatchQueue(label: "dev.dirijor.client.attach.\(sessionID.rawValue)")

        var continuation: AsyncStream<TerminalChunk>.Continuation!
        self.chunks = AsyncStream(bufferingPolicy: .unbounded) { continuation = $0 }
        self.chunkContinuation = continuation
    }

    // MARK: Public API

    /// Connects, sends the attach handshake, and begins delivering chunks.
    /// On any connection failure the `chunks` stream simply finishes; the caller
    /// re-attaches and replay makes the result correct.
    public func connect() async throws {
        guard connection == nil, !closed else { return }
        generation += 1
        let generation = self.generation
        codec = FrameCodec()

        let connection = endpoint.makeConnection()
        self.connection = connection
        connection.stateUpdateHandler = { [weak self] state in
            Task { await self?.handleState(state, generation: generation) }
        }

        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            readyContinuation = continuation
            connection.start(queue: queue)
        }

        // Ready: send the single NDJSON attach line, then read binary frames.
        let request = AttachRequest(
            attach: sessionID, fromOffset: fromOffset, token: token, role: role)
        var line = try JSONEncoder.dirijor.encode(request)
        line.append(0x0A)
        connection.send(content: line, completion: .contentProcessed { [weak self] error in
            guard let error else { return }
            Task { await self?.fail(error) }
        })
        lastReceivedAt = Date()
        startReceiveLoop(generation: generation)
        startKeepalive(generation: generation)
    }

    /// Polls for a silent link: pings after `pingAfter`s of no frames, fails
    /// after `deadAfter`s. Tied to `generation` so a reconnect's loop supersedes.
    private func startKeepalive(generation: Int) {
        keepaliveTask?.cancel()
        keepaliveTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                guard let self else { return }
                if await self.keepaliveTick(generation: generation) { return }
            }
        }
    }

    /// One keepalive check. Returns true when the loop should stop (stale
    /// generation, closed, or we failed the connection).
    private func keepaliveTick(generation: Int) -> Bool {
        guard generation == self.generation, !closed, let connection else { return true }
        let idle = Date().timeIntervalSince(lastReceivedAt)
        if idle >= Self.deadAfter {
            fail(ControlError(code: "timeout", message: "no data-channel frames for \(Int(idle))s"))
            return true
        }
        if idle >= Self.pingAfter {
            connection.send(content: FrameCodec.encode(Frame(type: .ping)),
                completion: .contentProcessed { [weak self] error in
                    guard let error else { return }
                    Task { await self?.fail(error) }
                })
        }
        return false
    }

    /// Sends raw keystroke bytes to the PTY.
    public func sendInput(_ bytes: Data) {
        send(frame: .input(bytes))
    }

    /// Resizes the PTY; values are clamped to `UInt16`.
    public func resize(cols: Int, rows: Int) {
        send(frame: .resize(cols: UInt16(clamping: cols), rows: UInt16(clamping: rows)))
    }

    /// Forwards a mouse-wheel scroll; the daemon encodes it per the session's
    /// mouse mode. `dir` 0 = up, 1 = down; `col`/`row` are the pointer's cell.
    public func scroll(dir: UInt8, lines: Int, col: Int, row: Int) {
        send(frame: .scroll(
            dir: dir, lines: UInt16(clamping: lines),
            col: UInt16(clamping: col), row: UInt16(clamping: row)))
    }

    /// Detaches and finishes the stream. Idempotent.
    public func close() {
        guard !closed else { return }
        closed = true
        generation += 1
        keepaliveTask?.cancel()
        keepaliveTask = nil
        connection?.cancel()
        connection = nil
        chunkContinuation.finish()
    }

    // MARK: Connection handling

    private func handleState(_ state: NWConnection.State, generation: Int) {
        guard generation == self.generation else { return }
        switch state {
        case .ready:
            if let continuation = readyContinuation {
                readyContinuation = nil
                continuation.resume()
            }
        case .failed(let error):
            fail(error)
        case .cancelled:
            fail(nil)
        default:
            break
        }
    }

    private func startReceiveLoop(generation: Int) {
        guard generation == self.generation, let connection else { return }
        connection.receive(minimumIncompleteLength: 1, maximumLength: 1 << 16) { [weak self] data, _, isComplete, error in
            Task { await self?.handleReceived(data: data, isComplete: isComplete, error: error, generation: generation) }
        }
    }

    private func handleReceived(data: Data?, isComplete: Bool, error: NWError?, generation: Int) {
        guard generation == self.generation else { return }
        if let data, !data.isEmpty {
            lastReceivedAt = Date()   // any inbound frame proves the link is alive
            do {
                let frames = try codec.append(data)
                for frame in frames { process(frame) }
            } catch {
                fail(error)
                return
            }
        }
        if let error { fail(error); return }
        if isComplete { fail(nil); return }
        startReceiveLoop(generation: generation)
    }

    private func process(_ frame: Frame) {
        switch frame.type {
        case .output:
            if let payload = frame.outputPayload {
                chunkContinuation.yield(.output(offset: payload.offset, bytes: payload.bytes))
            }
        case .replayBegin:
            if let offset = frame.offsetPayload {
                chunkContinuation.yield(.replayBegin(offset: offset))
            }
        case .replayEnd:
            if let offset = frame.offsetPayload {
                chunkContinuation.yield(.replayEnd(offset: offset))
            }
        case .grid:
            if let update = frame.gridPayload {
                chunkContinuation.yield(.grid(update))
            }
        case .modes:
            if let m = frame.modesPayload {
                chunkContinuation.yield(.modes(altScreen: m.altScreen, mouseReporting: m.mouseReporting))
            }
        case .ping:
            send(frame: Frame(type: .pong))
        case .input, .resize, .scroll, .pong:
            break   // not expected from the daemon
        }
    }

    private func send(frame: Frame) {
        guard !closed, let connection else { return }
        let data = FrameCodec.encode(frame)
        connection.send(content: data, completion: .contentProcessed { [weak self] error in
            guard let error else { return }
            Task { await self?.fail(error) }
        })
    }

    /// Terminates the attachment on failure, finishing the stream so the caller
    /// can re-attach. Also resumes a still-pending `connect()`.
    private func fail(_ error: Error?) {
        if let continuation = readyContinuation {
            readyContinuation = nil
            continuation.resume(throwing: error ?? ControlError(code: "disconnected", message: "attach connection cancelled"))
        }
        guard !closed else { return }
        closed = true
        generation += 1
        keepaliveTask?.cancel()
        keepaliveTask = nil
        connection?.cancel()
        connection = nil
        chunkContinuation.finish()
    }
}
