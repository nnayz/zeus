import Foundation
import SwiftUI

// MARK: - Pairing & Auth Wire Models

struct PairPayload: Codable {
    let origin: URL
    let serverID: String
    let code: String
    let expiresAtMS: UInt64

    enum CodingKeys: String, CodingKey {
        case origin
        case serverID = "server_id"
        case code
        case expiresAtMS = "expires_at_ms"
    }
}

struct PairRequest: Codable {
    let apiMajor: UInt32
    let expectedServerID: String
    let code: String
    let deviceName: String

    enum CodingKeys: String, CodingKey {
        case apiMajor = "api_major"
        case expectedServerID = "expected_server_id"
        case code
        case deviceName = "device_name"
    }
}

struct PairResponse: Codable {
    let serverID: String
    let deviceID: String?
    let token: String
    let expiresAtMS: UInt64

    enum CodingKeys: String, CodingKey {
        case serverID = "server_id"
        case deviceID = "device_id"
        case token
        case expiresAtMS = "expires_at_ms"
    }
}

struct Hello: Codable {
    let serverID: String
    let apiMajor: UInt32
    let apiMinor: UInt32?
    let capabilities: [String]
    let engineEpoch: String?

    enum CodingKeys: String, CodingKey {
        case serverID = "server_id"
        case apiMajor = "api_major"
        case apiMinor = "api_minor"
        case capabilities
        case engineEpoch = "engine_epoch"
    }
}

// MARK: - Pagination & Projects

struct Page<T: Codable>: Codable {
    let items: [T]
    let nextOffset: Int?
    let engineEpoch: String?

    enum CodingKeys: String, CodingKey {
        case items
        case nextOffset = "next_offset"
        case engineEpoch = "engine_epoch"
    }
}

struct Project: Codable, Identifiable {
    let id: String
    let name: String
    let root: String
    let host: String?
}

// MARK: - Session Models

struct Session: Codable, Identifiable, Hashable {
    let id: String
    let projectID: String
    let kind: String
    var title: String
    let cwd: String
    let host: String?
    let status: String
    let revision: String
    let archived: Bool
    let hibernated: Bool
    let createdAtMS: Double?
    let updatedAtMS: Double?

    enum CodingKeys: String, CodingKey {
        case id
        case projectID = "project_id"
        case kind
        case title
        case cwd
        case host
        case status
        case revision
        case archived
        case hibernated
        case createdAtMS = "created_at_ms"
        case updatedAtMS = "updated_at_ms"
    }

    func hash(into hasher: inout Hasher) {
        hasher.combine(id)
    }

    static func == (lhs: Session, rhs: Session) -> Bool {
        lhs.id == rhs.id && lhs.revision == rhs.revision && lhs.status == rhs.status && lhs.title == rhs.title
    }

    var shortID: String {
        String(id.prefix(8))
    }

    var isLive: Bool {
        let s = status.lowercased()
        return !archived && !hibernated && (s == "running" || s == "active" || s == "idle")
    }

    var isHibernated: Bool {
        hibernated || status.lowercased() == "hibernated"
    }

    var displayCwd: String {
        let cleaned = cwd.trimmingCharacters(in: .whitespacesAndNewlines)
        if let home = ProcessInfo.processInfo.environment["HOME"], cleaned.hasPrefix(home) {
            return "~" + cleaned.dropFirst(home.count)
        }
        if cleaned.hasPrefix("/Users/") {
            let parts = cleaned.split(separator: "/")
            if parts.count >= 2 {
                let suffix = parts.dropFirst(2).joined(separator: "/")
                return "~/" + suffix
            }
        }
        return cleaned
    }

    var kindLabel: String {
        let k = kind.lowercased()
        if k.contains("agent") { return "Agent Workspace" }
        if k.contains("shell") || k.contains("pty") { return "Shell Session" }
        return "Command Task"
    }
}

struct SessionDetail: Codable {
    let session: Session
    let engineEpoch: String?

    enum CodingKeys: String, CodingKey {
        case session
        case engineEpoch = "engine_epoch"
    }
}

// MARK: - Screen & Terminal

struct Screen: Codable {
    let sessionID: String?
    let text: String
    let cols: UInt16
    let rows: UInt16
    let cursorRow: UInt16
    let cursorCol: UInt16
    let control: ControlState
    let exited: Bool
    let truncated: Bool

    enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
        case text
        case cols
        case rows
        case cursorRow = "cursor_row"
        case cursorCol = "cursor_col"
        case control
        case exited
        case truncated
    }

    var dimensionsDescription: String {
        "\(cols)×\(rows)"
    }

    var cursorDescription: String {
        "Ln \(cursorRow + 1), Col \(cursorCol + 1)"
    }
}

// MARK: - Terminal Control & Protocol

struct ControlEpoch: Codable, Equatable {
    let incarnation: String
    let generation: UInt64
}

struct Controller: Codable, Equatable {
    let id: String
    let label: String
    let role: String
}

struct ControlState: Codable, Equatable {
    let epoch: ControlEpoch
    let commandSeq: UInt64
    let owner: Controller?

    enum CodingKeys: String, CodingKey {
        case epoch
        case commandSeq = "command_seq"
        case owner
    }

    var hasOwner: Bool {
        owner != nil
    }
}

struct AcquireControl: Codable {
    let expected: ControlEpoch
    let takeover: Bool
}

struct ReleaseControl: Codable {
    let expected: ControlEpoch
}

struct SendText: Codable {
    let expected: ControlEpoch
    let commandSeq: UInt64
    let text: String
    let submit: Bool

    enum CodingKeys: String, CodingKey {
        case expected
        case commandSeq = "command_seq"
        case text
        case submit
    }
}

// MARK: - Mutations

enum Action: Encodable {
    case rename(title: String)
    case archive(confirmed: Bool)
    case wake(confirmed: Bool)
    case hibernate(confirmed: Bool)
    case terminate(confirmed: Bool)

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .rename(let title):
            try container.encode("rename", forKey: .kind)
            try container.encode(title, forKey: .title)
        case .archive(let confirmed):
            try container.encode("archive", forKey: .kind)
            try container.encode(confirmed, forKey: .confirmed)
        case .wake(let confirmed):
            try container.encode("wake", forKey: .kind)
            try container.encode(confirmed, forKey: .confirmed)
        case .hibernate(let confirmed):
            try container.encode("hibernate", forKey: .kind)
            try container.encode(confirmed, forKey: .confirmed)
        case .terminate(let confirmed):
            try container.encode("terminate", forKey: .kind)
            try container.encode(confirmed, forKey: .confirmed)
        }
    }

    private enum CodingKeys: String, CodingKey {
        case kind
        case title
        case confirmed
    }
}

struct Mutation: Encodable {
    let engineEpoch: String
    let mutationID: String
    let expectedRevision: String
    let expectedControl: ControlEpoch?
    let action: Action

    enum CodingKeys: String, CodingKey {
        case engineEpoch = "engine_epoch"
        case mutationID = "mutation_id"
        case expectedRevision = "expected_revision"
        case expectedControl = "expected_control"
        case action
    }
}

struct MutationResult: Codable {
    let mutationID: String
    let applied: Bool

    enum CodingKeys: String, CodingKey {
        case mutationID = "mutation_id"
        case applied
    }
}

// MARK: - Errors & Filters

struct APIError: Codable {
    let code: String
}

enum CompanionError: LocalizedError {
    case invalidPairing
    case expiredPairing
    case serverMismatch
    case unsupportedVersion
    case http(Int, String)
    case missingToken
    case insecureOrigin

    var errorDescription: String? {
        switch self {
        case .invalidPairing: "The pairing payload is invalid."
        case .expiredPairing: "The pairing code has expired."
        case .insecureOrigin: "HTTPS is required. The Companion gateway must use HTTPS."
        case .serverMismatch: "The server identity did not match."
        case .unsupportedVersion: "This Companion API version is unsupported."
        case .http(let status, let code): "Gateway error (\(status)): \(code)"
        case .missingToken: "Pair this device first."
        }
    }
}

// MARK: - Conversational Document Workspace Block Model

enum BlockKind: String, Codable {
    case userPrompt
    case assistantResponse
    case codeBlock
    case systemNote
}

struct WorkspaceBlock: Identifiable, Codable, Equatable {
    let id: String
    let kind: BlockKind
    var content: String
    var title: String?
    let timestamp: Date
    var isStreaming: Bool

    init(
        id: String = UUID().uuidString,
        kind: BlockKind,
        content: String,
        title: String? = nil,
        timestamp: Date = Date(),
        isStreaming: Bool = false
    ) {
        self.id = id
        self.kind = kind
        self.content = content
        self.title = title
        self.timestamp = timestamp
        self.isStreaming = isStreaming
    }
}
