import DirijorDetection
import DirijorProtocol
import Foundation
import SwiftTerm

/// Headless VT emulation of one session's screen, for status detection and
/// remote (iOS / menu-bar) previews. Confined inside its owning AgentSession.
final class HeadlessScreen {
    /// Keep history by cell memory rather than by line count. SwiftTerm's
    /// `CharData` is currently 24 bytes, so a fixed 2,000-line history costs
    /// ~22 MiB at 480 columns — per session. The durable OutputLog remains the
    /// source for byte replay; this buffer is only the recent styled viewport.
    private static let historyCellBudgetBytes = 1 * 1_024 * 1_024
    private static let minimumHistoryLines = 64
    private static let maximumHistoryLines = 512

    /// How a resize should treat the scrollback line budget.
    enum HistoryBudget {
        /// Re-budget in both directions — the settled state.
        case full
        /// Apply the budget only when the new width *lowers* the line limit.
        /// A live drag re-sizes at ~20Hz; growing the retained history is an
        /// allocation that can wait for the drag to settle, while shrinking it
        /// must not, or a widening drag would blow past the memory budget.
        case trimOnly
    }

    private let terminal: Terminal
    private let delegate: ScreenDelegate
    private var inputFilter = HeadlessTerminalInputFilter()
    private(set) var contentSeq: UInt64 = 0
    /// The scrollback limit currently applied to `terminal`, so a `.trimOnly`
    /// resize can tell which direction the budget is moving.
    private var historyLimit: Int

    init(cols: Int, rows: Int) {
        let limit = Self.historyLineLimit(cols: cols)
        let options = TerminalOptions(
            cols: cols,
            rows: rows,
            scrollback: limit,
            enableSixelReported: false,
            kittyImageCacheLimitBytes: 0
        )
        historyLimit = limit
        delegate = ScreenDelegate()
        terminal = Terminal(delegate: delegate, options: options)
    }

    private static func historyLineLimit(cols: Int) -> Int {
        let bytesPerLine = max(1, cols) * MemoryLayout<CharData>.stride
        let budgeted = historyCellBudgetBytes / bytesPerLine
        return max(minimumHistoryLines, min(maximumHistoryLines, budgeted))
    }

    /// The program is on the alternate buffer (no scrollback) — the client uses
    /// this to decide whether local scrollback browsing is appropriate.
    var isAltScreen: Bool { terminal.isCurrentBufferAlternate }

    /// The program has enabled mouse tracking, so it consumes wheel events.
    var mouseReporting: Bool { terminal.mouseMode != .off }

    /// The program enabled bracketed-paste mode (DECSET 2004). A TUI turns this on
    /// only once its input line is live and ready to accept text — so it doubles as
    /// a spawn-readiness marker (safe to start typing) and as the signal that
    /// injected prompts should be wrapped in ESC[200~ … ESC[201~ (embedded newlines
    /// stay literal instead of submitting the composer line by line).
    var bracketedPasteActive: Bool { terminal.bracketedPasteMode }

    /// Feeds raw PTY bytes; returns any bytes the emulator wants sent back to
    /// the child (DA/DSR query responses).
    func feed(_ data: Data) -> Data {
        let filtered = inputFilter.filter(data)
        if !filtered.isEmpty {
            terminal.feed(buffer: [UInt8](filtered)[...])
            contentSeq &+= 1
        }
        let responses = delegate.pendingResponses
        delegate.pendingResponses = Data()
        return responses
    }

    /// Encodes `lines` mouse-wheel steps at cell (col,row) and returns the bytes
    /// to write to the child. Empty when the child hasn't enabled mouse tracking
    /// (a bare shell) — the caller writes nothing, so no escape codes leak to a
    /// prompt. SwiftTerm formats them per the active mouse protocol (SGR/x10/…).
    func mouseWheel(up: Bool, lines: Int, col: Int, row: Int) -> Data {
        guard terminal.mouseMode != .off, lines > 0 else { return Data() }
        let x = max(0, min(terminal.cols - 1, col))
        let y = max(0, min(terminal.rows - 1, row))
        let button = up ? 4 : 5   // X11 wheel buttons
        for _ in 0..<lines {
            let flags = terminal.encodeButton(
                button: button, release: false, shift: false, meta: false, control: false)
            terminal.sendEvent(buttonFlags: flags, x: x, y: y)
        }
        let out = delegate.pendingResponses
        delegate.pendingResponses = Data()
        return out
    }

