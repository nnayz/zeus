import Foundation

/// The set of agents Dirijor knows how to run, loaded from manifest files.
///
/// One JSON file per agent under `Resources/manifests/`, each carrying an
/// `"agent"` descriptor block (what this type reads) plus the detection `rules`
/// (which `DirijorDetection.ManifestEngine` reads from the same files). Both
/// halves live in one file so "add an agent" stays one file drop.
///
/// The catalog is process-wide and immutable after first use: descriptors feed
/// `AgentKind.displayName` / `isFirstClass`, which are read from every actor in
/// the daemon and from SwiftUI view bodies. Hot-reloading them would mean
/// making `AgentKind` async, so overrides are picked up at process start (the
/// daemon restarts on upgrade anyway) rather than live.
public struct AgentCatalog: Sendable {
    /// Keyed by manifest id.
    public let descriptors: [String: AgentDescriptor]
    /// Load order: bundled manifests sorted by id, then override-only agents.
    /// Gives menus and `dirijor agents` a stable, non-random ordering.
    public let ordered: [AgentDescriptor]

    public init(descriptors: [AgentDescriptor]) {
        var byID: [String: AgentDescriptor] = [:]
        for descriptor in descriptors { byID[descriptor.id] = descriptor }
        self.descriptors = byID
        self.ordered = byID.values.sorted { $0.id < $1.id }
    }

    /// The descriptor for `id`, or a conservative terminal-shaped fallback so a
    /// record persisted by a newer build (or referencing a manifest the user
    /// deleted) still renders instead of crashing.
    public func descriptor(for id: String) -> AgentDescriptor {
        descriptors[id] ?? .fallback(id: id)
    }

    /// Agents that can be offered in a "new session" menu: everything with a
    /// real CLI behind it, in id order.
    public var launchable: [AgentDescriptor] {
        ordered.filter { $0.binary != nil }
    }

    /// Resolves a user-typed agent name (`"claude"`, `"claude-code"`, `"amp"`)
    /// to a manifest id. Case-insensitive; matches id, shortLabel, or alias.
    public func resolve(name: String) -> AgentDescriptor? {
        let needle = name.lowercased()
        if let exact = descriptors[needle] { return exact }
        return ordered.first {
            $0.shortLabel.lowercased() == needle
                || $0.aliases.contains { $0.lowercased() == needle }
        }
    }

    /// Every env prefix any known agent uses to mark a nested session. The
    /// daemon strips the union from inherited environments — see
    /// `InjectionBuilder.sanitizeInheritedEnvironment`.
    public var envScrubPrefixes: [String] {
        var seen = Set<String>()
        var result: [String] = []
        for descriptor in ordered {
            for prefix in descriptor.envScrubPrefixes where seen.insert(prefix).inserted {
                result.append(prefix)
            }
        }
        return result
    }

    // MARK: Shared instance

    /// Loaded once per process from the bundle + the user's override directory.
    public static let shared: AgentCatalog = load()

    /// `~/Library/Application Support/Dirijor/manifests/overrides`.
    ///
    /// Duplicated from `DirijorPaths` on purpose: DirijorProtocol depends on
    /// DirijorCore, so Core cannot import it back. Keep the two in sync.
    public static var overridesDirectory: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Dirijor/manifests/overrides", isDirectory: true)
    }

    /// Manifest files in load order: bundled first, then user overrides (which
    /// replace a bundled agent of the same id). Also what DirijorDetection
    /// walks for detection rules, so both halves of a file always agree.
    public static func manifestURLs(overridesDirectory: URL? = overridesDirectory) -> [URL] {
        var urls: [URL] = []
        if let dir = ResourceBundle.core?.url(forResource: "manifests", withExtension: nil) {
            urls += jsonFiles(in: dir)
        }
        if let overridesDirectory { urls += jsonFiles(in: overridesDirectory) }
        return urls
    }

    private static func jsonFiles(in directory: URL) -> [URL] {
        let contents =
            (try? FileManager.default.contentsOfDirectory(
                at: directory, includingPropertiesForKeys: nil)) ?? []
        return contents.filter { $0.pathExtension == "json" }.sorted { $0.path < $1.path }
    }

    /// One manifest file, decoded far enough to reach the `"agent"` block.
    /// Missing block ⇒ nil (a detection-only manifest for an agent that some
    /// other file declares, or an override that only tweaks rules).
    private struct ManifestHead: Decodable {
        let id: String
        let agent: AgentDescriptor?

        private enum CodingKeys: String, CodingKey { case id, agent }

        init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            id = try c.decode(String.self, forKey: .id)
            // The descriptor's `id` is optional in the file — it defaults to the
            // manifest id so a manifest can't disagree with itself.
            if var nested = try c.decodeIfPresent(AgentDescriptor.self, forKey: .agent) {
                nested.id = id
                if nested.shortLabel.isEmpty { nested.shortLabel = id }
                agent = nested
            } else {
                agent = nil
            }
        }
    }

    static func load(overridesDirectory: URL? = overridesDirectory) -> AgentCatalog {
        var descriptors: [AgentDescriptor] = []
        for url in manifestURLs(overridesDirectory: overridesDirectory) {
            guard let data = try? Data(contentsOf: url),
                let head = try? JSONDecoder().decode(ManifestHead.self, from: data),
                let agent = head.agent
            else { continue }
            descriptors.append(agent)
        }
        return AgentCatalog(descriptors: descriptors)
    }
}

/// Manifest `id` values Dirijor itself has behavior for. Everything else is
/// pure data; these exist because the daemon has to name them in code (Claude's
/// transcript-path prediction, Codex's notify parser, the shell/generic
/// pseudo-agents that don't correspond to a CLI at all).
public enum BuiltinAgentID {
    public static let claudeCode = "claude-code"
    public static let codex = "codex"
    public static let cursor = "cursor"
    public static let gemini = "gemini"
    /// The user's login shell — not an agent, but a spawnable kind.
    public static let shell = "shell"
    /// An arbitrary command line typed by the user; carries its own payload.
    public static let generic = "generic"
}
