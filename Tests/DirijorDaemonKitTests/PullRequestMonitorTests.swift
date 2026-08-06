import DirijorCore
import Foundation
import Testing

@testable import DirijorDaemonKit

private let prURL = "https://github.com/acme/widgets/pull/123"

@Test func pullRequestPollingHasABoundedSubprocessBudget() {
    #expect(
        PullRequestMonitor.maxFetchesPerTick <= 2,
        "a monitor tick must leave the daemon idle instead of serially spawning gh for every PR"
    )
}

@Test func ghPayloadParsing() {
    let json = """
        {
          "number": 123,
          "title": "Fix scroll jitter",
          "author": {"login": "antfu"},
          "body": "Moves repository cloning off the foreground path.",
          "baseRefName": "main",
          "headRefName": "refactor/repository-cloning",
          "state": "OPEN",
          "isDraft": false,
          "reviewDecision": "APPROVED",
          "mergeable": "MERGEABLE",
          "mergeStateStatus": "CLEAN",
          "additions": 45,
          "deletions": 12,
          "changedFiles": 3,
          "comments": [
            {"author": {"login": "ruru"}, "body": "nice",
             "createdAt": "2026-08-04T09:10:00Z", "url": "https://github.com/acme/widgets/pull/123#issuecomment-1"},
            {"author": {"login": "yyx990803"}, "body": "ship it",
             "createdAt": "2026-08-04T10:10:00Z"}
          ],
          "reviews": [{"author": {"login": "octocat"}, "state": "APPROVED", "body": "Looks good",
                       "submittedAt": "2026-08-04T10:30:00Z"}],
          "statusCheckRollup": [
            {"__typename": "CheckRun", "name": "build", "workflowName": "CI",
             "status": "COMPLETED", "conclusion": "SUCCESS",
             "detailsUrl": "https://github.com/acme/widgets/runs/1"},
            {"__typename": "CheckRun", "name": "test", "status": "IN_PROGRESS", "conclusion": ""},
            {"__typename": "StatusContext", "context": "vercel", "state": "FAILURE"}
          ]
        }
        """
    let now = Date()
    let status = PullRequestMonitor.parse(Data(json.utf8), url: prURL, now: now)

    #expect(status != nil)
    guard let status else { return }
    #expect(status.url == prURL)
    #expect(status.number == 123)
    #expect(status.title == "Fix scroll jitter")
    #expect(status.author == "antfu")
    #expect(status.body == "Moves repository cloning off the foreground path.")
    #expect(status.baseRefName == "main")
    #expect(status.headRefName == "refactor/repository-cloning")
    #expect(status.state == "OPEN")
    #expect(!status.isDraft)
    #expect(status.reviewDecision == "APPROVED")
    #expect(status.mergeable == "MERGEABLE")
    #expect(status.additions == 45)
    #expect(status.deletions == 12)
    #expect(status.changedFiles == 3)
    #expect(status.commentCount == 2)
    #expect(status.reviewCount == 1)
    #expect(status.checksPassed == 1)
    #expect(status.checksFailed == 1)
    #expect(status.checksPending == 1)
    #expect(status.fetchedAt == now)

    let checks = status.checks ?? []
    #expect(checks.count == 3)
    #expect(checks[0].name == "CI / build")
    #expect(checks[0].result == "pass")
    #expect(checks[0].url == "https://github.com/acme/widgets/runs/1")
    // Empty conclusion while IN_PROGRESS must bucket as pending, not pass.
    #expect(checks[1].name == "test")
    #expect(checks[1].result == "pending")
    #expect(checks[1].detail == "IN_PROGRESS")
    #expect(checks[2].name == "vercel")
    #expect(checks[2].result == "fail")

    let discussion = status.discussion ?? []
    #expect(discussion.count == 3)
    #expect(discussion[0].author == "ruru")
    #expect(discussion[0].body == "nice")
    #expect(discussion[2].kind == "review")
    #expect(discussion[2].state == "APPROVED")
}

@Test func ghPayloadParsingMinimalFields() {
    // gh omits reviewDecision entirely on repos without required reviews and
    // returns "" on others; both must land as nil.
    let json = """
        {"number": 7, "state": "MERGED", "reviewDecision": ""}
        """
    let status = PullRequestMonitor.parse(Data(json.utf8), url: prURL, now: Date())
    #expect(status != nil)
    #expect(status?.state == "MERGED")
    #expect(status?.reviewDecision == nil)
    #expect(status?.commentCount == 0)
    #expect(status?.checksPassed == 0)
}

@Test func ghPayloadParsingRejectsGarbage() {
    #expect(PullRequestMonitor.parse(Data("not json".utf8), url: prURL, now: Date()) == nil)
    #expect(PullRequestMonitor.parse(Data("{}".utf8), url: prURL, now: Date()) == nil)
}

@Test func overallRollupLadder() {
    func status(
        state: String = "OPEN", draft: Bool = false, decision: String? = nil,
        mergeable: String? = nil, mergeState: String? = nil,
        failed: Int = 0, pending: Int = 0
    ) -> PullRequestStatus {
        PullRequestStatus(
            url: prURL, number: 1, state: state, isDraft: draft,
            reviewDecision: decision, mergeable: mergeable, mergeStateStatus: mergeState,
            checksFailed: failed, checksPending: pending, fetchedAt: Date())
    }

    #expect(status(state: "MERGED").overall == "merged")
    #expect(status(state: "CLOSED").overall == "closed")
    #expect(status(draft: true).overall == "draft")
    #expect(status(mergeable: "CONFLICTING").overall == "conflicts")
    #expect(status(failed: 2).overall == "checks failing")
    #expect(status(decision: "CHANGES_REQUESTED").overall == "changes requested")
    #expect(status(pending: 1).overall == "checks pending")
    #expect(status(decision: "REVIEW_REQUIRED").overall == "needs review")
    #expect(status(mergeState: "BLOCKED").overall == "blocked")
    #expect(status(decision: "APPROVED", mergeable: "MERGEABLE").overall == "ready")
    // Merged wins over everything else the payload still carries.
    #expect(status(state: "MERGED", mergeable: "CONFLICTING", failed: 3).overall == "merged")
}

@Test func reviewThreadsParsing() {
    let json = """
        {"data": {"repository": {"pullRequest": {"reviewThreads": {
          "totalCount": 5,
          "nodes": [
            {"isResolved": true}, {"isResolved": true}, {"isResolved": true},
            {"isResolved": false}, {"isResolved": false}
          ]}}}}}
        """
    let threads = PullRequestMonitor.parseThreads(Data(json.utf8))
    #expect(threads?.resolved == 3)
    #expect(threads?.total == 5)
    #expect(PullRequestMonitor.parseThreads(Data("{}".utf8)) == nil)
}

@Test func prCoordinatesParsing() {
    let coords = PullRequestMonitor.prCoordinates(url: "https://github.com/anaralabs/anara/pull/4570")
    #expect(coords?.owner == "anaralabs")
    #expect(coords?.repo == "anara")
    #expect(coords?.number == 4570)
    #expect(PullRequestMonitor.prCoordinates(url: "https://github.com/anaralabs/anara") == nil)
}

@Test func sameStatsIgnoresFetchTimestamp() {
    let a = PullRequestStatus(url: prURL, number: 1, state: "OPEN", fetchedAt: Date())
    var b = a
    b.fetchedAt = Date().addingTimeInterval(60)
    #expect(a.sameStats(as: b))
    b.additions = 10
    #expect(!a.sameStats(as: b))
}
