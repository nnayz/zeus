import DirijorCore
import Foundation
import Testing

@testable import DirijorDetection

@Suite struct ReducerTests {
    let t0 = Date(timeIntervalSince1970: 1_000_000)
    func at(_ dt: TimeInterval) -> Date { t0.addingTimeInterval(dt) }

    private func idleScreen(_ seq: UInt64) -> StatusSignal {
        .screen(ScreenObservation(state: .idle, matchedRuleID: "idle", priority: 500, contentSeq: seq))
    }
    private func workingScreen(_ seq: UInt64) -> StatusSignal {
        .screen(ScreenObservation(state: .working, matchedRuleID: "work", priority: 900, contentSeq: seq))
    }
    private func blocker(_ kind: ManifestState, _ seq: UInt64, excerpt: String? = nil, options: [String]? = nil) -> StatusSignal {
        .screen(ScreenObservation(state: kind, matchedRuleID: "blk", priority: 1000, contentSeq: seq,
                                  promptExcerpt: excerpt, options: options))
    }

    // MARK: Process exit

    @Test func processExitSignaledVsExited() {
        var r = StatusReducer(authority: .hooksPrimary, spawnedAt: t0)
        let o = r.reduce(.processExit(code: nil, signal: 9), now: t0)
        #expect(o.statusChange == .exited(ExitInfo(reason: .signaled, code: nil, signal: 9)))
        #expect(r.status == .exited(ExitInfo(reason: .signaled, code: nil, signal: 9)))

        var r2 = StatusReducer(authority: .hooksPrimary, spawnedAt: t0)
        let o2 = r2.reduce(.processExit(code: 0, signal: nil), now: t0)
        #expect(o2.statusChange == .exited(ExitInfo(reason: .exited, code: 0, signal: nil)))
    }

    @Test func exitedIsAbsorbing() {
        var r = StatusReducer(authority: .hooksPrimary, spawnedAt: t0)
        _ = r.reduce(.processExit(code: 0, signal: nil), now: t0)
        let o = r.reduce(.claudeHook(.userPromptSubmit(promptText: "hi"), isSubagent: false), now: at(1))
        #expect(o.statusChange == nil)
        if case .exited = r.status {} else { Issue.record("expected still exited") }
    }

    // MARK: processOnly

    @Test func processOnlyOutputThenExit() {
        var r = StatusReducer(authority: .processOnly, spawnedAt: t0)
        #expect(r.status == .starting)
        let o1 = r.reduce(.ptyOutputActivity, now: t0)
        #expect(o1.statusChange == .working)
        let o2 = r.reduce(.ptyOutputActivity, now: at(1))
        #expect(o2.statusChange == nil)  // already working
        // Screen observations are ignored in processOnly.
        let o3 = r.reduce(idleScreen(1), now: at(2))
        #expect(o3.statusChange == nil)
        #expect(r.status == .working)
        let o4 = r.reduce(.processExit(code: 0, signal: nil), now: at(3))
        #expect(o4.statusChange == .exited(ExitInfo(reason: .exited, code: 0, signal: nil)))
    }

    // MARK: Normal Claude turn

    @Test func normalClaudeTurnCompletesOnce() {
        var r = StatusReducer(authority: .hooksPrimary, spawnedAt: t0)
        #expect(r.reduce(.claudeHook(.sessionStart(source: "startup", agentSessionID: nil, transcriptPath: nil), isSubagent: false), now: t0).statusChange == .idle)
        #expect(r.reduce(.claudeHook(.userPromptSubmit(promptText: "do it"), isSubagent: false), now: at(1)).statusChange == .working)
        #expect(r.reduce(.claudeHook(.preToolUse(toolName: "Bash", toolInputSummary: "ls"), isSubagent: false), now: at(2)).statusChange == nil)
        #expect(r.reduce(.claudeHook(.preToolUse(toolName: "Read", toolInputSummary: "a.txt"), isSubagent: false), now: at(3)).statusChange == nil)
        #expect(r.status == .working)

        // Stop is a strong idle candidate — held until one confirmation.
        let oStop = r.reduce(.claudeHook(.stop, isSubagent: false), now: at(4))
        #expect(oStop.statusChange == nil)
        #expect(r.status == .working)
        #expect(oStop.turnCompleted == false)

        // Screen idle confirms → commit idle with turnCompleted exactly once.
        let oIdle = r.reduce(idleScreen(1), now: at(4.1))
        #expect(oIdle.statusChange == .idle)
        #expect(oIdle.turnCompleted == true)

        // A further idle screen does not re-fire turnCompleted.
        let oIdle2 = r.reduce(idleScreen(2), now: at(4.2))
        #expect(oIdle2.turnCompleted == false)
        #expect(oIdle2.statusChange == nil)
    }

