import DirijorCore
import DirijorProtocol
import Foundation

#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#elseif canImport(Musl)
import Musl
#endif

enum DaemonError: Error, CustomStringConvertible {
    case io(String)
    case timeout
    case control(ControlError)

    var description: String {
        switch self {
        case .io(let m): m
        case .timeout: "timed out"
        case .control(let e): "\(e.code): \(e.message)"
        }
    }
}

/// A minimal synchronous unix-socket NDJSON client for the short-lived CLI.
/// Uses raw POSIX sockets with poll()-based timeouts — no Network.framework, so
/// it stays simple and never hangs a hook or a `doctor` invocation.
final class DaemonConn {
    private let fd: Int32

    private init(fd: Int32) { self.fd = fd }

    /// The control socket path from `$DIRIJOR_SOCKET`, else the default location.
    static func socketPath() -> String {
        if let env = ProcessInfo.processInfo.environment[DirijorEnv.socket], !env.isEmpty {
            return env
        }
        return DirijorPaths.socket.path
    }

    /// Opens and connects a socket to the daemon. `connectTimeout` bounds the
    /// non-blocking connect handshake.
    static func connect(path: String = socketPath(), connectTimeout: TimeInterval = 3) throws -> DaemonConn {
        try DaemonConn(fd: openUDS(path: path, timeout: connectTimeout))
    }

    static func connectTCP(host: String, port: UInt16, connectTimeout: TimeInterval = 3) throws
        -> DaemonConn
    {
        try DaemonConn(fd: openTCP(host: host, port: port, timeout: connectTimeout))
    }

