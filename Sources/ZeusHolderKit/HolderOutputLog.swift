import Foundation

/// The holder is the sole writer for the existing OutputLog file format.
/// Stream offsets remain monotonic even when the bounded spill file rotates.
public final class HolderOutputLog: @unchecked Sendable {
    public static let magic: UInt32 = 0x4449_524A
    public static let headerSize = 16

    private let lock = NSLock()
    private let fileURL: URL
    private let diskCapacity: Int
    private var handle: FileHandle
    private var baseOffset: UInt64
    private var fileBytes: Int
    private var tailOffsetStorage: UInt64

    public var tailOffset: UInt64 {
        lock.lock()
        defer { lock.unlock() }
        return tailOffsetStorage
    }

    public init(fileURL: URL, diskCapacity: Int = 32 << 20) throws {
        self.fileURL = fileURL
        self.diskCapacity = diskCapacity
        try FileManager.default.createDirectory(
            at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)

        if let recovered = Self.recover(fileURL: fileURL) {
            baseOffset = recovered.base
            fileBytes = recovered.bytes
            tailOffsetStorage = recovered.base + UInt64(recovered.bytes)
        } else {
            var header = Data()
            header.appendBigEndianUInt32(Self.magic)
            header.appendBigEndianUInt32(1)
            header.appendBigEndianUInt64(0)
            try header.write(to: fileURL, options: .atomic)
            baseOffset = 0
            fileBytes = 0
            tailOffsetStorage = 0
        }
        handle = try FileHandle(forWritingTo: fileURL)
        try handle.seekToEnd()
    }

    deinit {
        try? handle.synchronize()
        try? handle.close()
    }

    @discardableResult
    public func append(_ data: Data) throws -> Range<UInt64> {
        lock.lock()
        defer { lock.unlock() }
        let start = tailOffsetStorage
        guard !data.isEmpty else { return start..<start }
        try handle.write(contentsOf: data)
        fileBytes += data.count
        tailOffsetStorage += UInt64(data.count)
        if fileBytes > diskCapacity { try rotateToHalf() }
        return start..<tailOffsetStorage
    }

    public func flush() {
        lock.lock()
        defer { lock.unlock() }
        try? handle.synchronize()
    }

    private func rotateToHalf() throws {
        let read = try FileHandle(forReadingFrom: fileURL)
        let keep = diskCapacity / 2
        let drop = fileBytes - keep
        try read.seek(toOffset: UInt64(Self.headerSize + drop))
        let kept = try read.readToEnd() ?? Data()
        try read.close()
        try handle.close()

        let newBase = baseOffset + UInt64(drop)
        var content = Data()
        content.appendBigEndianUInt32(Self.magic)
        content.appendBigEndianUInt32(1)
        content.appendBigEndianUInt64(newBase)
        content.append(kept)
        let temporary = fileURL.appendingPathExtension("holder-tmp")
        try content.write(to: temporary, options: .atomic)
        _ = try FileManager.default.replaceItemAt(fileURL, withItemAt: temporary)

        baseOffset = newBase
        fileBytes = kept.count
        handle = try FileHandle(forWritingTo: fileURL)
        try handle.seekToEnd()
    }

    private static func recover(fileURL: URL) -> (base: UInt64, bytes: Int)? {
        guard let read = try? FileHandle(forReadingFrom: fileURL),
            let header = try? read.read(upToCount: headerSize),
            header.count == headerSize,
            header.readBigEndianUInt32(at: 0) == magic
        else { return nil }
        let base = header.readBigEndianUInt64(at: 8)
        let end = (try? read.seekToEnd()) ?? UInt64(headerSize)
        try? read.close()
        return (base, max(0, Int(end) - headerSize))
    }
}

extension Data {
    mutating func appendBigEndianUInt32(_ value: UInt32) {
        Swift.withUnsafeBytes(of: value.bigEndian) { append(contentsOf: $0) }
    }

    mutating func appendBigEndianUInt64(_ value: UInt64) {
        Swift.withUnsafeBytes(of: value.bigEndian) { append(contentsOf: $0) }
    }

    func readBigEndianUInt32(at offset: Int) -> UInt32 {
        var value: UInt32 = 0
        for index in 0..<4 { value = (value << 8) | UInt32(self[startIndex + offset + index]) }
        return value
    }

    func readBigEndianUInt64(at offset: Int) -> UInt64 {
        var value: UInt64 = 0
        for index in 0..<8 { value = (value << 8) | UInt64(self[startIndex + offset + index]) }
        return value
    }
}