    func resize(cols: Int, rows: Int, historyBudget: HistoryBudget = .full) {
        // Trim while rows still have their old width. Widening first would
        // temporarily expand every retained line, exactly the peak we avoid.
        let limit = Self.historyLineLimit(cols: cols)
        if historyBudget == .full || limit < historyLimit {
            terminal.changeHistorySize(limit)
            historyLimit = limit
        }
        terminal.resize(cols: cols, rows: rows)
        // SwiftTerm's Buffer.resize clamps marginRight when SHRINKING but never
        // re-grows it on widen, and cmdScrollDown (CSI T) scrolls only up to
        // marginRight. After a widen, region scrolls then move just the left
        // columns; Claude Code's differential repaint (which assumes full-width
        // scrolls) interleaves stale and new text — the "garbled after wheel
        // scroll" bug. xterm resets margins to full width on resize; do the same.
        terminal.buffer.marginLeft = 0
        terminal.buffer.marginRight = cols - 1
        lastCols = 0    // force a full grid snapshot after a geometry change
        lastRows = 0
        contentSeq &+= 1
    }

    /// Applies the scrollback budget a `.trimOnly` resize deferred. Called once
    /// a live resize settles, so a drag pays the re-budget once instead of per
    /// step. No-op when the budget already matches the current width.
    func rebudgetHistory() {
        let limit = Self.historyLineLimit(cols: terminal.cols)
        guard limit != historyLimit else { return }
        terminal.changeHistorySize(limit)
        historyLimit = limit
    }

    var oscTitle: String? { delegate.title }

    func snapshot() -> ScreenSnapshot {
        var lines: [String] = []
        lines.reserveCapacity(terminal.rows)
        for row in 0..<terminal.rows {
            if let line = terminal.getLine(row: row) {
                // SwiftTerm fills blank cells with NUL; normalize to spaces so
                // substring/line-regex detection behaves like a real terminal.
                let raw = line.translateToString(trimRight: true)
                lines.append(Self.sanitize(raw))
            } else {
                lines.append("")
            }
        }
        return ScreenSnapshot(
            lines: lines,
            oscTitle: delegate.title,
            oscProgressState: delegate.progressState,
            oscProgressValue: delegate.progressValue,
            contentSeq: contentSeq,
            cols: terminal.cols,
            rows: terminal.rows
        )
    }

    /// Plain-text render for session.read_screen.
    func text() -> String {
        snapshot().lines.joined(separator: "\n")
    }

    /// True when every visible row is empty — used by resize recovery to detect
    /// a TUI that failed to repaint after a resize.
    func isBlank() -> Bool {
        for row in 0..<terminal.rows {
            if let line = terminal.getLine(row: row),
                !Self.sanitize(line.translateToString(trimRight: true)).trimmingCharacters(in: .whitespaces).isEmpty {
                return false
            }
        }
        return true
    }

    /// The visible non-blank rows as plain lines (trailing blanks trimmed), for
    /// re-display when a resize blanks the screen. The emulator re-wraps them at
    /// the current width on replay (herdr-style recovery).
    func captureVisibleLines() -> [String] {
        var lines: [String] = []
        for row in 0..<terminal.rows {
            guard let line = terminal.getLine(row: row) else { lines.append(""); continue }
            lines.append(Self.sanitize(line.translateToString(trimRight: true)))
        }
        while let last = lines.last, last.trimmingCharacters(in: .whitespaces).isEmpty {
            lines.removeLast()
        }
        return lines
    }

    /// Re-display captured lines into the emulator (not the PTY) after a clear.
    func replay(lines: [String]) {
        let text = "\u{1b}[H\u{1b}[2J" + lines.joined(separator: "\r\n")
        _ = feed(Data(text.utf8))
    }

