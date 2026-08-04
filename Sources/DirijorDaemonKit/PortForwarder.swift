import DirijorCore
import DirijorProtocol
import Foundation
import Network

enum PortForwarderError: Error {
    case connectFailed
}

/// Handles one forwarded TCP connection. After the JSON handshake, bytes are
/// raw and unframed in both directions; either side closing tears down both
/// sockets. Half-close/FIN forwarding is intentionally not implemented because
/// the intended targets are HTTP dev servers.
actor PortForwarder {
    private let port: UInt16
    private let client: NWTransport
    private var target: NWTransport?
    private var targetToClientTask: Task<Void, Never>?

    init(port: UInt16, client: NWTransport) {
        self.port = port
        self.client = client
    }

    func dial() async throws {
        guard let nwPort = NWEndpoint.Port(rawValue: port) else {
            throw PortForwarderError.connectFailed
        }
        let params = NWParameters.tcp
        if let tcp = params.defaultProtocolStack.internetProtocol as? NWProtocolTCP.Options {
            tcp.noDelay = true
        }
        let connection = NWConnection(
            host: NWEndpoint.Host("localhost"), port: nwPort, using: params)
        let waiter = DialWaiter()
        connection.stateUpdateHandler = { state in
            switch state {
            case .ready:
                waiter.succeed()
            case .failed, .cancelled:
                waiter.fail(PortForwarderError.connectFailed)
            default:
                break
            }
        }
        connection.start(queue: DispatchQueue(label: "dirijor.forward.target.\(port)"))

        do {
            try await waiter.wait(timeout: .seconds(3))
        } catch {
            connection.cancel()
            throw error
        }
        target = NWTransport(connection: connection)
    }

    func startPumping() {
        guard targetToClientTask == nil else { return }
        targetToClientTask = Task { [weak self] in
            await self?.pumpTargetToClient()
        }
    }

    func write(_ data: Data) async throws {
        guard let target else { throw PortForwarderError.connectFailed }
        try await target.sendAwaiting(data)
    }

    func cancel() {
        targetToClientTask?.cancel()
        targetToClientTask = nil
        target?.cancel()
        target = nil
    }

    static func isPortForwardable(_ port: UInt16, registry: SessionRegistry) async -> Bool {
        func contains(_ list: SessionListResult) -> Bool {
            list.sessions.contains { record in
                (record.listeningPorts ?? []).contains { $0.port == Int(port) }
            }
        }

        if contains(await registry.list()) {
            return true
        }

        for (_, session) in await registry.liveSessionsSnapshot() {
            guard await session.isRunning else { continue }
            let rootPid = await session.pid
            guard rootPid > 0 else { continue }
            let pids = ProcessTree.enumerate(root: rootPid).map(\.pid)
            let ports = ResourceGovernor.listeningPorts(of: pids) ?? []
            if ports.contains(where: { $0.port == Int(port) }) {
                return true
            }
        }
        return contains(await registry.list())
    }

    private func pumpTargetToClient() async {
        defer { client.cancel() }
        guard let target else { return }
        do {
            while let chunk = try await target.receive() {
                while client.pendingBytes > (1 << 20), !Task.isCancelled {
                    try? await Task.sleep(for: .milliseconds(10))
                }
                guard !Task.isCancelled else { break }
                client.send(chunk)
            }
        } catch {
            DaemonLog.shared.log("port forward target read error: \(error)")
        }
    }
}

private final class DialWaiter: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<Void, Error>?
    private var result: Result<Void, Error>?

    func wait(timeout: Duration) async throws {
        try await withThrowingTaskGroup(of: Void.self) { group in
            group.addTask { try await self.waitForState() }
            group.addTask {
                try await Task.sleep(for: timeout)
                throw PortForwarderError.connectFailed
            }
            do {
                _ = try await group.next()
                group.cancelAll()
            } catch {
                group.cancelAll()
                self.fail(error)
                throw error
            }
        }
    }

    func succeed() {
        finish(.success(()))
    }

    func fail(_ error: Error) {
        finish(.failure(error))
    }

    private func waitForState() async throws {
        try await withCheckedThrowingContinuation { continuation in
            lock.lock()
            if let result {
                lock.unlock()
                continuation.resume(with: result)
            } else {
                self.continuation = continuation
                lock.unlock()
            }
        }
    }

    private func finish(_ result: Result<Void, Error>) {
        lock.lock()
        guard self.result == nil else {
            lock.unlock()
            return
        }
        self.result = result
        let continuation = self.continuation
        self.continuation = nil
        lock.unlock()
        continuation?.resume(with: result)
    }
}
