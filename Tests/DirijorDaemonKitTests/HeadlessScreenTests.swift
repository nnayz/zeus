import Foundation
import DirijorProtocol
import Testing

@testable import DirijorDaemonKit

@Test func sanitizeReplacesNulAndControlWithSpace() {
    // SwiftTerm fills blank cells with NUL; detection must see spaces so
    // substring / line-regex rules match like a real terminal.
    let raw = "\u{0}❯ 1.\u{0}Yes,\u{0}I\u{0}trust\u{0}this\u{0}folder"
    let clean = HeadlessScreen.sanitize(raw)
    #expect(!clean.unicodeScalars.contains { $0.value < 0x20 && $0 != "\t" })
    #expect(clean.contains("❯ 1. Yes, I trust this folder"))
    // Tabs are preserved; plain text is returned unchanged.
    #expect(HeadlessScreen.sanitize("a\tb") == "a\tb")
    #expect(HeadlessScreen.sanitize("no controls here") == "no controls here")
}

@Test func mouseWheelSilentWhenMouseModeOff() {
    // A bare shell never enabled mouse tracking → scrolling must emit nothing,
    // so no escape codes leak onto the prompt.
    let screen = HeadlessScreen(cols: 80, rows: 24)
    #expect(screen.mouseWheel(up: true, lines: 3, col: 10, row: 5).isEmpty)
}

@Test func mouseWheelEmitsSGRWhenTracking() {
    // Enable SGR mouse tracking (1000 = button events, 1006 = SGR encoding),
    // exactly what Claude Code / vim / less turn on.
    let screen = HeadlessScreen(cols: 80, rows: 24)
    _ = screen.feed(Data("\u{1B}[?1000h\u{1B}[?1006h".utf8))

    let up = screen.mouseWheel(up: true, lines: 2, col: 10, row: 5)
    let s = String(decoding: up, as: UTF8.self)
    // Two wheel-up notches: SGR button 64 at 1-based col;row, press ('M').
    #expect(s == "\u{1B}[<64;11;6M\u{1B}[<64;11;6M")

    let down = screen.mouseWheel(up: false, lines: 1, col: 0, row: 0)
    #expect(String(decoding: down, as: UTF8.self) == "\u{1B}[<65;1;1M")
}

@Test func fullSnapshotReturnsCurrentScreenWithoutDisturbingDiffBaseline() {
    // The re-seed a sink gets after falling behind must carry the CURRENT screen
    // yet not perturb the diff stream other sinks rely on.
    let screen = HeadlessScreen(cols: 40, rows: 5)
    _ = screen.feed(Data("hello".utf8))
    _ = screen.gridUpdate(full: false)           // establishes the diff baseline

    _ = screen.feed(Data("\r\nworld".utf8))       // new content, not yet diffed out

    let snap = screen.fullSnapshot()
    #expect(snap.isFullSnapshot)
    let snapText = snap.changedRows.map { rowString($0.cells) }.joined(separator: "\n")
    #expect(snapText.contains("hello"))
    #expect(snapText.contains("world"))

    // The snapshot must NOT have advanced the diff baseline: the next diff still
    // reports "world" as a change. (If fullSnapshot mutated lastGrid, this diff
    // would come back empty and a real sink would miss the row.)
    let diff = screen.gridUpdate(full: false)
    let diffText = diff.changedRows.map { rowString($0.cells) }.joined(separator: "\n")
    #expect(diffText.contains("world"))
}

@Test func spaceOverBlankMovesCursorWithoutChangingRows() {
    // Blank cells are normalized to spaces (scalar 32), so typing a space over
    // an empty prompt changes NO row content — only the cursor moves. The
    // emitter (AgentSession.flushGrid) must therefore treat a cursor move as
    // reason to broadcast, or the caret freezes on space/arrow keys.
    let screen = HeadlessScreen(cols: 40, rows: 5)
    _ = screen.gridUpdate(full: false)   // establish the diff baseline

    _ = screen.feed(Data(" ".utf8))      // echoed space at a blank prompt
    let diff = screen.gridUpdate(full: false)
    #expect(diff.changedRows.isEmpty)    // the trap: content is "unchanged"
    #expect(diff.cursorCol == 1)         // …but the cursor DID move
}

