import DirijorCore
import Foundation
import Testing

@testable import DirijorDetection

@Test func claudeTrustDialogIsQuestionBlocker() throws {
    let lines = [
        "────────────────────────────────────────────────",
        " Accessing workspace:",
        " /private/tmp/scratchpad",
        " Quick safety check: Is this a project you created or one you trust?",
        " Claude Code'll be able to read, edit, and execute files here.",
        " Security guide",
        " ❯ 1. Yes, I trust this folder",
        "   2. No, exit",
        " Enter to confirm · Esc to cancel",
    ]
    let snap = ScreenSnapshot(lines: lines, contentSeq: 1, cols: 80, rows: 24)
    let engine = try ManifestEngine()
    let obs = engine.evaluate(snap, manifestID: "claude-code")
    #expect(obs != nil)
    #expect(obs?.state == .blockedQuestion)
}
