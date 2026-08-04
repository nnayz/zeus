import DirijorCore
import Foundation
import Testing

@testable import DirijorDaemonKit

@Test func artifactClassification() {
    let now = Date()
    let text = """
        Opened https://github.com/acme/widgets/pull/123 for review.
        Tracking in https://linear.app/acme/issue/ENG-42/fix-the-thing
        Dev server on http://localhost:3000/dashboard
        Preview: https://widgets-git-main-acme.vercel.app/home
        Tunnel: https://f00d.ngrok-free.app/hook
        Docs at https://example.com/docs.
        """
    let artifacts = ArtifactScanner.scan(text, existing: [], now: now)
    let byURL = Dictionary(uniqueKeysWithValues: artifacts.map { ($0.url, $0.kind) })

    #expect(byURL["https://github.com/acme/widgets/pull/123"] == .pullRequest)
    #expect(byURL["https://linear.app/acme/issue/ENG-42/fix-the-thing"] == .linearIssue)
    #expect(byURL["http://localhost:3000/dashboard"] == .preview)
    #expect(byURL["https://widgets-git-main-acme.vercel.app/home"] == .preview)
    #expect(byURL["https://f00d.ngrok-free.app/hook"] == .preview)
    // Trailing period stripped; classified as plain link.
    #expect(byURL["https://example.com/docs"] == .link)
    // The PR URL must not ALSO appear as a generic link.
    #expect(artifacts.filter { $0.url.contains("/pull/123") }.count == 1)
}

@Test func artifactSchemeIsAddedToBareMatches() {
    let artifacts = ArtifactScanner.scan(
        "see github.com/acme/widgets/pull/7 please", existing: [], now: Date())
    #expect(artifacts.count == 1)
    #expect(artifacts[0].url == "https://github.com/acme/widgets/pull/7")
    #expect(artifacts[0].kind == .pullRequest)
}

@Test func artifactTrailingPunctuationStripped() {
    #expect(ArtifactScanner.normalize("https://example.com/a),") == "https://example.com/a")
    #expect(ArtifactScanner.normalize("https://example.com/a\"") == "https://example.com/a")
    #expect(ArtifactScanner.normalize("https://example.com/a;") == "https://example.com/a")
}

@Test func artifactDedupePreservesFirstSeen() {
    let early = Date(timeIntervalSince1970: 1000)
    let late = Date(timeIntervalSince1970: 2000)
    let existing = [
        SessionArtifact(
            kind: .pullRequest,
            url: "https://github.com/acme/widgets/pull/123",
            firstSeenAt: early)
    ]
    let rescanned = ArtifactScanner.scan(
        "still open: https://github.com/acme/widgets/pull/123",
        existing: existing, now: late)
    #expect(rescanned.count == 1)
    #expect(rescanned[0].firstSeenAt == early)
}

@Test func artifactCapKeepsNewest() {
    var existing: [SessionArtifact] = []
    for i in 0..<ArtifactScanner.maxArtifacts {
        existing.append(
            SessionArtifact(
                kind: .link,
                url: "https://example.com/page/\(i)",
                firstSeenAt: Date(timeIntervalSince1970: Double(i))))
    }
    let now = Date()
    let result = ArtifactScanner.scan(
        "new: https://github.com/acme/widgets/pull/999", existing: existing, now: now)
    #expect(result.count == ArtifactScanner.maxArtifacts)
    #expect(result.contains { $0.url == "https://github.com/acme/widgets/pull/999" })
    // The oldest existing entry was evicted.
    #expect(!result.contains { $0.url == "https://example.com/page/0" })
}

@Test func artifactClassifyHelper() {
    #expect(ArtifactScanner.classify("https://github.com/a/b/pull/1") == .pullRequest)
    #expect(ArtifactScanner.classify("https://linear.app/acme/issue/ABC-9") == .linearIssue)
    #expect(ArtifactScanner.classify("http://127.0.0.1:8123") == .preview)
    #expect(ArtifactScanner.classify("https://x.vercel.app") == .preview)
    #expect(ArtifactScanner.classify("https://news.example.org/story") == .link)
    #expect(ArtifactScanner.classify("not a url") == nil)
}
