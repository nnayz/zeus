import Foundation

struct PairPayload: Codable {
    let origin: URL
    let serverID: String
    let code: String
    let expiresAtMS: UInt64
    enum CodingKeys: String, CodingKey { case origin, serverID = "server_id", code, expiresAtMS = "expires_at_ms" }
}

struct PairRequest: Codable {
    let apiMajor: UInt32
    let expectedServerID: String
    let code: String
    let deviceName: String
    enum CodingKeys: String, CodingKey { case apiMajor = "api_major", expectedServerID = "expected_server_id", code, deviceName = "device_name" }
}

struct PairResponse: Codable {
    let serverID: String
    let token: String
    let expiresAtMS: UInt64
    enum CodingKeys: String, CodingKey { case serverID = "server_id", token, expiresAtMS = "expires_at_ms" }
}

struct Hello: Codable {
    let serverID: String
    let apiMajor: UInt32
    let capabilities: [String]
    enum CodingKeys: String, CodingKey { case serverID = "server_id", apiMajor = "api_major", capabilities }
}

struct Page<T: Codable>: Codable { let items: [T]; let nextOffset: Int?; enum CodingKeys: String, CodingKey { case items, nextOffset = "next_offset" } }

struct Project: Codable, Identifiable { let id: String; let name: String; let root: String; let host: String? }

struct Session: Codable, Identifiable {
    let id: String; let projectID: String; let kind: String; var title: String; let cwd: String
    let host: String?; let status: String; let revision: String; let archived: Bool; let hibernated: Bool
    enum CodingKeys: String, CodingKey { case id, projectID = "project_id", kind, title, cwd, host, status, revision, archived, hibernated }
}

struct SessionDetail: Codable { let session: Session }

struct Screen: Codable {
    let text: String; let cols: UInt16; let rows: UInt16; let cursorRow: UInt16; let cursorCol: UInt16
    let control: ControlState; let exited: Bool; let truncated: Bool
    enum CodingKeys: String, CodingKey { case text, cols, rows, cursorRow = "cursor_row", cursorCol = "cursor_col", control, exited, truncated }
}

struct ControlEpoch: Codable { let incarnation: String; let generation: UInt64 }
struct ControlState: Codable { let epoch: ControlEpoch; let commandSeq: UInt64; let owner: Controller?; enum CodingKeys: String, CodingKey { case epoch, commandSeq = "command_seq", owner } }
struct Controller: Codable { let id: String; let label: String; let role: String }
struct AcquireControl: Codable { let expected: ControlEpoch; let takeover: Bool }
struct SendText: Codable { let expected: ControlEpoch; let commandSeq: UInt64; let text: String; let submit: Bool; enum CodingKeys: String, CodingKey { case expected, commandSeq = "command_seq", text, submit } }
struct APIError: Codable { let code: String }

enum CompanionError: LocalizedError {
    case invalidPairing, expiredPairing, serverMismatch, unsupportedVersion, http(Int, String), missingToken, insecureOrigin
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
