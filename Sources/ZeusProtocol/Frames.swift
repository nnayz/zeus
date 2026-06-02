import Foundation

/// Binary frames on a per-session data channel: `[type u8][len u32 BE][payload]`.
public enum FrameType: UInt8, Sendable {
    case output = 1       // payload: offset u64 BE + raw PTY bytes
    case input = 2        // payload: raw bytes for the PTY master
    case resize = 3       // payload: cols u16 BE + rows u16 BE
    case replayBegin = 4  // payload: offset u64 BE (start of replayed range)
    case replayEnd = 5    // payload: offset u64 BE (log tail offset after replay)
    case ping = 6
    case pong = 7
    case grid = 8         // payload: encoded GridUpdate (see Grid.swift)
    case scroll = 9       // payload: dir u8 (0=up,1=down) + lines u16 BE + col u16 BE + row u16 BE
    case modes = 10       // payload: 1-byte bitmask (bit0 altScreen, bit1 mouseReporting)
}

public struct Frame: Sendable, Equatable {
    public var type: FrameType
    public var payload: Data

    public init(type: FrameType, payload: Data = Data()) {
        self.type = type
        self.payload = payload
    }

    // MARK: Typed constructors

    public static func output(offset: UInt64, bytes: Data) -> Frame {
        var payload = Data(capacity: 8 + bytes.count)
        payload.appendBigEndian(offset)
        payload.append(bytes)
        return Frame(type: .output, payload: payload)
    }

    public static func input(_ bytes: Data) -> Frame {
        Frame(type: .input, payload: bytes)
    }

    public static func resize(cols: UInt16, rows: UInt16) -> Frame {
        var payload = Data(capacity: 4)
        payload.appendBigEndian(cols)
        payload.appendBigEndian(rows)
        return Frame(type: .resize, payload: payload)
    }

    public static func replayBegin(offset: UInt64) -> Frame {
        var payload = Data(capacity: 8)
        payload.appendBigEndian(offset)
        return Frame(type: .replayBegin, payload: payload)
    }

    public static func replayEnd(offset: UInt64) -> Frame {
        var payload = Data(capacity: 8)
        payload.appendBigEndian(offset)
        return Frame(type: .replayEnd, payload: payload)
    }

    public static func grid(_ update: GridUpdate) -> Frame {
        Frame(type: .grid, payload: update.encoded())
    }

    /// A mouse-wheel scroll request. The daemon (which owns the emulator and its
    /// mouse mode) turns this into the correct wheel escape sequence — the app
    /// never guesses the mouse protocol. `dir` 0 = up (toward history), 1 = down.
    public static func scroll(dir: UInt8, lines: UInt16, col: UInt16, row: UInt16) -> Frame {
        var payload = Data(capacity: 7)
        payload.append(dir)
        payload.appendBigEndian(lines)
        payload.appendBigEndian(col)
        payload.appendBigEndian(row)
        return Frame(type: .scroll, payload: payload)
    }

    /// Tells the client when local scrollback browsing is appropriate: `altScreen`
    /// (the program is on the alternate buffer, which has no scrollback) and
    /// `mouseReporting` (the program consumes wheel events, so scroll requests go
    /// to it rather than a local viewport). The daemon owns these facts.
    public static func modes(altScreen: Bool, mouseReporting: Bool) -> Frame {
        var bits: UInt8 = 0
        if altScreen { bits |= 1 }
        if mouseReporting { bits |= 2 }
        return Frame(type: .modes, payload: Data([bits]))
    }

    /// For `.grid`: the decoded screen update. Nil if malformed.
    public var gridPayload: GridUpdate? {
        guard type == .grid else { return nil }
        return GridUpdate(decoding: payload)
    }

    /// For `.modes`: (altScreen, mouseReporting). Nil if malformed.
    public var modesPayload: (altScreen: Bool, mouseReporting: Bool)? {
        guard type == .modes, let bits = payload.first else { return nil }
        return (bits & 1 != 0, bits & 2 != 0)
    }