    /// Restores a persisted visible grid into a fresh emulator. Cursor-address
    /// each non-ASCII cell independently: a wide glyph consumes two terminal
    /// columns while the grid also contains its blank continuation cell, so a
    /// naive row string would shift everything after it by one column.
    func restore(
        checkpoint update: GridUpdate,
        altScreen: Bool,
        bracketedPaste: Bool,
        mouseReporting: Bool
    ) -> Bool {
        guard update.isFullSnapshot,
            update.cols == terminal.cols,
            update.rows == terminal.rows,
            update.changedRows.count == update.rows
        else { return false }

        var bytes = Data()
        bytes.reserveCapacity(update.cols * update.rows * 2)
        if altScreen { bytes.append(contentsOf: "\u{1B}[?1049h".utf8) }
        bytes.append(contentsOf: "\u{1B}[H\u{1B}[2J".utf8)

        for row in update.changedRows.sorted(by: { $0.y < $1.y }) {
            guard row.y >= 0, row.y < update.rows, row.cells.count == update.cols else {
                return false
            }
            var previous: GridCell?
            for (x, cell) in row.cells.enumerated() {
                // The initial clear already created true NUL-backed blank cells.
                // Leaving those untouched preserves SwiftTerm's trimmed-line
                // semantics and makes sparse checkpoints especially cheap.
                if cell == .blank { continue }
                bytes.append(contentsOf: "\u{1B}[\(row.y + 1);\(x + 1)H".utf8)
                if previous?.fg != cell.fg || previous?.bg != cell.bg || previous?.style != cell.style {
                    bytes.append(contentsOf: Self.sgr(for: cell).utf8)
                }
                if let scalar = UnicodeScalar(cell.scalar), cell.scalar != 0 {
                    bytes.append(contentsOf: String(scalar).utf8)
                } else {
                    bytes.append(0x20)
                }
                previous = cell
            }
        }

        bytes.append(contentsOf: "\u{1B}[0m".utf8)
        if bracketedPaste { bytes.append(contentsOf: "\u{1B}[?2004h".utf8) }
        if mouseReporting {
            bytes.append(contentsOf: "\u{1B}[?1000h\u{1B}[?1006h".utf8)
        }
        bytes.append(contentsOf: update.cursorVisible ? "\u{1B}[?25h".utf8 : "\u{1B}[?25l".utf8)
        bytes.append(
            contentsOf: "\u{1B}[\(min(update.rows, max(0, update.cursorRow) + 1));\(min(update.cols, max(0, update.cursorCol) + 1))H".utf8)
        _ = feed(bytes)
        lastCols = 0
        lastRows = 0
        return true
    }

    private static func sgr(for cell: GridCell) -> String {
        var codes = ["0"]
        if cell.style.contains(.bold) { codes.append("1") }
        if cell.style.contains(.dim) { codes.append("2") }
        if cell.style.contains(.italic) { codes.append("3") }
        if cell.style.contains(.underline) { codes.append("4") }
        if cell.style.contains(.blink) { codes.append("5") }
        if cell.style.contains(.inverse) { codes.append("7") }
        if cell.style.contains(.invisible) { codes.append("8") }
        if cell.style.contains(.crossedOut) { codes.append("9") }
        appendColor(cell.fg, foreground: true, to: &codes)
        appendColor(cell.bg, foreground: false, to: &codes)
        return "\u{1B}[\(codes.joined(separator: ";"))m"
    }

    private static func appendColor(_ color: TermColor, foreground: Bool, to codes: inout [String]) {
        switch color {
        case .defaultColor, .defaultInverted:
            codes.append(foreground ? "39" : "49")
        case .ansi(let value):
            codes.append(contentsOf: [foreground ? "38" : "48", "5", String(value)])
        case .rgb(let red, let green, let blue):
            codes.append(
                contentsOf: [
                    foreground ? "38" : "48", "2", String(red), String(green), String(blue),
                ])
        }
    }

    // MARK: Grid extraction (authoritative cell stream)

    /// The last grid we emitted, for row diffing — flattened to `rows * cols`.
    /// As an array-of-arrays this cost one heap allocation per row on every
    /// flush (20 a second, for a screen where two or three rows typically
    /// changed); flat, an unchanged row costs a comparison and nothing else.
    /// `lastCols`/`lastRows` at zero means "no baseline", i.e. force a full.
    private var lastCells: [GridCell] = []
    private var lastCols = 0
    private var lastRows = 0
    /// Reused row buffer feeding the diff, so only rows that actually changed
    /// allocate (copy-on-write hands their storage to the outgoing update).
    private var rowScratch: [GridCell] = []
    #if DEBUG
    /// Test-only evidence that a caller paid the full screen-to-cells cost.
    private(set) var gridExtractionCount = 0
    #endif

