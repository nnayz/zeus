import Foundation

// The daemon is the single authoritative terminal emulator (mosh/tmux model).
// It streams the screen as a GRID OF CELLS — not a raw byte log — so the app is
// a pure renderer that never emulates. This eliminates the whole class of
// dual-emulator / byte-replay bugs (mangle on reattach, resize desync): the
// program repaints into the daemon's correctly-sized grid, and the app just
// draws whatever cells the daemon reports.

/// A terminal color, matching SwiftTerm's `Attribute.Color` cases.
public enum TermColor: Equatable, Sendable {
    case defaultColor
    case defaultInverted
    case ansi(UInt8)
    case rgb(UInt8, UInt8, UInt8)

    /// 4-byte packed form: [tag][a][b][c]. Public so renderers can use it as a
    /// cheap cache key.
    public var packed: UInt32 {
        switch self {
        case .defaultColor: return 0
        case .defaultInverted: return 1 << 24
        case .ansi(let c): return (2 << 24) | UInt32(c)
        case .rgb(let r, let g, let b):
            return (3 << 24) | (UInt32(r) << 16) | (UInt32(g) << 8) | UInt32(b)
        }
    }

    static func unpack(_ v: UInt32) -> TermColor {
        switch v >> 24 {
        case 0: return .defaultColor
        case 1: return .defaultInverted
        case 2: return .ansi(UInt8(v & 0xff))
        default:
            return .rgb(UInt8((v >> 16) & 0xff), UInt8((v >> 8) & 0xff), UInt8(v & 0xff))
        }
    }
}

/// Cell style flags — same bit meanings as SwiftTerm's `CharacterStyle`.
public struct TermStyle: OptionSet, Sendable, Equatable {
    public let rawValue: UInt16
    public init(rawValue: UInt16) { self.rawValue = rawValue }

    public static let bold = TermStyle(rawValue: 1 << 0)
    public static let underline = TermStyle(rawValue: 1 << 1)
    public static let blink = TermStyle(rawValue: 1 << 2)
    public static let inverse = TermStyle(rawValue: 1 << 3)
    public static let invisible = TermStyle(rawValue: 1 << 4)
    public static let dim = TermStyle(rawValue: 1 << 5)
    public static let italic = TermStyle(rawValue: 1 << 6)
    public static let crossedOut = TermStyle(rawValue: 1 << 7)
}

/// One screen cell: a unicode scalar plus its colors and style.
public struct GridCell: Equatable, Sendable {
    /// Unicode scalar value; 0 renders as blank.
    public var scalar: UInt32
    public var fg: TermColor
    public var bg: TermColor
    public var style: TermStyle

    public init(scalar: UInt32, fg: TermColor, bg: TermColor, style: TermStyle) {
        self.scalar = scalar
        self.fg = fg
        self.bg = bg
        self.style = style
    }

    public static let blank = GridCell(scalar: 32, fg: .defaultColor, bg: .defaultInverted, style: [])
}

/// A screen update: the grid geometry, cursor, and the rows that changed
/// (all rows for a full snapshot). The app applies it to its cell buffer.
public struct GridUpdate: Sendable {
    public var cols: Int
    public var rows: Int
    public var cursorCol: Int
    public var cursorRow: Int
    public var cursorVisible: Bool
    /// `true` = replace the whole grid (attach / resize); `false` = patch rows.
    public var isFullSnapshot: Bool
    /// Changed rows, each as (rowIndex, cells-of-that-row-length-cols).
    public var changedRows: [(y: Int, cells: [GridCell])]

    public init(
        cols: Int, rows: Int, cursorCol: Int, cursorRow: Int, cursorVisible: Bool,
        isFullSnapshot: Bool, changedRows: [(y: Int, cells: [GridCell])]
    ) {
        self.cols = cols
        self.rows = rows
        self.cursorCol = cursorCol
        self.cursorRow = cursorRow
        self.cursorVisible = cursorVisible
        self.isFullSnapshot = isFullSnapshot
        self.changedRows = changedRows
    }
}

// MARK: - Binary encoding (data-channel Frame payload)
//
// Layout (all big-endian):
//   cols u16, rows u16, cursorCol u16, cursorRow u16, flags u8,
//   rowCount u16,
//   per row: y u16, runCount u16,
//     per run: repeat u16, scalar u32, fgPacked u32, bgPacked u32, style u16
// flags bit0 = cursor visible, bit1 = full snapshot.
// Runs are RLE over identical adjacent cells, so blank rows cost one run.

