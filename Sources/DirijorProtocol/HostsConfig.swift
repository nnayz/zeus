import Foundation

/// One configured remote host reachable over SSH. Sessions spawned "on" a host
/// run the agent on that machine inside a tmux session (`ssh -t … tmux
/// new-session -A …`) so the agent survives SSH drops; the local daemon owns
/// the PTY running the ssh client.
public struct HostNodeConfig: Codable, Sendable, Equatable {
    public var endpoint: String
    public var tokenFile: String
    public var nodeID: String?

    enum CodingKeys: String, CodingKey {
        case endpoint, tokenFile
        case nodeID = "nodeId"
    }

    public init(endpoint: String, tokenFile: String, nodeID: String? = nil) {
        self.endpoint = endpoint
        self.tokenFile = tokenFile
        self.nodeID = nodeID
    }
}

public struct HostEntry: Codable, Sendable, Equatable, Identifiable {
    /// Stable identifier referenced by `SessionSpawnParams.host` /
    /// `SessionRecord.host`.
    public var id: String
    /// Human-readable name for pickers and badges; falls back to `id`.
    public var name: String?
    /// SSH destination (`user@host`, or an ssh_config alias).
    public var ssh: String
    /// Default remote working directory for new sessions (e.g. `~/code`).
    public var defaultCwd: String?
    /// First-party management endpoint. SSH remains the terminal/recovery
    /// fallback until this host has a compatible node installed.
    public var node: HostNodeConfig?

    public var displayName: String { name ?? id }

    public init(
        id: String,
        name: String? = nil,
        ssh: String,
        defaultCwd: String? = nil,
        node: HostNodeConfig? = nil
    ) {
        self.id = id
        self.name = name
        self.ssh = ssh
        self.defaultCwd = defaultCwd
        self.node = node
    }
}

/// Remote host catalog, stored next to `remote.json` at
/// `DirijorPaths.hostsConfigFile` (daemon-owned). Read by the daemon at spawn
/// time and by clients on the same machine for the host picker. A missing or
/// invalid file means "no remote hosts": pickers show Local only.
public struct HostsConfig: Codable, Sendable, Equatable {
    public var hosts: [HostEntry]

    public init(hosts: [HostEntry] = []) {
        self.hosts = hosts
    }

    /// Loads a HostsConfig from `url`, or nil if the file is absent/unreadable
    /// or malformed (nil ⇒ no remote hosts configured).
    public static func load(from url: URL) -> HostsConfig? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(HostsConfig.self, from: data)
    }

    /// Atomically writes this catalog to `url` with 0o600 permissions,
    /// matching the sibling `remote.json` hygiene.
    public func save(to url: URL) throws {
        let data = try JSONEncoder().encode(self)
        try data.write(to: url, options: [.atomic])
        try FileManager.default.setAttributes(
            [.posixPermissions: NSNumber(value: Int16(0o600))], ofItemAtPath: url.path)
    }

    public func host(id: String) -> HostEntry? {
        hosts.first { $0.id == id }
    }
}