    /// Builds a `GridUpdate` from the current screen. When `full` is true (or the
    /// geometry changed) every row is included and the diff baseline resets;
    /// otherwise only rows that changed since the last call are included.
    func gridUpdate(full: Bool) -> GridUpdate {
        #if DEBUG
        gridExtractionCount += 1
        #endif
        let cols = terminal.cols
        let rows = terminal.rows
        let geometryChanged = lastRows != rows || lastCols != cols
        let forceFull = full || geometryChanged
        if geometryChanged {
            lastCells = Array(repeating: .blank, count: rows * cols)
            lastCols = cols
            lastRows = rows
        }
        if rowScratch.count != cols {
            rowScratch = Array(repeating: .blank, count: cols)
        }

        var changed: [(y: Int, cells: [GridCell])] = []
        for y in 0..<rows {
            fillRow(y: y, cols: cols, into: &rowScratch)
            let base = y * cols
            var rowChanged = forceFull
            if !rowChanged {
                for x in 0..<cols where rowScratch[x] != lastCells[base + x] {
                    rowChanged = true
                    break
                }
            }
            guard rowChanged else { continue }
            changed.append((y, rowScratch))
            for x in 0..<cols { lastCells[base + x] = rowScratch[x] }
        }

        return GridUpdate(
            cols: cols, rows: rows,
            cursorCol: terminal.buffer.x, cursorRow: terminal.buffer.y,
            cursorVisible: delegate.cursorVisible,
            isFullSnapshot: forceFull, changedRows: changed)
    }

    /// A full-screen snapshot of the CURRENT grid that does NOT disturb the diff
    /// baseline (`lastCells`). Used to re-seed a sink that fell behind: grid
    /// frames are diffs, so a single missed frame desyncs a sink permanently —
    /// resending the whole screen repairs it without breaking other sinks' diffs.
    func fullSnapshot() -> GridUpdate {
        let cols = terminal.cols
        let rows = terminal.rows
        var all: [(y: Int, cells: [GridCell])] = []
        all.reserveCapacity(rows)
        for y in 0..<rows { all.append((y, rowCells(y: y, cols: cols))) }
        return GridUpdate(
            cols: cols, rows: rows,
            cursorCol: terminal.buffer.x, cursorRow: terminal.buffer.y,
            cursorVisible: delegate.cursorVisible, isFullSnapshot: true, changedRows: all)
    }

    private func rowCells(y: Int, cols: Int) -> [GridCell] {
        cells(from: terminal.getLine(row: y), cols: cols)
    }

    /// Writes row `y` into an existing buffer of exactly `cols` cells.
    private func fillRow(y: Int, cols: Int, into row: inout [GridCell]) {
        guard let line = terminal.getLine(row: y) else {
            for x in 0..<cols { row[x] = .blank }
            return
        }
        for x in 0..<cols { row[x] = Self.cell(from: line, at: x) }
    }

    private func cells(from line: BufferLine?, cols: Int) -> [GridCell] {
        guard let line else {
            return Array(repeating: GridCell.blank, count: cols)
        }
        var out: [GridCell] = []
        out.reserveCapacity(cols)
        for x in 0..<cols {
            out.append(Self.cell(from: line, at: x))
        }
        return out
    }

    private static func cell(from line: BufferLine, at x: Int) -> GridCell {
        let cd = line[x]
        let ch = cd.getCharacter()
        let scalar = ch.unicodeScalars.first?.value ?? 32
        let attr = cd.attribute
        return GridCell(
            scalar: scalar == 0 ? 32 : scalar,
            fg: mapColor(attr.fg),
            bg: mapColor(attr.bg),
            // SwiftTerm CharacterStyle bit layout matches TermStyle exactly.
            style: TermStyle(rawValue: UInt16(attr.style.rawValue)))
    }

    // MARK: Scrollback extraction (history above the visible screen)

