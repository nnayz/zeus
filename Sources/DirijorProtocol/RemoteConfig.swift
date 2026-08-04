import Foundation

/// Remote-access configuration for the daemon's TCP listener (the iOS companion
/// over Tailscale). Written by the Mac app's Settings → Remote pane to
/// `DirijorPaths.remoteConfigFile`; read by the daemon at startup. Its presence
/// alone enables the TCP listener — no file means remote access is off (the
/// historical, UDS-only behavior).
///
/// The token is a shared secret: the file is written 0o600 so only the user can
/// read it, and the token gates every remote connection (see the daemon's
/// per-connection token enforcement).
public struct RemoteConfig: Codable, Sendable, Equatable {
    /// TCP port the daemon listens on. The app uses a fixed 48620.
    public var port: UInt16
    /// Exact local address to bind. Production writes the Mac's Tailscale IPv4
    /// so the service is unreachable through Wi-Fi/Ethernet. A missing value
    /// from a legacy config fails closed to loopback.
    public var bindHost: String?
    /// Base64url shared secret required in `hello`/`attach` from remote peers.
    public var token: String
    /// Test/debug escape hatch: when true, remote port forwards may target any
    /// localhost TCP port instead of only ports tracked on live sessions.
    public var forwardAnyPort: Bool?

    public init(
        port: UInt16, token: String,
        bindHost: String? = "127.0.0.1",
        forwardAnyPort: Bool? = nil
    ) {
        self.port = port
        self.token = token
        self.bindHost = bindHost
        self.forwardAnyPort = forwardAnyPort
    }

    /// Loads a RemoteConfig from `url`, or nil if the file is absent/unreadable
    /// (nil ⇒ remote access disabled).
    public static func load(from url: URL) -> RemoteConfig? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(RemoteConfig.self, from: data)
    }

    /// Atomically writes this config to `url` with 0o600 permissions so the
    /// shared secret is owner-readable only.
    public func save(to url: URL) throws {
        let data = try JSONEncoder().encode(self)
        // Atomic write, then tighten permissions (the atomic write via a temp
        // file does not preserve a restrictive mode on its own).
        try data.write(to: url, options: [.atomic])
        try FileManager.default.setAttributes(
            [.posixPermissions: NSNumber(value: Int16(0o600))], ofItemAtPath: url.path)
    }

    /// Removes the config file if present (turning remote access off). Missing
    /// file is not an error.
    public static func remove(at url: URL) throws {
        if FileManager.default.fileExists(atPath: url.path) {
            try FileManager.default.removeItem(at: url)
        }
    }
}
