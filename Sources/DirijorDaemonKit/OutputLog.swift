import Foundation

/// Append-only, offset-addressed log of one session's raw PTY output.
///
/// Every byte ever produced has a monotonically increasing stream offset.
/// The most recent `ringCapacity` bytes stay in memory; everything is also
/// appended to a spill file (with a base-offset header) capped at
/// `diskCapacity` via half-truncation rewrites. Offsets never reset, so
/// clients can always reconcile with "skip to now" semantics.
///
/// Not thread-safe: owned and confined by one `AgentSession` actor.
final class OutputLog {
    /// Spill file layout: 16-byte header (magic u32, version u32, baseOffset u64 BE)
    /// followed by raw bytes starting at stream offset `baseOffset`.
    private static let magic: UInt32 = 0x4449_524A  // "DIRJ"
    private static let headerSize = 16

    let ringCapacity: Int
    let diskCapacity: Int

    private(set) var tailOffset: UInt64 = 0
    /// Oldest offset still available in memory.
    private(set) var ringStartOffset: UInt64 = 0
    private var ring = Data()

    private let fileURL: URL
    private let readOnly: Bool
    private var fileHandle: FileHandle?
    /// Long-lived reader for the daemon side. PTY output can trigger hundreds
    /// of filesystem events per second; opening and closing an NSFileHandle for
    /// every 64 KB chunk dominated log-drain samples.
    private var readHandle: FileHandle?
    private var fileBaseOffset: UInt64 = 0
    private var fileBytes: Int = 0
    private var lastSync = Date.distantPast
    private let syncInterval: TimeInterval = 2.0

    /// Offsets at which a full-screen reset begins (ESC c or CSI 2J) — clean
    /// replay starting points. Newest last, bounded.
    private(set) var syncPoints: [UInt64] = []
    private static let maxSyncPoints = 64
    /// Tracks a partial escape prefix across chunk boundaries (up to 3 bytes).
    private var carry = Data()

    init(
        directory: URL,
        sessionID: String,
        ringCapacity: Int = 4 << 20,
        diskCapacity: Int = SessionLogStorage.perSessionBytes,
        readOnly: Bool = false
    ) {
        self.ringCapacity = ringCapacity
        self.diskCapacity = diskCapacity
        self.readOnly = readOnly
        self.fileURL = directory.appendingPathComponent("\(sessionID).bin")
        try? FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        openOrRecoverFile()
    }

    deinit {
        close()
    }

    // MARK: Append

    /// Appends a chunk; returns the offset range it occupies.
    @discardableResult
    func append(_ data: Data) -> Range<UInt64> {
        guard !data.isEmpty else { return tailOffset..<tailOffset }
        let start = tailOffset

        scanForSyncPoints(data, chunkStart: start)

        ring.append(data)
        if ring.count > ringCapacity {
            let drop = ring.count - ringCapacity
            ring.removeFirst(drop)
            ringStartOffset += UInt64(drop)
        }
        tailOffset += UInt64(data.count)

        appendToDisk(data)
        return start..<tailOffset
    }

    // MARK: Read

    /// Reads up to `maxBytes` starting at `fromOffset` (clamped to what's still
    /// available). Returns the actual start offset and the bytes.
    func read(fromOffset: UInt64, maxBytes: Int) -> (offset: UInt64, data: Data) {
        let oldestAvailable = min(ringStartOffset, oldestDiskOffset())
        let start = max(fromOffset, oldestAvailable)
        guard start < tailOffset else { return (tailOffset, Data()) }
        let end = min(tailOffset, start + UInt64(maxBytes))

        if !readOnly, start >= ringStartOffset,
            end - ringStartOffset <= UInt64(ring.count)
        {
            let lower = Int(start - ringStartOffset)
            let upper = Int(end - ringStartOffset)
            let slice = ring.subdata(in: ring.startIndex + lower..<ring.startIndex + upper)
            return (start, slice)
        }
        // Serve older bytes from disk.
        return readFromDisk(fromOffset: start, toOffset: end)
    }

