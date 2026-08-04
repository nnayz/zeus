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

// Cross-platform POSIX wrappers. Several CLI types define their own members
// (`DaemonConn.connect`, `PumpCloser.close`, `Forward.socket`) whose names shadow
// the global C functions, so an unqualified call at those sites would resolve to
// the member. Inside these free functions there is no such shadowing, so the
// unqualified calls bind to the imported platform C library (Darwin/Glibc/Musl).

/// Global POSIX `close(2)`.
@inline(__always)
func posixClose(_ fd: Int32) {
    _ = close(fd)
}

/// Global POSIX `socket(2)`.
@inline(__always)
func posixSocket(_ domain: Int32, _ type: Int32, _ proto: Int32) -> Int32 {
    socket(domain, type, proto)
}

/// Global POSIX `connect(2)`.
@inline(__always)
func posixConnect(_ fd: Int32, _ addr: UnsafePointer<sockaddr>, _ len: socklen_t) -> Int32 {
    connect(fd, addr, len)
}

/// Global POSIX `bind(2)`.
@inline(__always)
func posixBind(_ fd: Int32, _ addr: UnsafePointer<sockaddr>, _ len: socklen_t) -> Int32 {
    bind(fd, addr, len)
}

enum CLISupport {
    /// Reads up to `cap` bytes from stdin, stopping on EOF or after `timeoutMs`
    /// with no more data. Never blocks longer than the timeout between reads.
    static func readStdin(cap: Int = 1 << 20, timeoutMs: Int32 = 500) -> Data {
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 65536)
        while data.count < cap {
            var pfd = pollfd(fd: 0, events: Int16(POLLIN), revents: 0)
            let pr = poll(&pfd, 1, timeoutMs)
            if pr <= 0 { break }  // timeout or error
            let want = min(buffer.count, cap - data.count)
            let n = read(0, &buffer, want)
            if n <= 0 { break }
            data.append(contentsOf: buffer[0..<n])
        }
        return data
    }

    /// Parses raw bytes into a JSONValue, falling back to `{"raw": <string>}`
    /// when the input isn't valid JSON.
    static func parsePayload(_ data: Data) -> JSONValue {
        if !data.isEmpty,
            let value = try? JSONDecoder.dirijor.decode(JSONValue.self, from: data)
        {
            return value
        }
        return .object(["raw": .string(String(decoding: data, as: UTF8.self))])
    }

    /// Compact single-line JSON for a JSONValue.
    static func encodeCompact(_ value: JSONValue) -> String {
        guard let data = try? JSONEncoder.dirijor.encode(value),
            let string = String(data: data, encoding: .utf8)
        else { return "{}" }
        return string
    }

    static func sessionID() -> SessionID? {
        guard let raw = ProcessInfo.processInfo.environment[DirijorEnv.sessionID], !raw.isEmpty
        else { return nil }
        return SessionID(rawValue: raw)
    }

    /// Resolves an executable on PATH, like `which`. Returns its path or nil.
    static func which(_ name: String) -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["which", name]
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            return nil
        }
        let out = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else { return nil }
        let path = String(decoding: out, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        return path.isEmpty ? nil : path
    }
}

extension AgentKind {
    /// Short lowercase label used in compact MCP results. A generic command
    /// names itself; everything else takes the label from its manifest.
    var shortLabel: String {
        if let command { return (command as NSString).lastPathComponent }
        return descriptor.shortLabel
    }

    var glyph: String { descriptor.glyph }
}