@Test func cursorVisibilityTracksDECTCM() {
    // TUIs hide the terminal cursor (CSI ?25l) while painting their own caret;
    // the grid must carry that so clients don't draw a stray block.
    let screen = HeadlessScreen(cols: 40, rows: 5)
    #expect(screen.gridUpdate(full: false).cursorVisible)

    _ = screen.feed(Data("\u{1B}[?25l".utf8))
    #expect(!screen.gridUpdate(full: false).cursorVisible)
    #expect(!screen.fullSnapshot().cursorVisible)

    _ = screen.feed(Data("\u{1B}[?25h".utf8))
    #expect(screen.gridUpdate(full: false).cursorVisible)
}

private func rowString(_ cells: [GridCell]) -> String {
    String(String.UnicodeScalarView(cells.map {
        Unicode.Scalar($0.scalar == 0 ? 32 : $0.scalar) ?? " "
    }))
}

// MARK: - Scrollback read (find-in-scrollback + local viewport)

@Test func wideHeadlessScrollbackStaysWithinItsCellMemoryBudget() {
    let cols = 480
    let rows = 66
    let screen = HeadlessScreen(cols: cols, rows: rows)
    let line = String(repeating: "x", count: 400)
    _ = screen.feed(Data((0..<400).map { _ in line }.joined(separator: "\r\n").utf8))

    // At this width every SwiftTerm row is about 11 KiB (480 CharData cells
    // at 24 bytes each). Keeping 2,000 rows would cost ~22 MiB per session.
    // The headless emulator gets a ~1 MiB history-cell budget instead.
    #expect(screen.scrollback().lines.count <= rows + 100)

    // A later widen must recompute the line limit; otherwise history created
    // at a narrow startup geometry balloons when every retained row is reflowed.
    let resized = HeadlessScreen(cols: 80, rows: rows)
    _ = resized.feed(Data((0..<400).map { _ in "narrow-line" }.joined(separator: "\r\n").utf8))
    resized.resize(cols: cols, rows: rows)
    #expect(resized.scrollback().lines.count <= rows + 100)
}

@Test func interactiveResizeDefersHistoryGrowthButNeverTheTrim() {
    let rows = 24
    // Widening lowers the line budget, so even a mid-drag step must apply it —
    // otherwise a drag from narrow to wide reflows a narrow-width history into
    // wide rows and blows past the per-session cell budget for its duration.
    let widening = HeadlessScreen(cols: 80, rows: rows)
    _ = widening.feed(Data((0..<400).map { _ in "narrow-line" }.joined(separator: "\r\n").utf8))
    widening.resize(cols: 480, rows: rows, historyBudget: .trimOnly)
    #expect(widening.scrollback().lines.count <= rows + 100)

    // Narrowing raises the budget; that is an allocation with no memory
    // pressure behind it, so a drag step defers it and the settle pass applies
    // it once the geometry stops moving.
    let narrowing = HeadlessScreen(cols: 480, rows: rows)
    _ = narrowing.feed(Data((0..<400).map { _ in "wide-line" }.joined(separator: "\r\n").utf8))
    narrowing.resize(cols: 80, rows: rows, historyBudget: .trimOnly)
    let deferred = narrowing.scrollback().lines.count
    _ = narrowing.feed(Data((0..<400).map { _ in "more" }.joined(separator: "\r\n").utf8))
    #expect(narrowing.scrollback().lines.count <= deferred + rows)

    narrowing.rebudgetHistory()
    _ = narrowing.feed(Data((0..<400).map { _ in "more" }.joined(separator: "\r\n").utf8))
    #expect(narrowing.scrollback().lines.count > deferred + rows)
}

