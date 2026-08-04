import Foundation

/// Dirijor's own session identifier. Stable for the lifetime of a logical session,
/// including across agent resume/restart (the agent-side session id may rotate; this never does).
public struct SessionID: RawRepresentable, Hashable, Codable, Sendable, CustomStringConvertible {
    public let rawValue: String

    public init(rawValue: String) { self.rawValue = rawValue }

    /// Generates a new id of the form `s_<12 hex chars>`.
    public static func generate() -> SessionID {
        SessionID(rawValue: "s_" + UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(12).lowercased())
    }

    public init(from decoder: Decoder) throws {
        rawValue = try decoder.singleValueContainer().decode(String.self)
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(rawValue)
    }

    public var description: String { rawValue }
}

public struct ProjectID: RawRepresentable, Hashable, Codable, Sendable, CustomStringConvertible {
    public let rawValue: String

    public init(rawValue: String) { self.rawValue = rawValue }

    /// Deterministic id derived from the project root path so re-adding the same
    /// folder never duplicates a project.
    public init(root: String) {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in root.utf8 {
            hash ^= UInt64(byte)
            hash = hash &* 0x1000_0000_01b3
        }
        self.rawValue = "p_" + String(format: "%012llx", hash & 0xFFFF_FFFF_FFFF)
    }

    public init(from decoder: Decoder) throws {
        rawValue = try decoder.singleValueContainer().decode(String.self)
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(rawValue)
    }

    public var description: String { rawValue }
}
