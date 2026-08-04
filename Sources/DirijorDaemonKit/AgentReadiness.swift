import DirijorCore
import DirijorProtocol

/// Which of the manifest-declared agents are actually installed on this Mac.
///
/// Sourced from `AgentCatalog` rather than a hardcoded list: every manifest with
/// a `binary` is probed, so dropping in a new agent manifest also makes it
/// appear in the client's agent picker (greyed out until its CLI is on PATH).
/// Probing is a PATH stat per binary, not a subprocess, so widening the list
/// from four agents to the whole catalog costs nothing.
enum AgentReadiness {
    /// Every launchable agent paired with the executable we resolve for it.
    /// `shell` and `generic` are excluded — they have no binary of their own.
    static var binaries: [(AgentKind, String)] {
        AgentCatalog.shared.launchable.compactMap { descriptor in
            descriptor.binary.map { (AgentKind(id: descriptor.id), $0) }
        }
    }

    static func inspect() -> AgentReadinessResult {
        AgentReadinessResult(agents: binaries.map { kind, binary in
            AgentReadinessItem(
                kind: kind, binary: binary, path: LoginEnvironment.resolve(binary),
                descriptor: kind.descriptor)
        })
    }

    static func require(_ kind: AgentKind) throws {
        guard let binary = kind.descriptor.binary else { return }
        guard LoginEnvironment.resolve(binary) != nil else {
            throw ControlError.badRequest(
                "\(binary) was not found in your login-shell PATH. Install and sign in to \(kind.displayName), then try again.")
        }
    }
}