    /// SwiftTerm keeps `linesTop` — the origin of `getScrollInvariantLine`'s
    /// absolute row index — internal, yet that method is the only public reader of
    /// scrollback rows. Recover the origin from its boundary: valid indices form a
    /// contiguous window `[linesTop, linesTop+count)` that is always ≥ `rows` tall
    /// (the viewport rows always exist). Climb in ≤ `rows`-sized steps so a window
    /// that tall is never skipped, then walk the low edge down to `linesTop`. A
    /// full reset / alt-buffer switch drops the origin to 0, which index 0 catches.
    private func scrollInvariantOrigin() -> Int {
        if terminal.getScrollInvariantLine(row: 0) != nil { return 0 }
        let cap = max(1, terminal.rows)
        var row = 0
        var step = 1
        while terminal.getScrollInvariantLine(row: row) == nil {
            row += step
            if step < cap { step = min(step * 2, cap) }
        }
        while row > 0, terminal.getScrollInvariantLine(row: row - 1) != nil { row -= 1 }
        return row
    }

    /// Full scrollback as plain text for search. Walks the scroll-invariant rows
    /// from the origin, reusing snapshot()'s NUL→space sanitize.
    func scrollback() -> ReadScrollbackResult {
        let origin = scrollInvariantOrigin()
        var lines: [String] = []
        var row = origin
        while let line = terminal.getScrollInvariantLine(row: row) {
            lines.append(Self.sanitize(line.translateToString(trimRight: true)))
            row += 1
        }
        // The live screen occupies the last `rows` scroll-invariant rows when the
        // emulator is at the bottom (it never user-scrolls headlessly).
        let visibleStartRow = origin + max(0, lines.count - terminal.rows)
        return ReadScrollbackResult(
            lines: lines,
            firstRow: origin,
            visibleStartRow: visibleStartRow,
            cols: terminal.cols,
            rows: terminal.rows,
            contentSeq: contentSeq,
            isAltScreen: terminal.isCurrentBufferAlternate)
    }

    /// A window of scrollback rows as cells (same conversion as the grid), sourced
    /// via scroll-invariant indices and clamped to the valid range.
    func scrollbackCells(firstRow: Int, maxRows: Int) -> ReadScrollbackCellsResult {
        let cols = terminal.cols
        let origin = scrollInvariantOrigin()
        var total = 0
        while terminal.getScrollInvariantLine(row: origin + total) != nil { total += 1 }

        let start = max(firstRow, origin)
        let end = min(start + max(0, maxRows), origin + total)
        var rows: [[GridCell]] = []
        if end > start {
            rows.reserveCapacity(end - start)
            for row in start..<end {
                rows.append(cells(from: terminal.getScrollInvariantLine(row: row), cols: cols))
            }
        }
        return ReadScrollbackCellsResult(
            payload: GridRowCodec.encodeRows(rows),
            firstRow: start,
            rowCount: rows.count,
            totalRows: total,
            liveStartRow: origin + max(0, total - terminal.rows),
            cols: cols,
            contentSeq: contentSeq)
    }

    private static func mapColor(_ c: Attribute.Color) -> TermColor {
        switch c {
        case .defaultColor: return .defaultColor
        case .defaultInvertedColor: return .defaultInverted
        case .ansi256(let code): return .ansi(code)
        case .trueColor(let r, let g, let b): return .rgb(r, g, b)
        }
    }

    /// Replaces NUL (blank-cell filler) and other C0 control characters — except
    /// tab — with spaces so text matching sees a real terminal's whitespace.
    static func sanitize(_ line: String) -> String {
        guard line.unicodeScalars.contains(where: { $0.value < 0x20 && $0 != "\t" }) else {
            return line
        }
        var scalars = String.UnicodeScalarView()
        scalars.reserveCapacity(line.unicodeScalars.count)
        for scalar in line.unicodeScalars {
            scalars.append(scalar.value < 0x20 && scalar != "\t" ? " " : scalar)
        }
        return String(scalars)
    }
}

/// Removes DEC synchronized-output set/reset controls before they reach the
/// headless emulator. Mode 2026 exists solely to freeze a renderer while a TUI
/// updates. HeadlessScreen reads SwiftTerm's live buffer and already coalesces
/// grid delivery, so SwiftTerm's frozen display buffer is never observed; its
/// deep copy of all history is pure memory bandwidth.
///
/// The filter is streaming (escape sequences may straddle PTY reads) and keeps
/// other private modes when an application combines them in one CSI sequence.
struct HeadlessTerminalInputFilter {
    private enum State {
        case text
        case escape
        case csi(prefix: [UInt8], body: [UInt8])
    }