    @Test func stopThenTickConfirmsIdle() {
        var r = StatusReducer(authority: .hooksPrimary, spawnedAt: t0)
        _ = r.reduce(.claudeHook(.userPromptSubmit(promptText: "x"), isSubagent: false), now: t0)
        _ = r.reduce(.claudeHook(.stop, isSubagent: false), now: at(1))
        #expect(r.status == .working)
        let o = r.reduce(.tick, now: at(1.1))
        #expect(o.statusChange == .idle)
        #expect(o.turnCompleted == true)
    }

    // MARK: Permission flow

    @Test func permissionFlowWithDetail() {
        var r = StatusReducer(authority: .hooksPrimary, spawnedAt: t0)
        _ = r.reduce(.claudeHook(.sessionStart(source: "startup", agentSessionID: nil, transcriptPath: nil), isSubagent: false), now: t0)
        _ = r.reduce(.claudeHook(.userPromptSubmit(promptText: "clean"), isSubagent: false), now: at(1))
        _ = r.reduce(.claudeHook(.preToolUse(toolName: "Bash", toolInputSummary: "echo hi"), isSubagent: false), now: at(2))

        // permissionRequest hook → needsInput with rich detail.
        let oPerm = r.reduce(.claudeHook(.permissionRequest(toolName: "Bash", toolInputSummary: "rm -rf build"), isSubagent: false), now: at(3))
        #expect(oPerm.statusChange == .needsInput(.permission))
        let d = try! #require(oPerm.needsInput)
        #expect(d.toolName == "Bash")
        #expect(d.source == .claudePermissionHook)
        #expect(d.summary.contains("rm -rf build"))
        #expect(d.riskHint == .destructive)

        // Screen blocker corroborates (source flips to screenScrape, options captured).
        let oScreen = r.reduce(blocker(.blockedPermission, 1, excerpt: "Bash command\nrm -rf build", options: ["Yes", "No"]), now: at(3.1))
        #expect(r.status == .needsInput(.permission))
        #expect(oScreen.needsInput?.source == .screenScrape)
        #expect(oScreen.needsInput?.options == ["Yes", "No"])

        // User starts typing — no state change.
        let oKey = r.reduce(.userKeystroke, now: at(4))
        #expect(oKey.statusChange == nil)
        #expect(r.status == .needsInput(.permission))

        // Claude proceeds (PreToolUse) → the hook clears the stale screen blocker.
        let oResume = r.reduce(.claudeHook(.preToolUse(toolName: "Bash", toolInputSummary: "rm -rf build"), isSubagent: false), now: at(5))
        #expect(oResume.statusChange == .working)
        #expect(r.status == .working)
    }

    @Test func notificationQuestionNeedsInput() {
        var r = StatusReducer(authority: .hooksPrimary, spawnedAt: t0)
        _ = r.reduce(.claudeHook(.userPromptSubmit(promptText: "x"), isSubagent: false), now: t0)
        let o = r.reduce(.claudeHook(.notification(notificationType: "agent_needs_input", message: "Which file?"), isSubagent: false), now: at(1))
        #expect(o.statusChange == .needsInput(.question))
        #expect(o.needsInput?.summary == "Which file?")
        #expect(o.needsInput?.source == .claudeNotificationHook)
    }

    // MARK: Subagent gotcha

    @Test func subagentStopDoesNotIdle() {
        var r = StatusReducer(authority: .hooksPrimary, spawnedAt: t0)
        _ = r.reduce(.claudeHook(.userPromptSubmit(promptText: "x"), isSubagent: false), now: t0)
        #expect(r.status == .working)
        _ = r.reduce(.claudeHook(.subagentStart(agentID: "a1"), isSubagent: true), now: at(1))
        let oStop = r.reduce(.claudeHook(.subagentStop(agentID: "a1"), isSubagent: true), now: at(2))
        #expect(oStop.statusChange == nil)
        #expect(r.status == .working)  // subagent stop must NOT idle the parent

        // A Stop tagged as subagent is also bookkeeping only.
        let oSubStop = r.reduce(.claudeHook(.stop, isSubagent: true), now: at(3))
        #expect(oSubStop.statusChange == nil)
        #expect(r.status == .working)

        // Only the parent Stop drives idle.
        _ = r.reduce(.claudeHook(.stop, isSubagent: false), now: at(4))
        let oIdle = r.reduce(idleScreen(1), now: at(4.1))
        #expect(oIdle.statusChange == .idle)
        #expect(oIdle.turnCompleted == true)
    }

    // MARK: Codex flow

    @Test func codexTurnFlow() {
        var r = StatusReducer(authority: .screenPrimary, spawnedAt: t0)
        // Screen working is definitive for codex — ends grace.
        #expect(r.reduce(workingScreen(1), now: t0).statusChange == .working)
        // Action Required blocker.
        let oBlk = r.reduce(blocker(.blockedPermission, 2, excerpt: "allow?"), now: at(1))
        #expect(oBlk.statusChange == .needsInput(.permission))
        // Codex resumes: two non-blocker scans clear the blocker.
        #expect(r.reduce(workingScreen(3), now: at(2)).statusChange == nil)  // miss 1, held
        #expect(r.status == .needsInput(.permission))
        #expect(r.reduce(workingScreen(4), now: at(3)).statusChange == .working)  // miss 2, cleared
        // Turn complete + idle screen → idle with turnCompleted.
        _ = r.reduce(.codexTurnComplete(lastAssistantMessage: "done"), now: at(4))
        #expect(r.status == .working)
        let oIdle = r.reduce(idleScreen(5), now: at(5))
        #expect(oIdle.statusChange == .idle)
        #expect(oIdle.turnCompleted == true)
    }