    static func openUDS(path: String = socketPath(), timeout: TimeInterval = 3) throws -> Int32 {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        if fd < 0 { throw DaemonError.io("socket() failed") }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let capacity = MemoryLayout.size(ofValue: addr.sun_path)
        let pathBytes = Array(path.utf8)
        if pathBytes.count >= capacity {
            posixClose(fd)
            throw DaemonError.io("socket path too long")
        }
        withUnsafeMutablePointer(to: &addr.sun_path) { rawPtr in
            rawPtr.withMemoryRebound(to: CChar.self, capacity: capacity) { dst in
                for (i, b) in pathBytes.enumerated() { dst[i] = CChar(bitPattern: b) }
                dst[pathBytes.count] = 0
            }
        }

        // Non-blocking connect so we can bound it with poll().
        let flags = fcntl(fd, F_GETFL, 0)
        _ = fcntl(fd, F_SETFL, flags | O_NONBLOCK)

        let addrLen = socklen_t(MemoryLayout<sockaddr_un>.stride)
        let result = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                posixConnect(fd, $0, addrLen)
            }
        }

        if result != 0 {
            if errno == EINPROGRESS {
                var pfd = pollfd(fd: fd, events: Int16(POLLOUT), revents: 0)
                let pr = poll(&pfd, 1, Int32(timeout * 1000))
                if pr <= 0 {
                    posixClose(fd)
                    throw DaemonError.io("connect timed out")
                }
                var soError: Int32 = 0
                var len = socklen_t(MemoryLayout<Int32>.size)
                getsockopt(fd, SOL_SOCKET, SO_ERROR, &soError, &len)
                if soError != 0 {
                    posixClose(fd)
                    throw DaemonError.io("connect failed (\(soError))")
                }
            } else {
                let err = String(cString: strerror(errno))
                posixClose(fd)
                throw DaemonError.io("connect failed: \(err)")
            }
        }
        _ = fcntl(fd, F_SETFL, flags)
        return fd
    }

    static func openTCP(host: String, port: UInt16, timeout: TimeInterval = 3) throws -> Int32 {
        var hints = addrinfo()
        hints.ai_family = AF_UNSPEC
        hints.ai_socktype = SOCK_STREAM
        hints.ai_protocol = IPPROTO_TCP

        var result: UnsafeMutablePointer<addrinfo>?
        let service = String(port)
        let rc = getaddrinfo(host, service, &hints, &result)
        guard rc == 0, let first = result else {
            throw DaemonError.io("getaddrinfo failed: \(String(cString: gai_strerror(rc)))")
        }
        defer { freeaddrinfo(first) }

        var lastError = "connect failed"
        var cursor: UnsafeMutablePointer<addrinfo>? = first
        while let ai = cursor {
            let fd = socket(ai.pointee.ai_family, ai.pointee.ai_socktype, ai.pointee.ai_protocol)
            if fd < 0 {
                cursor = ai.pointee.ai_next
                continue
            }
            do {
                try connectFD(fd, addr: ai.pointee.ai_addr, addrLen: ai.pointee.ai_addrlen, timeout: timeout)
                return fd
            } catch {
                lastError = "\(error)"
                posixClose(fd)
                cursor = ai.pointee.ai_next
            }
        }
        throw DaemonError.io(lastError)
    }

    func close() {
        posixClose(fd)
    }

    /// Sends a request (id 1) and reads control lines until the matching response
    /// arrives, skipping any interleaved events. `readTimeout` bounds the wait —
    /// pass a large value for long-poll methods like `events.wait`.
    func request<P: Encodable>(
        _ method: String,
        params: P,
        readTimeout: TimeInterval = 3,
        writeTimeout: TimeInterval = 3
    ) throws -> JSONValue {
        let message = ControlMessage.request(
            id: 1,
            method: method,
            params: try JSONValue(encoding: params)
        )
        let outbound = try NDJSONBuffer.encode(message)
        try writeAll(outbound, timeoutMs: Int32(writeTimeout * 1000))

        var ndjson = NDJSONBuffer()
        let deadline = Date().addingTimeInterval(readTimeout)
        var readBuffer = [UInt8](repeating: 0, count: 65536)

        while true {
            let remainingMs = Int32(max(0, deadline.timeIntervalSinceNow * 1000))
            if remainingMs == 0 { throw DaemonError.timeout }
            var pfd = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
            let pr = poll(&pfd, 1, remainingMs)
            if pr == 0 { throw DaemonError.timeout }
            if pr < 0 {
                if errno == EINTR { continue }
                throw DaemonError.io("poll failed")
            }
            let n = read(fd, &readBuffer, readBuffer.count)
            if n == 0 { throw DaemonError.io("connection closed by daemon") }
            if n < 0 {
                if errno == EINTR || errno == EAGAIN { continue }
                throw DaemonError.io("read failed")
            }
            let messages = try ndjson.append(Data(readBuffer[0..<n]))
            for m in messages {
                if case .response(let id, let result) = m, id == 1 {
                    switch result {
                    case .success(let value): return value
                    case .failure(let error): throw DaemonError.control(error)
                    }
                }
                // Skip events and any non-matching responses.
            }
        }
    }

    /// Sends a request and then stays on the connection, handing every event the
    /// daemon pushes to `onEvent` until it returns false, the peer closes, or
    /// `deadline` passes.
    ///
    /// Separate from `request` because a subscription has no terminal response:
    /// `request` returns at the first matching id and drops everything after it,
    /// which is exactly the wrong shape for `events subscribe`. A nil `deadline`
    /// blocks indefinitely — a subscriber is *supposed* to sit idle when nothing
    /// is happening, so there is no idle timeout to trip over.
    func stream<P: Encodable>(
        _ method: String,
        params: P,
        deadline: Date? = nil,
        writeTimeout: TimeInterval = 3,
        onEvent: (String, UInt64, JSONValue) throws -> Bool
    ) throws {
        let message = ControlMessage.request(
            id: 1, method: method, params: try JSONValue(encoding: params))
        try writeAll(try NDJSONBuffer.encode(message), timeoutMs: Int32(writeTimeout * 1000))

        var ndjson = NDJSONBuffer()
        var readBuffer = [UInt8](repeating: 0, count: 65536)

        while true {
            let pollMs = deadline.map { Int32(max(0, $0.timeIntervalSinceNow * 1000)) } ?? -1
            var pfd = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
            let pr = poll(&pfd, 1, pollMs)
            if pr == 0 { throw DaemonError.timeout }
            if pr < 0 {
                if errno == EINTR { continue }
                throw DaemonError.io("poll failed")
            }
            let n = read(fd, &readBuffer, readBuffer.count)
            if n == 0 { return }  // clean EOF: the daemon went away
            if n < 0 {
                if errno == EINTR || errno == EAGAIN { continue }
                throw DaemonError.io("read failed")
            }
            for m in try ndjson.append(Data(readBuffer[0..<n])) {
                switch m {
                case .event(let name, let seq, let params):
                    if try !onEvent(name, seq, params) { return }
                case .response(_, .failure(let error)):
                    throw DaemonError.control(error)
                case .response, .request:
                    continue  // the subscribe ack; nothing to report
                }
            }
        }
    }

    private func writeAll(_ data: Data, timeoutMs: Int32) throws {
        try data.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
            guard let base = raw.bindMemory(to: UInt8.self).baseAddress else { return }
            var offset = 0
            while offset < data.count {
                var pfd = pollfd(fd: fd, events: Int16(POLLOUT), revents: 0)
                let pr = poll(&pfd, 1, timeoutMs)
                if pr <= 0 { throw DaemonError.timeout }
                let n = write(fd, base + offset, data.count - offset)
                if n < 0 {
                    if errno == EINTR || errno == EAGAIN { continue }
                    throw DaemonError.io("write failed")
                }
                offset += n
            }
        }
    }

    private static func connectFD(
        _ fd: Int32,
        addr: UnsafePointer<sockaddr>,
        addrLen: socklen_t,
        timeout: TimeInterval
    ) throws {
        let flags = fcntl(fd, F_GETFL, 0)
        _ = fcntl(fd, F_SETFL, flags | O_NONBLOCK)

        let result = posixConnect(fd, addr, addrLen)
        if result != 0 {
            if errno == EINPROGRESS {
                var pfd = pollfd(fd: fd, events: Int16(POLLOUT), revents: 0)
                let pr = poll(&pfd, 1, Int32(timeout * 1000))
                if pr <= 0 {
                    throw DaemonError.io("connect timed out")
                }
                var soError: Int32 = 0
                var len = socklen_t(MemoryLayout<Int32>.size)
                getsockopt(fd, SOL_SOCKET, SO_ERROR, &soError, &len)
                if soError != 0 {
                    throw DaemonError.io("connect failed (\(soError))")
                }
            } else {
                throw DaemonError.io("connect failed: \(String(cString: strerror(errno)))")
            }
        }
        _ = fcntl(fd, F_SETFL, flags)
    }
}
