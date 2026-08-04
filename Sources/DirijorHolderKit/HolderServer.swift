import Darwin
import Foundation

/// Owns exactly one PTY and child tree. The holder has no daemon dependencies:
/// its entire control interface is one request/response NDJSON line per UDS
/// connection, and its durable output interface is the OutputLog file.
public final class HolderServer: @unchecked Sendable {
    private let spec: HolderLaunchSpec
    private let queue: DispatchQueue
    private let stateLock = NSLock()

    private var masterFD: Int32 = -1
    private var childPID: pid_t = -1
    private var listenFD: Int32 = -1
    private var readSource: DispatchSourceRead?
    private var exitSource: DispatchSourceProcess?
    private var log: HolderOutputLog?
    /// Log tail at the moment this holder started: the boundary between prior
    /// incarnations' bytes (the log survives relaunches under the same session
    /// id) and bytes attributable to THIS child. Reported via stat.
    private var epochOffset: UInt64 = 0
    private var finished = false

    public init(spec: HolderLaunchSpec) {
        self.spec = spec
        self.queue = DispatchQueue(label: "dev.dirijor.holder.\(spec.sessionID)")
    }

    public func run() throws {
        // Never double-run: a second holder for the same session would
        // interleave two writers into one output log and stack a second child
        // (e.g. another ssh client onto the same remote tmux). If a live
        // holder already serves this socket, defer to it — bail before
        // touching the log or spawning anything.
        if HolderClient(socketPath: spec.socketPath).isAlive() {
            throw HolderError.launch(
                "a live holder already serves \(spec.socketPath); refusing to double-run")
        }

        try FileManager.default.createDirectory(
            at: URL(fileURLWithPath: spec.socketPath).deletingLastPathComponent(),
            withIntermediateDirectories: true)

        let outputLog = try HolderOutputLog(
            fileURL: URL(fileURLWithPath: spec.logFilePath),
            diskCapacity: spec.diskCapacity)
        // Captured before the child can produce a single byte: everything
        // below this offset predates this incarnation.
        epochOffset = outputLog.tailOffset
        let spawned = try HolderPTY.spawn(spec)
        let currentFlags = fcntl(spawned.master, F_GETFL)
        if currentFlags >= 0 { _ = fcntl(spawned.master, F_SETFL, currentFlags | O_NONBLOCK) }
        let listener = try UnixSocket.listen(path: spec.socketPath)

        stateLock.lock()
        log = outputLog
        masterFD = spawned.master
        childPID = spawned.child
        listenFD = listener
        stateLock.unlock()

        try writePIDFile()
        startPTYReader()
        startExitWatcher()

        while true {
            let client = Darwin.accept(listener, nil, nil)
            if client >= 0 {
                serve(client)
                Darwin.close(client)
                continue
            }
            if errno == EINTR { continue }
            stateLock.lock()
            let done = finished
            stateLock.unlock()
            if done { break }
            throw HolderError.transport(UnixSocket.posixMessage("accept"))
        }
    }

    private func startPTYReader() {
        let source = DispatchSource.makeReadSource(fileDescriptor: masterFD, queue: queue)
        readSource = source
        source.setEventHandler { [weak self] in self?.drainPTY() }
        source.activate()
    }

    private func startExitWatcher() {
        let pid = childPID
        let source = DispatchSource.makeProcessSource(
            identifier: pid, eventMask: .exit, queue: queue)
        exitSource = source
        source.setEventHandler { [weak self] in
            guard let self else { return }
            var status: Int32 = 0
            while waitpid(pid, &status, 0) < 0, errno == EINTR {}
            let exit: HolderExitStatus
            if status & 0x7F != 0 {
                exit = HolderExitStatus(reason: .signaled, signal: status & 0x7F)
            } else {
                exit = HolderExitStatus(reason: .exited, code: (status >> 8) & 0xFF)
            }
            self.finish(exit)
        }
        source.activate()
    }

    private func drainPTY() {
        var bytes = [UInt8](repeating: 0, count: 64 << 10)
        while true {
            let count = Darwin.read(masterFD, &bytes, bytes.count)
            if count > 0 {
                try? log?.append(Data(bytes.prefix(count)))
            } else if count < 0, errno == EINTR {
                continue
            } else {
                break
            }
        }
    }

