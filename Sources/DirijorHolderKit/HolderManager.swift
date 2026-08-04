import Darwin
import Foundation

/// Control client for the one lightweight holder manager in a registry.
///
/// Session traffic never flows through this socket. It is used only to ask the
/// manager to create a session-local `HolderServer`; all existing per-session
/// sockets, logs, and restart adoption semantics remain unchanged.
public struct HolderManagerClient: Sendable {
    public let socketPath: String

    public init(socketPath: String) {
        self.socketPath = socketPath
    }

    public func ping() throws -> pid_t {
        try request(
            HolderManagerRequest(
                version: HolderManagerPaths.protocolVersion,
                op: .ping,
                spec: nil))
    }

    @discardableResult
    public func launch(_ spec: HolderLaunchSpec) throws -> pid_t {
        try request(
            HolderManagerRequest(
                version: HolderManagerPaths.protocolVersion,
                op: .launch,
                spec: spec))
    }

    public func isAlive() -> Bool {
        (try? ping()) != nil
    }

    private func request(_ request: HolderManagerRequest) throws -> pid_t {
        let fd = try UnixSocket.connect(path: socketPath)
        defer { Darwin.close(fd) }
        var encoded = try JSONEncoder().encode(request)
        encoded.append(0x0A)
        try UnixSocket.writeAll(fd: fd, data: encoded)
        let line = try UnixSocket.readLine(fd: fd)
        let response = try JSONDecoder().decode(HolderManagerResponse.self, from: line)
        guard response.ok else {
            throw HolderError.rejected(response.error ?? "unknown manager error")
        }
        guard let managerPID = response.managerPID, managerPID > 1 else {
            throw HolderError.transport("manager response omitted pid")
        }
        return managerPID
    }
}

/// Owns many independent `HolderServer`s in one detached process.
///
/// Mutable lifecycle state is protected by `stateLock`; session PTY state
/// remains encapsulated by each `HolderServer` and its serial dispatch queue.
/// The manager exits only after every managed child has exited and the idle
/// grace period has elapsed, so daemon crashes and upgrades never take a PTY
/// with them.
public final class HolderManagerServer: @unchecked Sendable {
    private let paths: HolderManagerPaths
    private let idleTimeout: TimeInterval
    private let stateLock = NSLock()
    private let workerQueue = DispatchQueue(
        label: "dev.dirijor.holder-manager.sessions",
        qos: .userInitiated,
        attributes: .concurrent)
    private let lifecycleQueue = DispatchQueue(label: "dev.dirijor.holder-manager.lifecycle")

    private var activeSessionIDs: Set<String> = []
    private var listenFD: Int32 = -1
    private var shuttingDown = false
    private var idleTimer: DispatchSourceTimer?

    public init(directory: URL, idleTimeout: TimeInterval = 30) {
        self.paths = HolderManagerPaths(directory: directory)
        self.idleTimeout = max(0.1, idleTimeout)
    }

    public func run() throws {
        try FileManager.default.createDirectory(
            at: paths.directory, withIntermediateDirectories: true)
        let listener = try UnixSocket.listen(path: paths.socket.path)

        stateLock.lock()
        listenFD = listener
        stateLock.unlock()

        try writePIDFile()
        armIdleTimerIfEmpty()

        defer {
            lifecycleQueue.sync {
                idleTimer?.cancel()
                idleTimer = nil
            }
            stateLock.lock()
            if listenFD == listener {
                Darwin.close(listener)
                listenFD = -1
            }
            stateLock.unlock()
            cleanupControlFiles()
        }

        while true {
            let client = Darwin.accept(listener, nil, nil)
            if client >= 0 {
                serve(client)
                Darwin.close(client)
                continue
            }
            if errno == EINTR { continue }
            stateLock.lock()
            let done = shuttingDown
            stateLock.unlock()
            if done || errno == EBADF || errno == EINVAL { break }
            throw HolderError.transport(UnixSocket.posixMessage("manager accept"))
        }
    }

    private func serve(_ client: Int32) {
        let response: HolderManagerResponse
        do {
            let line = try UnixSocket.readLine(fd: client)
            let request = try JSONDecoder().decode(HolderManagerRequest.self, from: line)
            response = try handle(request)
        } catch {
            response = .failure(String(describing: error))
        }
        if var encoded = try? JSONEncoder().encode(response) {
            encoded.append(0x0A)
            try? UnixSocket.writeAll(fd: client, data: encoded)
        }
    }

