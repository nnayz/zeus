import Foundation
import Testing

@testable import DirijorDetection

/// End-to-end manifest evaluation against realistic Claude/Codex screens.
@Suite struct GoldenScreenTests {
    let engine: ManifestEngine

    init() throws {
        engine = try ManifestEngine()
    }

    private func snap(_ lines: [String], title: String? = nil, progress: Int? = nil) -> ScreenSnapshot {
        ScreenSnapshot(lines: lines, oscTitle: title, oscProgressState: progress,
                       contentSeq: 1, cols: 100, rows: 30)
    }

    // MARK: Claude

    @Test func claudeIdlePromptBox() {
        let s = snap([
            "Done. Anything else?",
            "╭────────────────────────────────────────────╮",
            "│ ❯                                          │",
            "╰────────────────────────────────────────────╯",
        ])
        let obs = engine.evaluate(s, manifestID: "claude-code")
        #expect(obs?.state == .idle)
        #expect(obs?.matchedRuleID == "idle-prompt-box")
    }

    @Test func claudePermissionDialog() {
        let s = snap([
            "╭────────────────────────────────────────────╮",
            "│ Bash command                               │",
            "│                                            │",
            "│ rm -rf build                               │",
            "│                                            │",
            "│ Do you want to proceed?                    │",
            "│ ❯ 1. Yes                                   │",
            "│   2. No, and tell Claude what to do (esc)  │",
            "╰────────────────────────────────────────────╯",
            "esc to cancel",
        ])
        let obs = try! #require(engine.evaluate(s, manifestID: "claude-code"))
        #expect(obs.state == .blockedPermission)
        #expect(obs.options == ["Yes", "No, and tell Claude what to do (esc)"])
        #expect(obs.promptExcerpt?.contains("rm -rf build") == true)
    }

    @Test func claudeWorkingBrailleTitle() {
        // U+2839 (⠹) is within the braille range.
        let s = snap(["thinking..."], title: "⠹ Waddling…")
        let obs = engine.evaluate(s, manifestID: "claude-code")
        #expect(obs?.state == .working)
        #expect(obs?.matchedRuleID == "working-spinner")
    }

    @Test func claudeTranscriptViewerSkips() {
        let s = snap([
            "Showing detailed transcript · ctrl+r to toggle",
            "╭────────────╮",
            "│ ❯          │",
            "╰────────────╯",
        ])
        let obs = engine.evaluate(s, manifestID: "claude-code")
        #expect(obs?.state == .skip)
        #expect(obs?.priority == 1200)
    }

    @Test func claudeIdleProgressZero() {
        let s = snap(["no box here"], progress: 0)
        let obs = engine.evaluate(s, manifestID: "claude-code")
        #expect(obs?.state == .idle)
        #expect(obs?.matchedRuleID == "idle-progress-zero")
    }

    // MARK: Codex

    @Test func codexActionRequiredTitle() {
        let s = snap([
            "running command…",
            "npm install",
        ], title: "● Action Required")
        let obs = try! #require(engine.evaluate(s, manifestID: "codex"))
        #expect(obs.state == .blockedPermission)
        #expect(obs.matchedRuleID == "action-required-title")
        #expect(obs.promptExcerpt?.contains("npm install") == true)
    }

    @Test func codexConfirmPrompt() {
        let s = snap([
            "╭─ Allow command? ─────────────╮",
            "│ npm install                  │",
            "│ ❯ 1. Yes                     │",
            "│   2. No                      │",
            "╰──────────────────────────────╯",
            "Press enter to confirm or esc to cancel",
        ])
        let obs = try! #require(engine.evaluate(s, manifestID: "codex"))
        #expect(obs.state == .blockedPermission)
        #expect(obs.options == ["Yes", "No"])
    }

    @Test func codexIdlePrompt() {
        let s = snap([
            "╭──────────────────────────────╮",
            "│ › Ask Codex to do something  │",
            "╰──────────────────────────────╯",
        ])
        let obs = engine.evaluate(s, manifestID: "codex")
        #expect(obs?.state == .idle)
        #expect(obs?.matchedRuleID == "idle-prompt-box")
    }

    // MARK: Cursor

    @Test func cursorConfirmDialog() {
        let s = snap([
            "╭──────────────────────────────╮",
            "│ Run this command?            │",
            "│ npm install                  │",
            "│ Run (y)   Reject (esc/n)     │",
            "╰──────────────────────────────╯",
        ])
        let obs = try! #require(engine.evaluate(s, manifestID: "cursor"))
        #expect(obs.state == .blockedPermission)
        #expect(obs.matchedRuleID == "confirm-dialog")
        #expect(obs.promptExcerpt?.contains("npm install") == true)
    }

    @Test func cursorWorkingStatusLine() {
        let s = snap([
            "some earlier output",
            "Generating",
        ])
        let obs = engine.evaluate(s, manifestID: "cursor")
        #expect(obs?.state == .working)
        #expect(obs?.matchedRuleID == "working-status-line")
    }

    @Test func cursorIdlePrompt() {
        let s = snap([
            "╭──────────────────────────────╮",
            "│ → Add a follow-up            │",
            "╰──────────────────────────────╯",
        ])
        let obs = engine.evaluate(s, manifestID: "cursor")
        #expect(obs?.state == .idle)
    }

    // MARK: Gemini

    @Test func geminiConfirmDialog() {
        let s = snap([
            "╭──────────────────────────────────────╮",
            "│ Apply this change?                   │",
            "│ ● 1. Yes, allow once                 │",
            "│   2. Yes, allow always               │",
            "│   3. No, suggest changes (esc)       │",
            "╰──────────────────────────────────────╯",
        ])
        let obs = try! #require(engine.evaluate(s, manifestID: "gemini"))
        #expect(obs.state == .blockedPermission)
        #expect(obs.matchedRuleID == "confirm-dialog")
    }

    @Test func geminiWorkingCancelTimer() {
        let s = snap([
            "⠹ Polishing the code (esc to cancel, 12s)",
        ])
        let obs = engine.evaluate(s, manifestID: "gemini")
        #expect(obs?.state == .working)
        #expect(obs?.matchedRuleID == "working-cancel-timer")
    }

    @Test func geminiIdlePrompt() {
        let s = snap([
            "╭──────────────────────────────────────╮",
            "│ >   Type your message or @path/to/file │",
            "╰──────────────────────────────────────╯",
        ])
        let obs = engine.evaluate(s, manifestID: "gemini")
        #expect(obs?.state == .idle)
    }

    @Test func unknownManifestReturnsNil() {
        #expect(engine.evaluate(snap(["x"]), manifestID: "nope") == nil)
    }
}