    private func serve(_ client: Int32) {
        let response: HolderResponse
        do {
            let line = try UnixSocket.readLine(fd: client)
            let request = try JSONDecoder().decode(HolderRequest.self, from: line)
            response = try handle(request)
        } catch {
            response = .failure(String(describing: error))
        }
        if var encoded = try? JSONEncoder().encode(response) {
            encoded.append(0x0A)
            try? UnixSocket.writeAll(fd: client, data: encoded)
        }
    }

    private func handle(_ request: HolderRequest) throws -> HolderResponse {
        switch request.op {
        case .write:
            guard let encoded = request.data, let data = Data(base64Encoded: encoded) else {
                throw HolderError.invalidRequest("write requires base64 data")
            }
            try writePTY(data)
            return .success()

        case .resize:
            guard let cols = request.cols, let rows = request.rows, cols >= 2, rows >= 2 else {
                throw HolderError.invalidRequest("resize requires cols/rows >= 2")
            }
            HolderPTY.resize(master: masterFD, cols: cols, rows: rows)
            return .success()

        case .signal:
            guard let signal = request.sig, signal > 0, signal < NSIG else {
                throw HolderError.invalidRequest("signal requires a valid sig")
            }
            return .success(tree: HolderProcessTree.signal(root: childPID, signal: signal))

        case .killTree:
            HolderProcessTree.killTree(root: childPID)
            return .success()

        case .stat:
            return .success(stat: currentStat())
        }
    }

    /// The PTY master is nonblocking once DispatchSource owns it. Large pastes
    /// therefore retry EINTR/EAGAIN and wait for the child to drain, with the
    /// same bounded semantics AgentSession used before holders owned the fd.
    private func writePTY(_ data: Data) throws {
        try data.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else { return }
            var written = 0
            while written < raw.count {
                let count = Darwin.write(masterFD, base + written, raw.count - written)
                if count > 0 {
                    written += count
                } else if count < 0, errno == EINTR {
                    continue
                } else if count < 0, errno == EAGAIN || errno == EWOULDBLOCK {
                    var pollDescriptor = pollfd(
                        fd: masterFD, events: Int16(POLLOUT), revents: 0)
                    let ready = poll(&pollDescriptor, 1, 1000)
                    guard ready > 0, pollDescriptor.revents & Int16(POLLOUT) != 0 else {
                        throw HolderError.transport("PTY remained unwritable for 1 second")
                    }
                } else {
                    throw HolderError.transport(UnixSocket.posixMessage("PTY write"))
                }
            }
        }
    }

    private func currentStat() -> HolderStat {
        stateLock.lock()
        let done = finished
        stateLock.unlock()
        let foreground = tcgetpgrp(masterFD)
        let dimensions = HolderPTY.size(master: masterFD)
        return HolderStat(
            childPID: childPID,
            alive: !done && Darwin.kill(childPID, 0) == 0,
            logOffset: log?.tailOffset ?? 0,
            foregroundPID: foreground > 0 ? foreground : nil,
            cols: dimensions?.cols,
            rows: dimensions?.rows,
            epochOffset: epochOffset)
    }

    private func finish(_ status: HolderExitStatus) {
        stateLock.lock()
        guard !finished else {
            stateLock.unlock()
            return
        }
        finished = true
        stateLock.unlock()

        drainPTY()
        try? log?.append(HolderExitMarker.encode(status))
        log?.flush()
        readSource?.cancel()
        readSource = nil
        exitSource?.cancel()
        exitSource = nil
        if masterFD >= 0 {
            Darwin.close(masterFD)
            masterFD = -1
        }
        cleanupFiles()
        if listenFD >= 0 {
            Darwin.shutdown(listenFD, SHUT_RDWR)
            Darwin.close(listenFD)
            listenFD = -1
        }
    }

    private func writePIDFile() throws {
        let data = Data("\(getpid())\n".utf8)
        try data.write(to: URL(fileURLWithPath: spec.pidFilePath), options: .atomic)
        _ = chmod(spec.pidFilePath, 0o600)
    }

    private func cleanupFiles() {
        try? FileManager.default.removeItem(atPath: spec.socketPath)
        try? FileManager.default.removeItem(atPath: spec.pidFilePath)
    }
}