    private var state: State = .text

    mutating func filter(_ data: Data) -> Data {
        var output = Data()
        output.reserveCapacity(data.count)
        for byte in data {
            consume(byte, into: &output)
        }
        return output
    }

    private mutating func consume(_ byte: UInt8, into output: inout Data) {
        switch state {
        case .text:
            if byte == 0x1B {
                state = .escape
            } else {
                output.append(byte)
            }

        case .escape:
            if byte == 0x5B { // '['
                state = .csi(prefix: [0x1B, 0x5B], body: [])
            } else {
                output.append(0x1B)
                state = .text
                consume(byte, into: &output)
            }

        case .csi(let prefix, var body):
            body.append(byte)
            if (0x40...0x7E).contains(byte) {
                emitCSI(prefix: prefix, body: body, into: &output)
                state = .text
            } else if body.count > 1_024 {
                // Malformed/unbounded CSI: stop buffering and pass it through.
                output.append(contentsOf: prefix)
                output.append(contentsOf: body)
                state = .text
            } else {
                state = .csi(prefix: prefix, body: body)
            }
        }
    }

    private func emitCSI(prefix: [UInt8], body: [UInt8], into output: inout Data) {
        guard let final = body.last,
            final == UInt8(ascii: "h") || final == UInt8(ascii: "l"),
            body.first == UInt8(ascii: "?")
        else {
            output.append(contentsOf: prefix)
            output.append(contentsOf: body)
            return
        }

        let parameterBytes = body.dropFirst().dropLast()
        guard let parameterText = String(bytes: parameterBytes, encoding: .ascii) else {
            output.append(contentsOf: prefix)
            output.append(contentsOf: body)
            return
        }
        let parameters = parameterText.split(separator: ";", omittingEmptySubsequences: false)
        guard parameters.allSatisfy({ !$0.isEmpty && $0.allSatisfy(\.isNumber) }),
            parameters.contains("2026")
        else {
            output.append(contentsOf: prefix)
            output.append(contentsOf: body)
            return
        }

        let kept = parameters.filter { $0 != "2026" }
        guard !kept.isEmpty else { return }
        output.append(contentsOf: prefix)
        output.append(UInt8(ascii: "?"))
        output.append(contentsOf: kept.joined(separator: ";").utf8)
        output.append(final)
    }
}

private final class ScreenDelegate: TerminalDelegate {
    var title: String?
    var progressState: Int?
    var progressValue: Int?
    var pendingResponses = Data()
    /// Tracks DECTCM (CSI ?25 h/l). SwiftTerm keeps `cursorHidden` internal, so
    /// mirror it via the delegate callbacks; TUIs (Claude Code, vim redraws)
    /// hide the terminal cursor while painting their own, and the clients must
    /// not draw a stray block there.
    var cursorVisible = true

    func showCursor(source: Terminal) { cursorVisible = true }
    func hideCursor(source: Terminal) { cursorVisible = false }

    func setTerminalTitle(source: Terminal, title: String) {
        self.title = title
    }

    /// Called synchronously when 1049h/l (alt screen) switches buffers.
    /// SwiftTerm's ALT buffer activates with uninitialized left/right margins
    /// (0..0), and cmdScrollDown (CSI T) unconditionally scrolls only
    /// marginLeft...marginRight — so Claude Code's scroll view (which runs in
    /// the alt screen) scrolled a single column, leaving every other column
    /// stale, and its differential repaints interleaved old and new lines.
    /// Reset the just-activated buffer's margins to full width, matching xterm
    /// (margins are only ever narrowed via DECSLRM, which needs DECLRMM).
    func bufferActivated(source: Terminal) {
        source.buffer.marginLeft = 0
        source.buffer.marginRight = source.cols - 1
    }

    func progressReport(source: Terminal, report: Terminal.ProgressReport) {
        progressState = report.state.rawValue
        progressValue = report.progress.map(Int.init)
    }

    func send(source: Terminal, data: ArraySlice<UInt8>) {
        pendingResponses.append(contentsOf: data)
    }

    func sizeChanged(source: Terminal) {}
    func scrolled(source: Terminal, yDisp: Int) {}
}
