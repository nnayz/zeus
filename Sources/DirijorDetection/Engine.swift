import DirijorCore
import Foundation

/// Immutable, thread-safe manifest storage built at engine init.
struct EngineStorage: Sendable {
    let manifests: [String: Manifest]
}

/// Cache key for per-snapshot region memoization: a region's extracted text
/// depends only on its kind and requested line count.
struct RegionCacheKey: Hashable {
    let region: RegionKind
    let regionLines: Int
}

extension ManifestEngine {
    /// Manifest files are the same files `AgentCatalog` reads for the `agent`
    /// descriptor half — one file per agent, bundled first then user overrides,
    /// later files replacing earlier ones by id. Sourcing the URL list from the
    /// catalog is what keeps an agent's rules and its capabilities from drifting
    /// into separate files.
    ///
    /// Rule decoding is best-effort per file: a syntactically broken override
    /// must not take out the whole engine (and with it every other agent's
    /// detection) — it just leaves its agent on the previous definition.
    init(loadingBundledWithOverrides overridesDir: URL?) throws {
        var manifests: [String: Manifest] = [:]
        for url in AgentCatalog.manifestURLs(overridesDirectory: overridesDir) {
            guard let data = try? Data(contentsOf: url),
                let manifest = try? JSONDecoder().decode(Manifest.self, from: data)
            else { continue }
            manifests[manifest.id] = manifest
        }
        self.storage = EngineStorage(manifests: manifests)
    }

    func evaluateImpl(_ snapshot: ScreenSnapshot, manifestID: String) -> ScreenObservation? {
        guard let manifest = storage.manifests[manifestID] else { return nil }

        // Region text is shared across rules (5 of claude's 10 rules read
        // `whole_recent`); extract + join each region at most once per snapshot
        // rather than rebuilding a full-screen string per rule. Keyed by region
        // *and* line count since `bottom_non_empty_lines` varies by count.
        var regionCache: [RegionCacheKey: (lines: [String], text: String)] = [:]
        func context(region: RegionKind, regionLines: Int) -> PredicateContext {
            let key = RegionCacheKey(region: region, regionLines: regionLines)
            let entry: (lines: [String], text: String)
            if let cached = regionCache[key] {
                entry = cached
            } else {
                let lines = Regions.extract(region, regionLines: regionLines, from: snapshot)
                entry = (lines, lines.joined(separator: "\n"))
                regionCache[key] = entry
            }
            return PredicateContext(
                text: entry.text, lines: entry.lines, progressState: snapshot.oscProgressState)
        }

        // `manifest.rules` is pre-sorted by descending priority, so the first
        // rule whose predicate matches is the highest-priority match — take it
        // and stop, instead of scoring every rule each frame.
        var winner: Rule?
        for rule in manifest.rules {
            let ctx = context(region: rule.region, regionLines: rule.regionLines)
            if rule.when.evaluate(ctx) {
                winner = rule
                break
            }
        }

        guard let rule = winner else { return nil }

        var excerpt: String?
        var options: [String]?

        // Determine the capture region: explicit `capture`, or (for blockers)
        // default to the prompt box.
        let captureRegion: RegionKind?
        let captureLineCount: Int
        let maxChars: Int
        if let cap = rule.capture {
            captureRegion = cap.region
            captureLineCount = cap.regionLines
            maxChars = cap.maxChars
        } else if rule.isBlocker {
            captureRegion = .prompt_box_body
            captureLineCount = 5
            maxChars = 400
        } else {
            captureRegion = nil
            captureLineCount = 5
            maxChars = 400
        }

        if let captureRegion {
            let capLines = Regions.extract(captureRegion, regionLines: captureLineCount, from: snapshot)
            let joined = capLines.joined(separator: "\n")
            if !joined.isEmpty {
                excerpt = String(Redaction.redact(joined).prefix(maxChars))
            }
            if rule.isBlocker {
                let opts = Regions.numberedOptions(capLines)
                if !opts.isEmpty { options = opts }
            }
        }

        return ScreenObservation(
            state: rule.state,
            matchedRuleID: rule.id,
            priority: rule.priority,
            contentSeq: snapshot.contentSeq,
            promptExcerpt: excerpt,
            options: options)
    }
}