    // MARK: Typed accessors

    /// For `.output`: (stream offset, bytes). Nil if malformed.
    public var outputPayload: (offset: UInt64, bytes: Data)? {
        guard type == .output, payload.count >= 8 else { return nil }
        return (payload.readBigEndianUInt64(at: 0), payload.subdata(in: (payload.startIndex + 8)..<payload.endIndex))
    }

    public var resizePayload: (cols: UInt16, rows: UInt16)? {
        guard type == .resize, payload.count >= 4 else { return nil }
        return (payload.readBigEndianUInt16(at: 0), payload.readBigEndianUInt16(at: 2))
    }

    public var offsetPayload: UInt64? {
        guard type == .replayBegin || type == .replayEnd, payload.count >= 8 else { return nil }
        return payload.readBigEndianUInt64(at: 0)
    }

    /// For `.scroll`: (direction, line count, pointer col, pointer row). Nil if malformed.
    public var scrollPayload: (dir: UInt8, lines: UInt16, col: UInt16, row: UInt16)? {
        guard type == .scroll, payload.count >= 7 else { return nil }
        return (
            payload[payload.startIndex],
            payload.readBigEndianUInt16(at: 1),
            payload.readBigEndianUInt16(at: 3),
            payload.readBigEndianUInt16(at: 5))
    }
}

/// Streaming frame codec. Feed arbitrary chunks; get complete frames.
public struct FrameCodec: Sendable {
    private var buffer = Data()
    /// A single frame larger than this indicates a corrupt stream.
    public static let maxFrameBytes = 16 * 1024 * 1024

    public init() {}

    public static func encode(_ frame: Frame) -> Data {
        var data = Data(capacity: 5 + frame.payload.count)
        data.append(frame.type.rawValue)
        data.appendBigEndian(UInt32(frame.payload.count))
        data.append(frame.payload)
        return data
    }

    public mutating func append(_ data: Data) throws -> [Frame] {
        buffer.append(data)
        var frames: [Frame] = []
        while buffer.count >= 5 {
            let start = buffer.startIndex
            guard let type = FrameType(rawValue: buffer[start]) else {
                throw ControlError.badRequest("unknown frame type \(buffer[start])")
            }
            let length = Int(buffer.readBigEndianUInt32(at: 1))
            guard length <= Self.maxFrameBytes else {
                throw ControlError.badRequest("frame exceeds \(Self.maxFrameBytes) bytes")
            }
            guard buffer.count >= 5 + length else { break }
            let payload = buffer.subdata(in: (start + 5)..<(start + 5 + length))
            buffer.removeSubrange(start..<(start + 5 + length))
            frames.append(Frame(type: type, payload: payload))
        }
        return frames
    }
}

// MARK: - Big-endian helpers

extension Data {
    mutating func appendBigEndian(_ value: UInt64) {
        Swift.withUnsafeBytes(of: value.bigEndian) { append(contentsOf: $0) }
    }
    mutating func appendBigEndian(_ value: UInt32) {
        Swift.withUnsafeBytes(of: value.bigEndian) { append(contentsOf: $0) }
    }
    mutating func appendBigEndian(_ value: UInt16) {
        Swift.withUnsafeBytes(of: value.bigEndian) { append(contentsOf: $0) }
    }

    /// `at` is relative to startIndex; caller guarantees bounds.
    func readBigEndianUInt64(at offset: Int) -> UInt64 {
        var value: UInt64 = 0
        for i in 0..<8 { value = (value << 8) | UInt64(self[startIndex + offset + i]) }
        return value
    }
    func readBigEndianUInt32(at offset: Int) -> UInt32 {
        var value: UInt32 = 0
        for i in 0..<4 { value = (value << 8) | UInt32(self[startIndex + offset + i]) }
        return value
    }
    func readBigEndianUInt16(at offset: Int) -> UInt16 {
        var value: UInt16 = 0
        for i in 0..<2 { value = (value << 8) | UInt16(self[startIndex + offset + i]) }
        return value
    }
}
