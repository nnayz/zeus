import Foundation

public struct HolderPaths: Sendable {
    public let directory: URL
    public let sessionID: String

    public init(directory: URL, sessionID: String) {
        self.directory = Self.safeDirectory(preferred: directory)
        self.sessionID = sessionID
    }

    public var socket: URL { directory.appendingPathComponent("\(sessionID).sock") }
    public var pidFile: URL { directory.appendingPathComponent("\(sessionID).pid") }
    public var launchFile: URL { directory.appendingPathComponent("\(sessionID).launch.json") }

    /// Darwin's `sockaddr_un.sun_path` is only 104 bytes. Production's
    /// Application Support path fits; deeply nested XCTest temp roots may not.
    /// Use a stable, session-independent short root in that case so a fresh
    /// registry computes the same holder directory.
    public static func safeDirectory(preferred: URL) -> URL {
        let budgetedSocket = preferred
            .appendingPathComponent(String(repeating: "s", count: 40) + ".sock")
            .path.utf8.count
        guard budgetedSocket >= 100 else { return preferred }
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in preferred.path.utf8 {
            hash ^= UInt64(byte)
            hash &*= 0x0000_0100_0000_01B3
        }
        return URL(fileURLWithPath: "/tmp", isDirectory: true)
            .appendingPathComponent("dirijor-holders", isDirectory: true)
            .appendingPathComponent(String(hash, radix: 16), isDirectory: true)
    }
}

/// Stable process-level endpoint shared by every session holder in one
/// registry. The protocol major is part of the filename: a future incompatible
/// holder can start beside an older manager without taking its live PTYs away.
public struct HolderManagerPaths: Sendable {
    public static let protocolVersion = 1

    public let directory: URL

    public init(directory: URL) {
        self.directory = HolderPaths.safeDirectory(preferred: directory)
    }

    public var socket: URL {
        directory.appendingPathComponent("manager-v\(Self.protocolVersion).sock")
    }
    public var pidFile: URL {
        directory.appendingPathComponent("manager-v\(Self.protocolVersion).pid")
    }
    public var launchLock: URL {
        directory.appendingPathComponent("manager-v\(Self.protocolVersion).lock")
    }

    public static func isManagerSocket(_ url: URL) -> Bool {
        url.pathExtension == "sock"
            && url.deletingPathExtension().lastPathComponent.hasPrefix("manager-v")
    }
}

public struct HolderLaunchSpec: Codable, Sendable {
    public var sessionID: String
    public var socketPath: String
    public var pidFilePath: String
    public var logFilePath: String
    public var argv: [String]
    public var cwd: String
    public var environment: [String: String]
    public var cols: UInt16
    public var rows: UInt16
    public var diskCapacity: Int

    public init(
        sessionID: String,
        socketPath: String,
        pidFilePath: String,
        logFilePath: String,
        argv: [String],
        cwd: String,
        environment: [String: String],
        cols: UInt16,
        rows: UInt16,
        diskCapacity: Int = 32 << 20
    ) {
        self.sessionID = sessionID
        self.socketPath = socketPath
        self.pidFilePath = pidFilePath
        self.logFilePath = logFilePath
        self.argv = argv
        self.cwd = cwd
        self.environment = environment
        self.cols = cols
        self.rows = rows
        self.diskCapacity = diskCapacity
    }
}

public struct HolderProcessSample: Codable, Hashable, Sendable {
    public var pid: Int32
    public var startSec: Int64

    public init(pid: Int32, startSec: Int64) {
        self.pid = pid
        self.startSec = startSec
    }
}

public struct HolderStat: Codable, Hashable, Sendable {
    public var childPID: Int32
    public var alive: Bool
    public var logOffset: UInt64
    public var foregroundPID: Int32?
    public var cols: UInt16?
    public var rows: UInt16?
    /// Stream offset of the first byte THIS holder incarnation wrote. The
    /// per-session log survives relaunches under the same session id, so
    /// bytes below this offset — including a prior incarnation's exit marker
    /// — belong to previous incarnations and must not be attributed to this
    /// child. `nil` when talking to a holder built before this field existed.
    public var epochOffset: UInt64?

    public init(
        childPID: Int32,
        alive: Bool,
        logOffset: UInt64,
        foregroundPID: Int32? = nil,
        cols: UInt16? = nil,
        rows: UInt16? = nil,
        epochOffset: UInt64? = nil
    ) {
        self.childPID = childPID
        self.alive = alive
        self.logOffset = logOffset
        self.foregroundPID = foregroundPID
        self.cols = cols
        self.rows = rows
        self.epochOffset = epochOffset
    }
}

