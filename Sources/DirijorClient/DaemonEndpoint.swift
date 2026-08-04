import Foundation
import Network
import DirijorProtocol

/// Where a Dirijor client connects to the daemon.
///
/// The same client code path serves both the local macOS app (unix domain
/// socket) and the future iOS companion over Tailscale (TCP). Only
/// `Network.framework` is used so this compiles for macOS and iOS alike.
public enum DaemonEndpoint: Sendable {
    /// Local unix domain socket (default: `DirijorPaths.socket`).
    case unixSocket(path: String)
    /// TCP host/port — reserved for iOS/Tailscale remote control.
    case tcp(host: String, port: UInt16)

    /// The canonical local endpoint.
    public static var `default`: DaemonEndpoint {
        .unixSocket(path: DirijorPaths.socket.path)
    }

    /// The `NWEndpoint` to connect to.
    public var nwEndpoint: NWEndpoint {
        switch self {
        case .unixSocket(let path):
            return .unix(path: path)
        case .tcp(let host, let port):
            return .hostPort(
                host: NWEndpoint.Host(host),
                port: NWEndpoint.Port(rawValue: port) ?? .any
            )
        }
    }

    /// Parameters for the connection. Both cases want a reliable byte stream;
    /// `.tcp` parameters combined with a `.unix` endpoint open a UDS stream.
    public func makeParameters() -> NWParameters {
        let parameters = NWParameters.tcp
        // Disable Nagle-style delays: control traffic is small and latency-sensitive,
        // and terminal input wants to arrive immediately.
        if let tcp = parameters.defaultProtocolStack.internetProtocol as? NWProtocolTCP.Options {
            tcp.noDelay = true
        }
        return parameters
    }

    /// Builds (but does not start) a connection to this endpoint. The caller
    /// (an actor) owns and confines the returned non-`Sendable` connection.
    public func makeConnection() -> NWConnection {
        NWConnection(to: nwEndpoint, using: makeParameters())
    }
}
