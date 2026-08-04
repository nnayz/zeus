import Foundation

/// What runs inside a session's PTY, identified by its detection-manifest id.
///
/// This used to be a closed enum, which meant every new agent was a code change
/// spanning the daemon, the CLI, the protocol and the Rust client. It is now a
/// thin id + optional payload; everything else — display name, first-class
/// status, resume flags, approve keystrokes, env hygiene — comes from the
/// manifest via `AgentCatalog`. Adding an agent is a file drop.
///
/// The former enum cases survive as static members (`.claudeCode`, `.shell`,
/// `.generic(command:)`) so call sites read the same and the diff stayed
/// reviewable.
public struct AgentKind: Hashable, Sendable {
    /// Detection-manifest id: "claude-code", "codex", "amp", "shell", "generic".
    public let id: String
    /// Only meaningful for `.generic`: the command line the user asked for.
    /// Part of identity — two generic sessions running different commands are
    /// different kinds, which is what the old `generic(command:)` case meant.
    public let command: String?

    public init(id: String, command: String? = nil) {
        self.id = id
        self.command = command
    }

    // MARK: The kinds Dirijor names in code

    public static let claudeCode = AgentKind(id: BuiltinAgentID.claudeCode)
    public static let codex = AgentKind(id: BuiltinAgentID.codex)
    public static let cursor = AgentKind(id: BuiltinAgentID.cursor)
    public static let gemini = AgentKind(id: BuiltinAgentID.gemini)
    public static let shell = AgentKind(id: BuiltinAgentID.shell)

    public static func generic(command: String) -> AgentKind {
        AgentKind(id: BuiltinAgentID.generic, command: command)
    }

    // MARK: Manifest-sourced properties

    /// Detection-manifest id for this kind. Kept as a distinct name because the
    /// daemon reads it as "which rule set do I evaluate against".
    public var manifestID: String { id }

    public var descriptor: AgentDescriptor { AgentCatalog.shared.descriptor(for: id) }

    /// True when Dirijor has real status detection for this agent — titles,
    /// needs-input, turn completion — rather than process-alive/exited only.
    public var isFirstClass: Bool { descriptor.firstClass }

    public var displayName: String {
        // A generic command names itself; there is no manifest that could.
        if let command { return (command as NSString).lastPathComponent }
        return descriptor.displayName
    }
}

// MARK: - Codable

/// Wire and on-disk representation, unchanged in shape from the enum days: a
/// single-key object `{"<case>": {<payload>}}`, which is what Swift's
/// synthesized `Codable` produced for the old enum and what the Rust client
/// parses.
///
/// The five legacy ids keep their exact legacy keys, so state files and live
/// wire traffic written by any previous build decode here and — critically —
/// records written by THIS build still decode in an older daemon/client. Agents
/// that had no enum case encode as `{"agent": {"id": "amp"}}`; an old Rust
/// client maps that to its tolerant `Unknown` fallback rather than failing the
/// whole session list.
extension AgentKind: Codable {
    /// Legacy enum case name ⇄ manifest id. `generic` is handled separately
    /// because it carries a payload.
    private static let legacyCaseToID: [String: String] = [
        "claudeCode": BuiltinAgentID.claudeCode,
        "codex": BuiltinAgentID.codex,
        "cursor": BuiltinAgentID.cursor,
        "gemini": BuiltinAgentID.gemini,
        "shell": BuiltinAgentID.shell,
    ]
    private static let idToLegacyCase: [String: String] = [
        BuiltinAgentID.claudeCode: "claudeCode",
        BuiltinAgentID.codex: "codex",
        BuiltinAgentID.cursor: "cursor",
        BuiltinAgentID.gemini: "gemini",
        BuiltinAgentID.shell: "shell",
    ]
    /// Case key used for every manifest agent without a legacy enum case.
    private static let openCaseKey = "agent"

    private struct AnyKey: CodingKey {
        var stringValue: String
        var intValue: Int? { nil }
        init(stringValue: String) { self.stringValue = stringValue }
        init?(intValue: Int) { nil }
    }

    private struct Payload: Codable {
        var id: String?
        var command: String?
    }

    public init(from decoder: any Decoder) throws {
        // Tolerated convenience form for hand-written JSON (hosts config, MCP
        // arguments, manifest fixtures): a bare `"claude-code"` string.
        if let single = try? decoder.singleValueContainer(), let raw = try? single.decode(String.self) {
            self.init(id: Self.legacyCaseToID[raw] ?? raw)
            return
        }
        let container = try decoder.container(keyedBy: AnyKey.self)
        guard let key = container.allKeys.first else {
            throw DecodingError.dataCorrupted(
                .init(
                    codingPath: decoder.codingPath,
                    debugDescription: "AgentKind object had no case key"))
        }
        let payload = (try? container.decode(Payload.self, forKey: key)) ?? Payload()
        if let mapped = Self.legacyCaseToID[key.stringValue] {
            self.init(id: mapped)
        } else if key.stringValue == BuiltinAgentID.generic {
            self.init(id: BuiltinAgentID.generic, command: payload.command ?? "")
        } else if key.stringValue == Self.openCaseKey {
            // Missing id can't be recovered; degrade to a terminal rather than
            // failing the enclosing record (a whole state file, or a whole
            // session list).
            self.init(id: payload.id ?? BuiltinAgentID.generic, command: payload.command)
        } else {
            // A case key from a newer build we don't recognize: treat the key
            // itself as the manifest id. Costs nothing and keeps forward
            // compatibility open if the encoding ever changes again.
            self.init(id: key.stringValue, command: payload.command)
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: AnyKey.self)
        if id == BuiltinAgentID.generic {
            try container.encode(
                Payload(id: nil, command: command ?? ""), forKey: AnyKey(stringValue: id))
        } else if let legacy = Self.idToLegacyCase[id] {
            try container.encode(Payload(), forKey: AnyKey(stringValue: legacy))
        } else {
            try container.encode(
                Payload(id: id, command: command),
                forKey: AnyKey(stringValue: Self.openCaseKey))
        }
    }
}

// MARK: - Debugging

extension AgentKind: CustomStringConvertible {
    public var description: String {
        command.map { "\(id)(\($0))" } ?? id
    }
}