public struct HolderExitStatus: Codable, Hashable, Sendable {
    public enum Reason: String, Codable, Sendable {
        case exited
        case signaled
    }

    public var reason: Reason
    public var code: Int32?
    public var signal: Int32?

    public init(reason: Reason, code: Int32? = nil, signal: Int32? = nil) {
        self.reason = reason
        self.code = code
        self.signal = signal
    }
}

enum HolderOperation: String, Codable {
    case write
    case resize
    case signal
    case killTree = "kill-tree"
    case stat
}

struct HolderRequest: Codable {
    var op: HolderOperation
    var data: String?
    var cols: UInt16?
    var rows: UInt16?
    var sig: Int32?
}

struct HolderResponse: Codable {
    var ok: Bool
    var error: String?
    var stat: HolderStat?
    var tree: [HolderProcessSample]?

    static func success(
        stat: HolderStat? = nil,
        tree: [HolderProcessSample]? = nil
    ) -> HolderResponse {
        HolderResponse(ok: true, stat: stat, tree: tree)
    }

    static func failure(_ message: String) -> HolderResponse {
        HolderResponse(ok: false, error: message)
    }
}

enum HolderManagerOperation: String, Codable {
    case ping
    case launch
}

struct HolderManagerRequest: Codable {
    var version: Int
    var op: HolderManagerOperation
    var spec: HolderLaunchSpec?
}

struct HolderManagerResponse: Codable {
    var ok: Bool
    var error: String?
    var managerPID: Int32?

    static func success(managerPID: Int32) -> HolderManagerResponse {
        HolderManagerResponse(ok: true, managerPID: managerPID)
    }

    static func failure(_ message: String) -> HolderManagerResponse {
        HolderManagerResponse(ok: false, error: message)
    }
}

public enum HolderError: Error, CustomStringConvertible, Sendable {
    case invalidRequest(String)
    case transport(String)
    case rejected(String)
    case launch(String)

    public var description: String {
        switch self {
        case .invalidRequest(let message): "holder request: \(message)"
        case .transport(let message): "holder transport: \(message)"
        case .rejected(let message): "holder rejected: \(message)"
        case .launch(let message): "holder launch: \(message)"
        }
    }
}

/// An OSC marker is invisible to terminal clients but remains part of the
/// monotonic byte stream. The daemon strips it before feeding the emulator.
public enum HolderExitMarker {
    public static let prefix = Data("\u{1B}]777;dirijor-exit=".utf8)
    public static let terminator: UInt8 = 0x07

    public static func encode(_ status: HolderExitStatus) -> Data {
        guard let payload = try? JSONEncoder().encode(status) else { return Data() }
        var marker = prefix
        marker.append(Data(payload.base64EncodedString().utf8))
        marker.append(terminator)
        return marker
    }

    /// Pulls complete output and markers from a chunk accumulator. A possible
    /// split marker prefix stays buffered for the next append.
    public static func drain(_ buffer: inout Data) -> (output: Data, exit: HolderExitStatus?) {
        var output = Data()
        var exitStatus: HolderExitStatus?

        while !buffer.isEmpty {
            if let markerRange = buffer.range(of: prefix) {
                if markerRange.lowerBound > buffer.startIndex {
                    output.append(buffer[..<markerRange.lowerBound])
                    buffer.removeSubrange(..<markerRange.lowerBound)
                    continue
                }
                guard let end = buffer.firstIndex(of: terminator) else { break }
                let payloadStart = buffer.index(buffer.startIndex, offsetBy: prefix.count)
                let payload = buffer[payloadStart..<end]
                if let encoded = String(data: payload, encoding: .utf8),
                    let data = Data(base64Encoded: encoded),
                    let decoded = try? JSONDecoder().decode(HolderExitStatus.self, from: data)
                {
                    exitStatus = decoded
                }
                buffer.removeSubrange(...end)
                continue
            }

            let keep = longestSuffixOfPrefix(in: buffer)
            let emitCount = buffer.count - keep
            if emitCount > 0 {
                output.append(buffer.prefix(emitCount))
                buffer.removeFirst(emitCount)
            }
            break
        }
        return (output, exitStatus)
    }

    private static func longestSuffixOfPrefix(in data: Data) -> Int {
        let maxLength = min(data.count, prefix.count - 1)
        guard maxLength > 0 else { return 0 }
        for length in stride(from: maxLength, through: 1, by: -1) {
            if data.suffix(length).elementsEqual(prefix.prefix(length)) { return length }
        }
        return 0
    }
}