@Test func headlessScreenIgnoresRendererOnlySynchronizedOutputMode() {
    let screen = HeadlessScreen(cols: 80, rows: 24)
    let before = screen.contentSeq
    // DEC private mode 2026 is a renderer anti-tearing hint. HeadlessScreen
    // reads the live emulation buffer and never displays SwiftTerm's frozen
    // buffer, so honoring it would only deep-copy every retained cell.
    _ = screen.feed(Data("\u{1B}[?2026h".utf8))
    #expect(screen.contentSeq == before)

    // The integration stays correct when the sequence straddles PTY reads.
    _ = screen.feed(Data("\u{1B}[?20".utf8))
    _ = screen.feed(Data("26l".utf8))
    #expect(screen.contentSeq == before)
}

@Test func headlessInputFilterHandlesSplitAndCombinedPrivateModes() {
    var filter = HeadlessTerminalInputFilter()

    // The sequence may straddle arbitrary PTY read boundaries.
    #expect(filter.filter(Data("before\u{1B}[?20".utf8)) == Data("before".utf8))
    #expect(filter.filter(Data("26hafter".utf8)) == Data("after".utf8))

    // Preserve every other mode in a combined DECSET/DECRST sequence.
    let combined = filter.filter(Data("\u{1B}[?25;2026;1000l".utf8))
    #expect(combined == Data("\u{1B}[?25;1000l".utf8))

    // Unrelated CSI controls pass byte-for-byte.
    let unrelated = Data("\u{1B}[31mred\u{1B}[0m".utf8)
    #expect(filter.filter(unrelated) == unrelated)
    // 0x9B is also a valid UTF-8 continuation byte; never mistake it for a
    // single-byte C1 CSI while filtering the raw PTY stream.
    let utf8Containing9B = Data([0xC2, 0x9B])
    #expect(filter.filter(utf8Containing9B) == utf8Containing9B)
}

@Test func scrollbackWalksHistoryAndComputesRowArithmetic() {
    // Feed more lines than the screen is tall so history spills into scrollback.
    let rows = 5
    let screen = HeadlessScreen(cols: 20, rows: rows)
    let text = (1...12).map { "line-\($0)" }.joined(separator: "\r\n")
    _ = screen.feed(Data(text.utf8))

    let sb = screen.scrollback()
    #expect(sb.lines.count == 12)
    #expect(sb.lines.first?.hasPrefix("line-1") == true)
    #expect(sb.lines.last?.hasPrefix("line-12") == true)
    // Early in a session the buffer hasn't trimmed, so the origin is 0.
    #expect(sb.firstRow == 0)
    #expect(sb.rows == rows)
    #expect(!sb.isAltScreen)
    // Unscrolled invariant: the live screen top sits `lines.count - rows` below
    // the first scrollback row.
    #expect(sb.visibleStartRow - sb.firstRow == sb.lines.count - rows)
    // That row is the top of the visible screen (last `rows` lines are visible).
    let topVisible = sb.lines[sb.visibleStartRow - sb.firstRow]
    #expect(topVisible.hasPrefix("line-8"))
}

@Test func scrollbackIsAltScreenFlipsAfterAltEnter() {
    let screen = HeadlessScreen(cols: 20, rows: 5)
    _ = screen.feed(Data("before\r\n".utf8))
    #expect(!screen.scrollback().isAltScreen)
    // 1049h switches to the alternate buffer (which has no scrollback).
    _ = screen.feed(Data("\u{1B}[?1049h".utf8))
    #expect(screen.scrollback().isAltScreen)
}