    private func handle(_ request: HolderManagerRequest) throws -> HolderManagerResponse {
        guard request.version == HolderManagerPaths.protocolVersion else {
            throw HolderError.invalidRequest(
                "manager protocol \(request.version) is unsupported")
        }
        switch request.op {
        case .ping:
            armIdleTimerIfEmpty()
            return .success(managerPID: getpid())

        case .launch:
            guard let spec = request.spec else {
                throw HolderError.invalidRequest("manager launch requires a spec")
            }
            try validate(spec)

            // A session from an older per-session holder may already own this
            // socket. Adopt it instead of creating a second writer/child.
            if HolderClient(socketPath: spec.socketPath).isAlive() {
                return .success(managerPID: getpid())
            }

            stateLock.lock()
            guard !shuttingDown else {
                stateLock.unlock()
                throw HolderError.rejected("manager is shutting down")
            }
            let inserted = activeSessionIDs.insert(spec.sessionID).inserted
            stateLock.unlock()
            guard inserted else {
                return .success(managerPID: getpid())
            }
            cancelIdleTimer()

            workerQueue.async { [weak self] in
                defer { self?.sessionFinished(spec.sessionID) }
                do {
                    try HolderServer(spec: spec).run()
                } catch {
                    let message = Data(
                        "dirijord-holder manager: session \(spec.sessionID): \(error)\n".utf8)
                    FileHandle.standardError.write(message)
                }
            }
            return .success(managerPID: getpid())
        }
    }

    private func validate(_ spec: HolderLaunchSpec) throws {
        guard !spec.sessionID.isEmpty else {
            throw HolderError.invalidRequest("session id is empty")
        }
        let expected = HolderPaths(directory: paths.directory, sessionID: spec.sessionID)
        guard spec.socketPath == expected.socket.path, spec.pidFilePath == expected.pidFile.path else {
            throw HolderError.invalidRequest("session control paths are outside manager directory")
        }
    }

    private func sessionFinished(_ sessionID: String) {
        stateLock.lock()
        activeSessionIDs.remove(sessionID)
        let empty = activeSessionIDs.isEmpty
        stateLock.unlock()
        if empty { armIdleTimerIfEmpty() }
    }

    /// A one-shot timer exists only while no session is hosted. Active
    /// managers therefore have no polling wakeup at all.
    private func armIdleTimerIfEmpty() {
        lifecycleQueue.async { [weak self] in
            guard let self else { return }
            stateLock.lock()
            let shouldArm = activeSessionIDs.isEmpty && !shuttingDown
            stateLock.unlock()
            guard shouldArm else { return }
            idleTimer?.cancel()
            let timer = DispatchSource.makeTimerSource(queue: lifecycleQueue)
            idleTimer = timer
            timer.schedule(deadline: .now() + idleTimeout)
            timer.setEventHandler { [weak self] in self?.stopIfIdle() }
            timer.activate()
        }
    }

    private func cancelIdleTimer() {
        lifecycleQueue.async { [weak self] in
            self?.idleTimer?.cancel()
            self?.idleTimer = nil
        }
    }

    private func stopIfIdle() {
        stateLock.lock()
        guard !shuttingDown, activeSessionIDs.isEmpty else {
            stateLock.unlock()
            return
        }
        shuttingDown = true
        let listener = listenFD
        listenFD = -1
        stateLock.unlock()

        if listener >= 0 {
            Darwin.shutdown(listener, SHUT_RDWR)
            Darwin.close(listener)
        }
    }

    private func writePIDFile() throws {
        let data = Data("\(getpid())\n".utf8)
        try data.write(to: paths.pidFile, options: .atomic)
        _ = chmod(paths.pidFile.path, 0o600)
    }

    /// Remove only this incarnation's endpoint. The same launch lock used by
    /// `HolderLauncher` closes the idle-exit/new-manager race; the PID check
    /// prevents an old process from unlinking a successor's fresh socket.
    private func cleanupControlFiles() {
        let lockFD = open(paths.launchLock.path, O_CREAT | O_RDWR, 0o600)
        guard lockFD >= 0 else { return }
        defer {
            _ = flock(lockFD, LOCK_UN)
            Darwin.close(lockFD)
        }
        guard flock(lockFD, LOCK_EX) == 0 else { return }
        guard
            let text = try? String(contentsOf: paths.pidFile, encoding: .utf8),
            Int32(text.trimmingCharacters(in: .whitespacesAndNewlines)) == getpid()
        else { return }
        try? FileManager.default.removeItem(at: paths.socket)
        try? FileManager.default.removeItem(at: paths.pidFile)
    }
}