/// The RLE row codec shared by grid frames and scrollback cell reads, so both
/// speak one cell encoding. A row is `[runCount u16]` followed by `runCount`
/// runs, each `[repeat u16, scalar u32, fgPacked u32, bgPacked u32, style u16]`.
public enum GridRowCodec {
    /// Appends one row's cells (runCount + runs) — the per-row body, no y prefix.
    public static func appendRow(_ cells: [GridCell], to d: inout Data) {
        var runs = Data()
        var runCount: UInt16 = 0
        var i = 0
        while i < cells.count {
            let cell = cells[i]
            var rep: UInt16 = 1
            var j = i + 1
            while j < cells.count, cells[j] == cell, rep < UInt16.max {
                rep += 1
                j += 1
            }
            runs.appendBigEndian(rep)
            runs.appendBigEndian(cell.scalar)
            runs.appendBigEndian(cell.fg.packed)
            runs.appendBigEndian(cell.bg.packed)
            runs.appendBigEndian(cell.style.rawValue)
            runCount += 1
            i = j
        }
        d.appendBigEndian(runCount)
        d.append(runs)
    }

    /// Reads one row's cells written by `appendRow`, advancing `p`. Nil if malformed.
    public static func readRow(from payload: Data, at p: inout Data.Index) -> [GridCell]? {
        func u16() -> UInt16? {
            guard p + 2 <= payload.endIndex else { return nil }
            let v = payload.readBigEndianUInt16(at: p - payload.startIndex)
            p += 2
            return v
        }
        func u32() -> UInt32? {
            guard p + 4 <= payload.endIndex else { return nil }
            let v = payload.readBigEndianUInt32(at: p - payload.startIndex)
            p += 4
            return v
        }
        guard let runCount = u16() else { return nil }
        var cells: [GridCell] = []
        for _ in 0..<runCount {
            guard let rep = u16(), let scalar = u32(), let fg = u32(),
                let bg = u32(), let style = u16()
            else { return nil }
            let cell = GridCell(
                scalar: scalar, fg: TermColor.unpack(fg), bg: TermColor.unpack(bg),
                style: TermStyle(rawValue: style))
            for _ in 0..<rep { cells.append(cell) }
        }
        return cells
    }

    /// Encodes a block of consecutive rows (no y prefixes) — the scrollback payload.
    public static func encodeRows(_ rows: [[GridCell]]) -> Data {
        var d = Data()
        for cells in rows { appendRow(cells, to: &d) }
        return d
    }

    /// Decodes `rowCount` consecutive rows written by `encodeRows`. Nil if malformed.
    public static func decodeRows(_ payload: Data, rowCount: Int) -> [[GridCell]]? {
        var p = payload.startIndex
        var rows: [[GridCell]] = []
        rows.reserveCapacity(rowCount)
        for _ in 0..<rowCount {
            guard let cells = readRow(from: payload, at: &p) else { return nil }
            rows.append(cells)
        }
        return rows
    }
}

extension GridUpdate {
    public func encoded() -> Data {
        var d = Data()
        d.appendBigEndian(UInt16(cols))
        d.appendBigEndian(UInt16(rows))
        d.appendBigEndian(UInt16(clamping: cursorCol))
        d.appendBigEndian(UInt16(clamping: cursorRow))
        var flags: UInt8 = 0
        if cursorVisible { flags |= 1 }
        if isFullSnapshot { flags |= 2 }
        d.append(flags)
        d.appendBigEndian(UInt16(changedRows.count))
        for row in changedRows {
            d.appendBigEndian(UInt16(row.y))
            GridRowCodec.appendRow(row.cells, to: &d)
        }
        return d
    }

    public init?(decoding payload: Data) {
        var p = payload.startIndex
        func u16() -> UInt16? {
            guard p + 2 <= payload.endIndex else { return nil }
            let v = payload.readBigEndianUInt16(at: p - payload.startIndex)
            p += 2
            return v
        }
        func u8() -> UInt8? {
            guard p < payload.endIndex else { return nil }
            let v = payload[p]
            p += 1
            return v
        }
        guard let c = u16(), let r = u16(), let cc = u16(), let cr = u16(),
            let flags = u8(), let rowCount = u16()
        else { return nil }
        var rows: [(y: Int, cells: [GridCell])] = []
        rows.reserveCapacity(Int(rowCount))
        for _ in 0..<rowCount {
            guard let y = u16(), let cells = GridRowCodec.readRow(from: payload, at: &p)
            else { return nil }
            rows.append((Int(y), cells))
        }
        self.init(
            cols: Int(c), rows: Int(r), cursorCol: Int(cc), cursorRow: Int(cr),
            cursorVisible: flags & 1 != 0, isFullSnapshot: flags & 2 != 0, changedRows: rows)
    }
}