@Test func scrollbackCellsDecodeBackToSameCellsViaSharedCodec() {
    let rows = 5
    let cols = 20
    let screen = HeadlessScreen(cols: cols, rows: rows)
    let text = (1...12).map { "line-\($0)" }.joined(separator: "\r\n")
    _ = screen.feed(Data(text.utf8))

    let sb = screen.scrollback()
    let cellsResult = screen.scrollbackCells(firstRow: sb.firstRow, maxRows: 3)
    #expect(cellsResult.firstRow == sb.firstRow)
    #expect(cellsResult.rowCount == 3)
    #expect(cellsResult.totalRows == sb.lines.count)
    #expect(cellsResult.cols == cols)
    #expect(cellsResult.liveStartRow == sb.visibleStartRow)

    // The payload decodes (shared RLE codec) to cells whose text matches the
    // plain-text scrollback for the same rows.
    let decoded = GridRowCodec.decodeRows(cellsResult.payload, rowCount: cellsResult.rowCount)
    #expect(decoded != nil)
    let asText = (decoded ?? []).map { rowString($0).trimmingCharacters(in: .whitespaces) }
    #expect(asText == ["line-1", "line-2", "line-3"])
    // Re-encoding the decoded rows reproduces the exact payload bytes.
    #expect(GridRowCodec.encodeRows(decoded ?? []) == cellsResult.payload)
}

@Test func scrollbackCellsClampsOutOfRangeRequest() {
    let screen = HeadlessScreen(cols: 20, rows: 5)
    _ = screen.feed(Data("a\r\nb\r\nc".utf8))
    let total = screen.scrollback().lines.count
    // Ask well past the end: clamps to zero rows without crashing.
    let result = screen.scrollbackCells(firstRow: total + 100, maxRows: 10)
    #expect(result.rowCount == 0)
    #expect(result.totalRows == total)
    #expect(result.payload.isEmpty)
}

/// The exact frame shape Claude Code emits per wheel-scroll step — in the ALT
/// SCREEN (Claude's scroll view runs in 1049h). Region 2..N, scroll down 1,
/// then paint the exposed row 2 with word-jumps (CHA), assuming it's blank.
@Test func altScreenRegionScrollDownBlanksExposedRow() {
    let rows = 66
    let screen = HeadlessScreen(cols: 220, rows: rows)
    // History + enter alt screen (like Claude's renderer reinit), then paint
    // numbered rows.
    _ = screen.feed(Data((1...300).map { "filler-\($0)" }.joined(separator: "\r\n").utf8))
    _ = screen.feed(Data("\u{1B}[?1049h\u{1B}[2J\u{1B}[H".utf8))
    var paint = ""
    for r in 1...rows { paint += "\u{1B}[\(r);1HROW-\(String(format: "%02d", r))" }
    _ = screen.feed(Data(paint.utf8))

    // Claude's frame: home, region, scroll down 1, release, then paint row 2
    // by jumping columns (spaces skipped — row MUST have been blanked).
    _ = screen.feed(Data("\u{1B}[H\u{1B}[2;\(rows)r\u{1B}[1T\u{1B}[r\u{1B}[H\r\u{1B}[2C\u{1B}[1Binstruction\u{1B}[15Gto".utf8))

    let lines = screenLines(screen)
    #expect(lines[0].hasPrefix("ROW-01"), "row 1 outside region: \(lines[0])")
    // Row 2 was blanked by the scroll then painted sparsely: "  instruction to"
    #expect(lines[1].trimmingCharacters(in: .whitespaces) == "instruction to",
        "exposed row must hold ONLY the sparse paint, got: \(lines[1])")
    #expect(lines[2].hasPrefix("ROW-02"), "row 3 should hold pushed-down ROW-02: \(lines[2])")
    #expect(lines[rows - 1].hasPrefix("ROW-\(rows - 1)"), "bottom: \(lines[rows - 1])")
}

