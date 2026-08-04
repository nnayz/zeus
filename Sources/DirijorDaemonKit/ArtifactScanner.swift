import DirijorCore
import Foundation

/// Extracts and classifies URLs (PRs, Linear issues, previews, plain links)
/// from a session's screen text. Input is the ANSI-free joined
/// `captureVisibleLines()` output, so escape sequences never split a URL.
struct ArtifactScanner {
    static let maxArtifacts = 50

    private struct Rule {
        let kind: ArtifactKind
        let regex: NSRegularExpression
    }

    /// Ordered specific → generic; the first classifying match per URL wins.
    private static let rules: [Rule] = {
        func rule(_ kind: ArtifactKind, _ pattern: String) -> Rule {
            Rule(
                kind: kind,
                regex: try! NSRegularExpression(
                    pattern: pattern, options: [.caseInsensitive]))
        }
        return [
            rule(.pullRequest, #"(https?://)?github\.com/[\w.-]+/[\w.-]+/pull/\d+"#),
            rule(.linearIssue, #"(https?://)?linear\.app/[\w-]+/issue/[A-Za-z][A-Za-z0-9]*-\d+(/[\w-]+)?"#),
            rule(.preview, #"https?://(localhost|127\.0\.0\.1):\d+[^\s"'\)\]]*"#),
            rule(.preview, #"https?://[^\s"'\)\]]+\.(vercel\.app|ngrok[-.][^\s"'\)\]]+)[^\s"'\)\]]*"#),
            rule(.link, #"https?://[^\s"'\)\]]+"#),
        ]
    }()

    /// Scans `text`, merges with `existing` (preserving each URL's original
    /// `firstSeenAt`), dedupes by URL, and caps at `maxArtifacts` — the oldest
    /// entries are dropped first when over the cap.
    static func scan(_ text: String, existing: [SessionArtifact], now: Date) -> [SessionArtifact] {
        var byURL: [String: SessionArtifact] = [:]
        var order: [String] = []
        for artifact in existing where byURL[artifact.url] == nil {
            byURL[artifact.url] = artifact
            order.append(artifact.url)
        }

        let range = NSRange(text.startIndex..., in: text)
        var claimed: [NSRange] = []
        for rule in rules {
            rule.regex.enumerateMatches(in: text, options: [], range: range) { match, _, _ in
                guard let match, let swiftRange = Range(match.range, in: text) else { return }
                // A URL already classified by a more specific rule (its range
                // overlaps a claimed one) must not re-match as a generic link.
                for prior in claimed where NSIntersectionRange(prior, match.range).length > 0 {
                    return
                }
                claimed.append(match.range)
                guard let url = normalize(String(text[swiftRange])) else { return }
                if byURL[url] == nil {
                    byURL[url] = SessionArtifact(kind: rule.kind, url: url, firstSeenAt: now)
                    order.append(url)
                }
            }
        }

        var result = order.compactMap { byURL[$0] }
        if result.count > maxArtifacts {
            // Keep the newest by first-seen time (stable for ties via order).
            result = Array(
                result.enumerated()
                    .sorted { ($0.element.firstSeenAt, $0.offset) < ($1.element.firstSeenAt, $1.offset) }
                    .suffix(maxArtifacts)
                    .sorted { $0.offset < $1.offset }
                    .map(\.element))
        }
        return result
    }

    /// Strips trailing punctuation/quote characters terminals love to append
    /// and guarantees a scheme (bare `github.com/...` matches get `https://`).
    static func normalize(_ raw: String) -> String? {
        var url = raw
        while let last = url.last, ".,;:!?)]}>'\"`".contains(last) {
            url.removeLast()
        }
        guard !url.isEmpty else { return nil }
        if !url.lowercased().hasPrefix("http://") && !url.lowercased().hasPrefix("https://") {
            url = "https://" + url
        }
        return url
    }

    /// Classification for a single already-normalized URL (test hook).
    static func classify(_ url: String) -> ArtifactKind? {
        for rule in rules {
            let range = NSRange(url.startIndex..., in: url)
            if rule.regex.firstMatch(in: url, options: [], range: range) != nil {
                return rule.kind
            }
        }
        return nil
    }
}