    // MARK: Anti-flicker

    @Test func antiFlickerRequiresThreeIdleScans() {
        var r = StatusReducer(authority: .hooksPrimary, spawnedAt: t0)
        _ = r.reduce(.claudeHook(.userPromptSubmit(promptText: "x"), isSubagent: false), now: t0)
        // Single spurious idle scan while a turn is in flight does not commit.
        #expect(r.reduce(idleScreen(1), now: at(0.1)).statusChange == nil)
        #expect(r.status == .working)
        #expect(r.reduce(idleScreen(2), now: at(0.2)).statusChange == nil)
        #expect(r.status == .working)
        // Third consecutive idle commits (no stop → no turnCompleted).
        let o = r.reduce(idleScreen(3), now: at(0.3))
        #expect(o.statusChange == .idle)
        #expect(o.turnCompleted == false)
    }

    @Test func workingSignalCancelsIdleCandidacy() {
        var r = StatusReducer(authority: .hooksPrimary, spawnedAt: t0)
        _ = r.reduce(.claudeHook(.userPromptSubmit(promptText: "x"), isSubagent: false), now: t0)
        _ = r.reduce(idleScreen(1), now: at(0.1))  // candidate, 1 confirm
        _ = r.reduce(idleScreen(2), now: at(0.2))  // 2 confirms
        // A work hook resets candidacy.
        _ = r.reduce(.claudeHook(.preToolUse(toolName: "Bash", toolInputSummary: "x"), isSubagent: false), now: at(0.25))
        _ = r.reduce(idleScreen(3), now: at(0.3))  // only 1 confirm again → still working
        #expect(r.status == .working)
    }

    // MARK: Screen blocker clearing (screen-primary)

    @Test func screenBlockerClearsAfterTwoMisses() {
        var r = StatusReducer(authority: .screenPrimary, spawnedAt: t0)
        _ = r.reduce(workingScreen(1), now: t0)
        _ = r.reduce(blocker(.blockedQuestion, 2, excerpt: "pick one"), now: at(1))
        #expect(r.status == .needsInput(.question))
        #expect(r.reduce(idleScreen(3), now: at(2)).statusChange == nil)  // miss 1
        let o = r.reduce(idleScreen(4), now: at(3))                        // miss 2 → clear to idle
        #expect(o.statusChange == .idle)
    }

    @Test func visibleBlockerBeatsWorkingHook() {
        // A screen blocker overrides an idle/working belief immediately.
        var r = StatusReducer(authority: .hooksPrimary, spawnedAt: t0)
        _ = r.reduce(.claudeHook(.userPromptSubmit(promptText: "x"), isSubagent: false), now: t0)
        let o = r.reduce(blocker(.blockedPermission, 1, excerpt: "Do you want to proceed?"), now: at(1))
        #expect(o.statusChange == .needsInput(.permission))
        #expect(o.needsInput?.summary == "Do you want to proceed?")
    }

    // MARK: Staleness

    @Test func stalenessToUnknown() {
        var r = StatusReducer(authority: .hooksPrimary, spawnedAt: t0)
        _ = r.reduce(.claudeHook(.userPromptSubmit(promptText: "x"), isSubagent: false), now: t0)
        // A tick well past the staleness timeout with no signals → unknown.
        let o = r.reduce(.tick, now: at(61))
        #expect(o.statusChange == .unknown)
        #expect(r.status == .unknown)
    }

    @Test func ptyActivityRefreshesRecency() {
        var r = StatusReducer(authority: .hooksPrimary, spawnedAt: t0)
        _ = r.reduce(.claudeHook(.userPromptSubmit(promptText: "x"), isSubagent: false), now: t0)
        // Output at t+40 refreshes recency; tick at t+61 is only 21s stale.
        _ = r.reduce(.ptyOutputActivity, now: at(40))
        let o = r.reduce(.tick, now: at(61))
        #expect(o.statusChange == nil)
        #expect(r.status == .working)
    }

    // MARK: Startup grace

    @Test func startupGraceHoldsStartingForScreenPrimaryIdle() {
        var r = StatusReducer(authority: .screenPrimary, spawnedAt: t0)
        // An idle screen within the grace window does not leave starting.
        let o = r.reduce(idleScreen(1), now: at(0.5))
        #expect(o.statusChange == nil)
        #expect(r.status == .starting)
        // After grace, idle screen commits idle.
        let o2 = r.reduce(idleScreen(2), now: at(3.5))
        #expect(o2.statusChange == .idle)
    }
}