// MARK: - PTY log replay harness (env-gated forensic tool, not a CI test)
//
// Replays a captured session byte log through a fresh HeadlessScreen to
// reproduce emulator-level corruption deterministically. Drive it with:
//   DIRIJOR_REPLAY_FILE=/path/bytes.bin DIRIJOR_REPLAY_SIZE=220x66 \
//   DIRIJOR_REPLAY_START=120x32 DIRIJOR_REPLAY_RESIZE_AT=2048 \
//   swift test --filter replayCapturedLog
// Dumps the final screen to <file>.screen.txt for inspection.
@Test func replayCapturedLog() throws {
    guard let path = ProcessInfo.processInfo.environment["DIRIJOR_REPLAY_FILE"] else { return }
    func parseSize(_ s: String?) -> (Int, Int)? {
        guard let s, let x = s.firstIndex(of: "x"),
            let c = Int(s[..<x]), let r = Int(s[s.index(after: x)...]) else { return nil }
        return (c, r)
    }
    let env = ProcessInfo.processInfo.environment
    let final = parseSize(env["DIRIJOR_REPLAY_SIZE"]) ?? (220, 66)
    let start = parseSize(env["DIRIJOR_REPLAY_START"]) ?? final
    let resizeAt = Int(env["DIRIJOR_REPLAY_RESIZE_AT"] ?? "") ?? 0

    let data = try Data(contentsOf: URL(fileURLWithPath: path))
    let screen = HeadlessScreen(cols: start.0, rows: start.1)
    if resizeAt > 0, start != final, data.count > resizeAt {
        _ = screen.feed(data.prefix(resizeAt))
        screen.resize(cols: final.0, rows: final.1)
        _ = screen.feed(data.dropFirst(resizeAt))
    } else {
        if start != final { screen.resize(cols: final.0, rows: final.1) }
        _ = screen.feed(data)
    }
    let text = screen.snapshot().lines.joined(separator: "\n")
    try Data(text.utf8).write(to: URL(fileURLWithPath: path + ".screen.txt"))
    print("replay complete: \(data.count) bytes -> \(path).screen.txt")
}

// MARK: - Region scroll emulation (Claude Code's wheel-scroll recipe)
//
// Claude Code scrolls its view with DECSTBM (ESC[2;Nr) + CSI S / CSI T and then
// repaints DIFFERENTIALLY, skipping cells it believes unchanged (CSI C). If the
// emulator's scroll is off by even one row, every skipped cell keeps stale
// content and rows become character-interleaved garbage — the exact corruption
// seen live. These tests pin the emulator's region scrolls, crucially on a FULL
// scrollback buffer (the live failure had MBs of history; a full CircularList
// recycles storage and is where splice math goes wrong).

private func screenLines(_ screen: HeadlessScreen) -> [String] {
    screen.snapshot().lines
}

/// Fill the screen with numbered rows (row 1 at top) after flooding enough
/// output to saturate the scrollback (200 lines + 24 rows).
private func makeScrolledScreen(cols: Int = 40, rows: Int = 24) -> HeadlessScreen {
    let screen = HeadlessScreen(cols: cols, rows: rows)
    _ = screen.feed(Data((1...400).map { "filler-\($0)" }.joined(separator: "\r\n").utf8))
    // Now paint a known screen: cursor home, clear, numbered rows.
    var paint = "\u{1B}[H\u{1B}[2J"
    for r in 1...rows { paint += "\u{1B}[\(r);1HROW-\(String(format: "%02d", r))" }
    _ = screen.feed(Data(paint.utf8))
    return screen
}

@Test func regionScrollUpOnFullScrollbackShiftsExactly() {
    let rows = 24
    let screen = makeScrolledScreen(rows: rows)
    // Claude's recipe: region rows 2..24, scroll up 3, release region.
    _ = screen.feed(Data("\u{1B}[2;\(rows)r\u{1B}[3S\u{1B}[r".utf8))

    let lines = screenLines(screen)
    #expect(lines[0].hasPrefix("ROW-01"))          // outside region: untouched
    #expect(lines[1].hasPrefix("ROW-05"))          // region top now shows row 5
    #expect(lines[rows - 4].hasPrefix("ROW-24"))   // old bottom moved up 3
    // The 3 exposed bottom rows must be blank.
    for y in (rows - 3)..<rows {
        #expect(lines[y].trimmingCharacters(in: .whitespaces).isEmpty, "row \(y) should be blank, got: \(lines[y])")
    }
}

