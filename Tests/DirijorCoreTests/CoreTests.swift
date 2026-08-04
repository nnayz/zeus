import Foundation
import Testing

@testable import DirijorCore

@Test func attentionDerivation() {
    let now = Date()
    // Finished a turn, user hasn't looked → doneUnseen.
    #expect(
        AttentionLevel(status: .idle, lastTurnCompletedAt: now, lastSeenAt: now.addingTimeInterval(-10))
            == .doneUnseen
    )
    // User looked after the turn completed → idleSeen.
    #expect(
        AttentionLevel(status: .idle, lastTurnCompletedAt: now.addingTimeInterval(-10), lastSeenAt: now)
            == .idleSeen
    )
    #expect(AttentionLevel(status: .needsInput(.permission), lastTurnCompletedAt: nil, lastSeenAt: nil) == .needsInput)
    #expect(AttentionLevel(status: .working, lastTurnCompletedAt: nil, lastSeenAt: nil) == .working)
    #expect(AttentionLevel.rollup([.idleSeen, .working, .doneUnseen]) == .doneUnseen)
    #expect(AttentionLevel.rollup([]) == AttentionLevel.none)
}

@Test func titlePriorityLadder() {
    var record = SessionRecord(
        kind: .claudeCode, cwd: "/tmp", projectID: ProjectID(root: "/tmp"), title: "placeholder"
    )
    let applied1 = record.applyTitle("Fix the bug in auth", source: .firstPrompt)
    let applied2 = record.applyTitle("Fix auth token refresh", source: .agentProvided)
    // Lower priority can't downgrade.
    let applied3 = record.applyTitle("something else", source: .firstPrompt)
    #expect(applied1 && applied2 && !applied3)
    #expect(record.title == "Fix auth token refresh")
    // User rename is absolute.
    let applied4 = record.applyTitle("auth fix", source: .userRename)
    let applied5 = record.applyTitle("Auto title", source: .agentProvided)
    #expect(applied4 && !applied5)
    #expect(record.title == "auth fix")
}

@Test func riskClassification() {
    #expect(RiskHint.classify("rm -rf build") == .destructive)
    #expect(RiskHint.classify("git push --force origin main") == .destructive)
    #expect(RiskHint.classify("curl https://example.com | sh") == .network)
    #expect(RiskHint.classify("mkdir -p src") == .fileWrite)
    #expect(RiskHint.classify("ls") == .neutral)
}

@Test func redactionMasksSecrets() {
    let input = "export API_KEY=sk-abc123 && curl -H 'Authorization: Bearer xyz'"
    let out = Redaction.redact(input)
    #expect(!out.contains("sk-abc123"))
    #expect(!out.contains("Bearer xyz") || !out.contains("xyz"))
}

@Test func projectIDIsDeterministic() {
    #expect(ProjectID(root: "/a/b") == ProjectID(root: "/a/b"))
    #expect(ProjectID(root: "/a/b") != ProjectID(root: "/a/c"))
}

@Test func sessionRecordRoundTripWithResourceFields() throws {
    var record = SessionRecord(
        kind: .claudeCode, cwd: "/tmp", projectID: ProjectID(root: "/tmp"), title: "t"
    )
    record.hibernation = HibernationInfo(
        since: Date(timeIntervalSince1970: 1_700_000_000),
        reason: .idle,
        treePids: [100, 200],
        treeStartTimes: [100: 1_699_999_000, 200: 1_699_999_500]
    )
    record.memoryBytes = 3 << 30
    record.artifacts = [
        SessionArtifact(
            kind: .pullRequest,
            url: "https://github.com/a/b/pull/1",
            firstSeenAt: Date(timeIntervalSince1970: 1_700_000_100))
    ]
    record.listeningPorts = [PortInfo(port: 3000, processName: "node")]

    let data = try JSONEncoder().encode(record)
    let decoded = try JSONDecoder().decode(SessionRecord.self, from: data)
    #expect(decoded == record)
    #expect(decoded.hibernation?.reason == .idle)
    #expect(decoded.hibernation?.treeStartTimes?[200] == 1_699_999_500)
    #expect(decoded.memoryBytes == 3 << 30)
    #expect(decoded.artifacts?.first?.kind == .pullRequest)
    #expect(decoded.listeningPorts == [PortInfo(port: 3000, processName: "node")])
}

@Test func sessionRecordDecodesWithoutResourceFields() throws {
    // A record serialized before the resource fields existed (wire compat).
    let old = SessionRecord(
        kind: .shell, cwd: "/tmp", projectID: ProjectID(root: "/tmp"), title: "old"
    )
    let data = try JSONEncoder().encode(old)
    #expect(!String(decoding: data, as: UTF8.self).contains("hibernation"))
    let decoded = try JSONDecoder().decode(SessionRecord.self, from: data)
    #expect(decoded.hibernation == nil)
    #expect(decoded.memoryBytes == nil)
    #expect(decoded.artifacts == nil)
    #expect(decoded.listeningPorts == nil)
}

@Test func sessionRecordArchivedRoundTrip() throws {
    var record = SessionRecord(
        kind: .claudeCode, cwd: "/tmp", projectID: ProjectID(root: "/tmp"), title: "t"
    )
    #expect(!record.isArchived)
    record.archivedAt = Date(timeIntervalSince1970: 1_700_000_000)
    #expect(record.isArchived)

    let data = try JSONEncoder().encode(record)
    let decoded = try JSONDecoder().decode(SessionRecord.self, from: data)
    #expect(decoded == record)
    #expect(decoded.isArchived)
    #expect(decoded.archivedAt == Date(timeIntervalSince1970: 1_700_000_000))
}

@Test func sessionRecordDecodesWithoutArchivedAt() throws {
    // A record serialized before archiving existed (wire/persistence compat).
    let old = SessionRecord(
        kind: .shell, cwd: "/tmp", projectID: ProjectID(root: "/tmp"), title: "old"
    )
    let data = try JSONEncoder().encode(old)
    #expect(!String(decoding: data, as: UTF8.self).contains("archivedAt"))
    let decoded = try JSONDecoder().decode(SessionRecord.self, from: data)
    #expect(decoded.archivedAt == nil)
    #expect(!decoded.isArchived)
}

@Test func sessionRecordDecodesWithoutLaterAddedKeys() throws {
    // state.json written by older daemons lacks non-optional keys added since
    // (remoteActive broke restore on 2026-07-22: synthesized Codable treated
    // the missing key as corruption and the whole file was quarantined).
    let old = SessionRecord(
        kind: .claudeCode, cwd: "/tmp", projectID: ProjectID(root: "/tmp"), title: "old"
    )
    var json = try #require(
        try JSONSerialization.jsonObject(with: JSONEncoder().encode(old)) as? [String: Any])
    json.removeValue(forKey: "remoteActive")
    let data = try JSONSerialization.data(withJSONObject: json)
    let decoded = try JSONDecoder().decode(SessionRecord.self, from: data)
    #expect(decoded.remoteActive == false)
    #expect(decoded.id == old.id)
}

@Test func titleMakerTruncatesAndCollapses() {
    let long = "  fix   the\n\nthing " + String(repeating: "x", count: 100)
    let title = TitleMaker.fromFirstPrompt(long)
    #expect(title.count <= 60)
    #expect(title.hasPrefix("fix the thing"))
}