    /// Best offset to start a replay from, given a byte budget: the newest
    /// sync point within budget, else budget bytes back from the tail.
    func preferredReplayStart(budget: Int) -> UInt64 {
        let floor = tailOffset > UInt64(budget) ? tailOffset - UInt64(budget) : 0
        if let sync = syncPoints.last(where: { $0 >= floor && $0 < tailOffset }) {
            return sync
        }
        return max(floor, min(ringStartOffset, oldestDiskOffset(), tailOffset))
    }

    func flush() {
        guard !readOnly else { return }
        try? fileHandle?.synchronize()
        lastSync = Date()
    }

    /// Refreshes this reader after the holder appended or rotated the spill file.
    @discardableResult
    func refreshFromDisk() -> Bool {
        guard readOnly,
            let handle = reader(),
            (try? handle.seek(toOffset: 0)) != nil,
            let header = try? handle.read(upToCount: Self.headerSize),
            header.count == Self.headerSize,
            header.readBigEndianUInt32At(0) == Self.magic
        else { return false }

        let newBase = header.readBigEndianUInt64At(8)
        let size = (try? handle.seekToEnd()) ?? UInt64(Self.headerSize)
        let newBytes = max(0, Int(size) - Self.headerSize)
        let newTail = newBase + UInt64(newBytes)
        let changed = newTail != tailOffset || newBase != fileBaseOffset
        fileBaseOffset = newBase
        fileBytes = newBytes
        tailOffset = newTail
        syncPoints.removeAll { $0 < newBase }
        return changed
    }

    func close() {
        try? fileHandle?.synchronize()
        try? fileHandle?.close()
        fileHandle = nil
        invalidateReadHandle()
    }

    // MARK: - Sync-point scanning

    private func scanForSyncPoints(_ data: Data, chunkStart: UInt64) {
        // Work over carry + chunk so sequences split across chunks are found.
        let carryCount = carry.count
        var haystack = carry
        haystack.append(data)
        carry = Data(haystack.suffix(3))

        let bytes = [UInt8](haystack)
        var i = 0
        while i < bytes.count - 1 {
            if bytes[i] == 0x1B {
                var found = false
                if bytes[i + 1] == 0x63 {  // ESC c — full reset
                    found = true
                } else if i + 3 < bytes.count, bytes[i + 1] == 0x5B, bytes[i + 2] == 0x32,
                    bytes[i + 3] == 0x4A
                {  // CSI 2J — erase display
                    found = true
                }
                if found {
                    // A sequence that begins inside the carry region starts
                    // before chunkStart — still a valid (earlier) replay start.
                    let signedOffset = Int64(chunkStart) + Int64(i - carryCount)
                    let offset = UInt64(max(0, signedOffset))
                    if syncPoints.last != offset {
                        syncPoints.append(offset)
                        if syncPoints.count > Self.maxSyncPoints {
                            syncPoints.removeFirst(syncPoints.count - Self.maxSyncPoints)
                        }
                    }
                }
            }
            i += 1
        }
    }

    // MARK: - Disk spill

    private func openOrRecoverFile() {
        let fm = FileManager.default
        if fm.fileExists(atPath: fileURL.path),
            let handle = try? FileHandle(forReadingFrom: fileURL),
            let header = try? handle.read(upToCount: Self.headerSize),
            header.count == Self.headerSize,
            header.readBigEndianUInt32At(0) == Self.magic
        {
            let base = header.readBigEndianUInt64At(8)
            let size = (try? handle.seekToEnd()) ?? UInt64(Self.headerSize)
            fileBaseOffset = base
            fileBytes = Int(size) - Self.headerSize
            tailOffset = base + UInt64(fileBytes)
            ringStartOffset = tailOffset
            if readOnly {
                readHandle = handle
            } else {
                try? handle.close()
                fileHandle = try? FileHandle(forWritingTo: fileURL)
                _ = try? fileHandle?.seekToEnd()
            }
        } else {
            createFile(baseOffset: 0)
        }
    }

