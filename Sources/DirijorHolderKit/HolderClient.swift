import Darwin
import Foundation

public struct HolderClient: Sendable {
    public let socketPath: String

    /// Coders are stateless after configuration; building fresh ones per
    /// request put two allocations on every keystroke's write path.
    private static let encoder = JSONEncoder()
    private static let decoder = JSONDecoder()

    public init(socketPath: String) {
        self.socketPath = socketPath
    }

    public func write(_ data: Data) throws {
        _ = try request(HolderRequest(op: .write, data: data.base64EncodedString()))
    }

    public func resize(cols: UInt16, rows: UInt16) throws {
        _ = try request(HolderRequest(op: .resize, cols: cols, rows: rows))
    }

    @discardableResult
    public func signal(_ sig: Int32) throws -> [HolderProcessSample] {
        try request(HolderRequest(op: .signal, sig: sig)).tree ?? []
    }

    public func killTree() throws {
        _ = try request(HolderRequest(op: .killTree))
    }

    public func stat() throws -> HolderStat {
        let response = try request(HolderRequest(op: .stat))
        guard let stat = response.stat else {
            throw HolderError.transport("stat response omitted stat")
        }
        return stat
    }

    public func isAlive() -> Bool {
        (try? stat().alive) == true
    }

    private func request(_ request: HolderRequest) throws -> HolderResponse {
        let fd = try UnixSocket.connect(path: socketPath)
        defer { Darwin.close(fd) }
        var encoded = try Self.encoder.encode(request)
        encoded.append(0x0A)
        try UnixSocket.writeAll(fd: fd, data: encoded)
        let line = try UnixSocket.readLine(fd: fd)
        let response = try Self.decoder.decode(HolderResponse.self, from: line)
        guard response.ok else {
            throw HolderError.rejected(response.error ?? "unknown error")
        }
        return response
    }
}