@Test func regionScrollDownOnFullScrollbackShiftsExactly() {
    let rows = 24
    let screen = makeScrolledScreen(rows: rows)
    _ = screen.feed(Data("\u{1B}[2;\(rows)r\u{1B}[3T\u{1B}[r".utf8))

    let lines = screenLines(screen)
    #expect(lines[0].hasPrefix("ROW-01"))          // outside region: untouched
    // The 3 exposed top rows of the region must be blank.
    for y in 1...3 {
        #expect(lines[y].trimmingCharacters(in: .whitespaces).isEmpty, "row \(y) should be blank, got: \(lines[y])")
    }
    #expect(lines[4].hasPrefix("ROW-02"))          // old region top pushed down 3
    #expect(lines[rows - 1].hasPrefix("ROW-21"))   // bottom shows row 21
}

/// THE live corruption (reproduced): SwiftTerm's Buffer.resize clamps
/// marginRight when shrinking but never re-grows it when the terminal WIDENS,
/// and cmdScrollDown (CSI T) unconditionally scrolls only marginLeft...
/// marginRight. So after a widen, CSI T scrolls just the left columns; the
/// right side keeps stale rows, and Claude's differential repaint (which
/// assumes the whole width scrolled) interleaves old and new text.
@Test func scrollDownAfterWidenScrollsFullWidth() {
    let rows = 24
    let screen = HeadlessScreen(cols: 40, rows: rows)
    _ = screen.feed(Data((1...300).map { "filler-\($0)" }.joined(separator: "\r\n").utf8))

    // Widen 40 → 80 (window resize / sidebar toggle), then paint rows that
    // extend PAST the old width, with a marker at column 60.
    screen.resize(cols: 80, rows: rows)
    var paint = "\u{1B}[H\u{1B}[2J"
    for r in 1...rows {
        paint += "\u{1B}[\(r);1HROW-\(String(format: "%02d", r))"
        paint += "\u{1B}[\(r);60HMARK-\(String(format: "%02d", r))"
    }
    _ = screen.feed(Data(paint.utf8))

    // Claude's wheel-up recipe: region 2..24, scroll down 3, release.
    _ = screen.feed(Data("\u{1B}[2;\(rows)r\u{1B}[3T\u{1B}[r".utf8))

    let lines = screenLines(screen)
    // Row 5 (index 4) must now hold ALL of old row 2 — including the marker
    // beyond the old 40-col width. With the stuck margin it keeps MARK-05.
    #expect(lines[4].hasPrefix("ROW-02"), "left of old margin should scroll: \(lines[4])")
    #expect(lines[4].contains("MARK-02"), "beyond old width must scroll too, got: \(lines[4])")
}

/// The live corruption: scroll the region, then differentially repaint a row
/// using cursor-forward (CSI C) to skip spaces. If the scroll was wrong, the
/// skipped cells expose stale glyphs between words.
@Test func differentialRepaintAfterRegionScrollLeavesNoResidue() {
    let rows = 24
    let screen = makeScrolledScreen(rows: rows)
    _ = screen.feed(Data("\u{1B}[2;\(rows)r\u{1B}[1S".utf8))
    // Repaint row 2 the way Claude does: write "AB", skip 3 cells, write "CD".
    _ = screen.feed(Data("\u{1B}[2;1HAB\u{1B}[3CCD\u{1B}[r".utf8))

    let lines = screenLines(screen)
    // Row 2 after 1-up scroll held "ROW-03". Claude assumes its skip lands on
    // cells IT populated; the three skipped cells hold whatever the scrolled
    // content had there ("W-0" of ROW-03) — the invariant here is the scroll
    // must have brought ROW-03 (not any other row) into row 2.
    #expect(lines[1].hasPrefix("ABW-0CD"), "expected differential overlay on ROW-03, got: \(lines[1])")
}
