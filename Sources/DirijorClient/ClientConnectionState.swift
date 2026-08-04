import Foundation
import DirijorProtocol

/// Observable connection state of a `DaemonClient`, delivered on
/// `DaemonClient.connectionState`.
///
/// `@unchecked Sendable`: the only non-`Sendable`-by-default payload is the
/// `Error?` in `.disconnected`. In practice the errors flowing through are
/// `NWError`, `ControlError`, and `CancellationError`, all of which are safe to
/// hand between concurrency domains; honoring the spec's `.disconnected(Error?)`
/// shape is worth this narrow, documented escape hatch.
public enum ClientConnectionState: @unchecked Sendable {
    /// Establishing (or re-establishing) the connection.
    case connecting
    /// Connected and the `hello` handshake completed.
    case connected(HelloResult)
    /// The connection dropped or failed. Carries the underlying error when known.
    case disconnected(Error?)
}
