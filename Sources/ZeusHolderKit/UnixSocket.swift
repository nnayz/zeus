import Darwin
import Foundation

enum UnixSocket {
    static func connect(path: String) throws -> Int32 {
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw HolderError.transport(posixMessage("socket")) }
        do {
            var address = try makeAddress(path: path)
            let result = withUnsafePointer(to: &address) {
                $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            guard result == 0 else { throw HolderError.transport(posixMessage("connect")) }
            return fd
        } catch {
            Darwin.close(fd)
            throw error
        }
    }

    static func listen(path: String) throws -> Int32 {
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw HolderError.transport(posixMessage("socket")) }
        do {
            _ = unlink(path)
            var address = try makeAddress(path: path)
            let result = withUnsafePointer(to: &address) {
                $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            guard result == 0 else { throw HolderError.transport(posixMessage("bind")) }
            _ = chmod(path, 0o600)
            guard Darwin.listen(fd, 16) == 0 else {
                throw HolderError.transport(posixMessage("listen"))
            }
            return fd
        } catch {
            Darwin.close(fd)
            throw error
        }
    }

    static func writeAll(fd: Int32, data: Data) throws {
        try data.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else { return }
            var sent = 0
            while sent < raw.count {
                let count = Darwin.write(fd, base + sent, raw.count - sent)
                if count > 0 {
                    sent += count
                } else if count < 0, errno == EINTR {
                    continue
                } else {
                    throw HolderError.transport(posixMessage("write"))
                }
            }
        }
    }

    static func readLine(fd: Int32, limit: Int = 16 << 20) throws -> Data {
        var result = Data()
        var chunk = [UInt8](repeating: 0, count: 4096)
        while result.count < limit {
            let count = Darwin.read(fd, &chunk, chunk.count)
            if count > 0 {
                if let newline = chunk[..<count].firstIndex(of: 0x0A) {
                    result.append(contentsOf: chunk[..<newline])
                    return result
                }
                result.append(contentsOf: chunk[..<count])
            } else if count == 0 {
                return result
            } else if errno == EINTR {
                continue
            } else {
                throw HolderError.transport(posixMessage("read"))
            }
        }
        throw HolderError.transport("NDJSON line exceeds \(limit) bytes")
    }

    static func posixMessage(_ operation: String) -> String {
        "\(operation): \(String(cString: strerror(errno)))"
    }

    private static func makeAddress(path: String) throws -> sockaddr_un {
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8CString)
        let capacity = MemoryLayout.size(ofValue: address.sun_path)
        guard bytes.count <= capacity else {
            throw HolderError.transport("unix socket path is too long: \(path)")
        }
        withUnsafeMutableBytes(of: &address.sun_path) { destination in
            destination.initializeMemory(as: UInt8.self, repeating: 0)
            destination.copyBytes(from: bytes.map(UInt8.init(bitPattern:)))
        }
        return address
    }
}