    private func createFile(baseOffset: UInt64) {
        invalidateReadHandle()
        var header = Data()
        header.appendBigEndianUInt32(Self.magic)
        header.appendBigEndianUInt32(1)
        header.appendBigEndianUInt64(baseOffset)
        try? header.write(to: fileURL, options: .atomic)
        try? FileManager.default.setAttributes(
            [.posixPermissions: NSNumber(value: Int16(0o600))],
            ofItemAtPath: fileURL.path)
        fileBaseOffset = baseOffset
        fileBytes = 0
        if !readOnly {
            fileHandle = try? FileHandle(forWritingTo: fileURL)
            _ = try? fileHandle?.seekToEnd()
        }
    }

    private func appendToDisk(_ data: Data) {
        guard !readOnly else { return }
        guard let handle = fileHandle else { return }
        try? handle.write(contentsOf: data)
        fileBytes += data.count

        if Date().timeIntervalSince(lastSync) > syncInterval {
            try? handle.synchronize()
            lastSync = Date()
        }

        if fileBytes > diskCapacity {
            truncateToHalf()
        }
    }

    /// Rewrites the spill file keeping only the newest half, bumping baseOffset.
    private func truncateToHalf() {
        invalidateReadHandle()
        guard let read = try? FileHandle(forReadingFrom: fileURL) else { return }
        let keep = diskCapacity / 2
        let dropBytes = fileBytes - keep
        try? read.seek(toOffset: UInt64(Self.headerSize + dropBytes))
        let kept = (try? read.readToEnd()) ?? Data()
        try? read.close()
        try? fileHandle?.close()

        let newBase = fileBaseOffset + UInt64(dropBytes)
        var content = Data()
        content.appendBigEndianUInt32(Self.magic)
        content.appendBigEndianUInt32(1)
        content.appendBigEndianUInt64(newBase)
        content.append(kept)
        let tmp = fileURL.appendingPathExtension("tmp")
        try? content.write(to: tmp)
        _ = try? FileManager.default.replaceItemAt(fileURL, withItemAt: tmp)

        fileBaseOffset = newBase
        fileBytes = kept.count
        fileHandle = try? FileHandle(forWritingTo: fileURL)
        _ = try? fileHandle?.seekToEnd()
        syncPoints.removeAll { $0 < newBase }
    }

    private func oldestDiskOffset() -> UInt64 { fileBaseOffset }

    private func readFromDisk(fromOffset: UInt64, toOffset: UInt64) -> (UInt64, Data) {
        guard let handle = reader() else {
            return (tailOffset, Data())
        }
        try? fileHandle?.synchronize()
        let start = max(fromOffset, fileBaseOffset)
        let filePos = UInt64(Self.headerSize) + (start - fileBaseOffset)
        try? handle.seek(toOffset: filePos)
        let count = Int(toOffset - start)
        let data = (try? handle.read(upToCount: count)) ?? Data()
        return (start, data)
    }

    /// File replacement/truncation changes the inode behind a cached reader.
    /// AgentSession calls this before consuming a rename/delete notification.
    func invalidateReadHandle() {
        try? readHandle?.close()
        readHandle = nil
    }

    private func reader() -> FileHandle? {
        if let readHandle { return readHandle }
        let opened = try? FileHandle(forReadingFrom: fileURL)
        readHandle = opened
        return opened
    }
}

// MARK: - Local big-endian helpers (DirijorProtocol's are internal to that module)

extension Data {
    mutating func appendBigEndianUInt32(_ value: UInt32) {
        Swift.withUnsafeBytes(of: value.bigEndian) { append(contentsOf: $0) }
    }
    mutating func appendBigEndianUInt64(_ value: UInt64) {
        Swift.withUnsafeBytes(of: value.bigEndian) { append(contentsOf: $0) }
    }
    func readBigEndianUInt32At(_ offset: Int) -> UInt32 {
        var value: UInt32 = 0
        for i in 0..<4 { value = (value << 8) | UInt32(self[startIndex + offset + i]) }
        return value
    }
    func readBigEndianUInt64At(_ offset: Int) -> UInt64 {
        var value: UInt64 = 0
        for i in 0..<8 { value = (value << 8) | UInt64(self[startIndex + offset + i]) }
        return value
    }
}
