import Foundation
import Testing

@testable import DirijorDaemonKit

@Test func daemonRestartReplayUsesTheDocumentedSmallBudget() {
    #expect(AgentSession.restartReplayBudget == 256 * 1_024)
}

private func tempDir() throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-tests-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

@Test func outputLogAppendAndRead() throws {
    let dir = try tempDir()
    defer { try? FileManager.default.removeItem(at: dir) }
    let log = OutputLog(directory: dir, sessionID: "s_test")

    let range1 = log.append(Data("hello ".utf8))
    let range2 = log.append(Data("world".utf8))
    #expect(range1 == 0..<6)
    #expect(range2 == 6..<11)
    #expect(log.tailOffset == 11)

    let (offset, data) = log.read(fromOffset: 0, maxBytes: 100)
    #expect(offset == 0)
    #expect(String(decoding: data, as: UTF8.self) == "hello world")

    let (offset2, data2) = log.read(fromOffset: 6, maxBytes: 3)
    #expect(offset2 == 6)
    #expect(String(decoding: data2, as: UTF8.self) == "wor")
}

@Test func outputLogRingEvictionServesFromDisk() throws {
    let dir = try tempDir()
    defer { try? FileManager.default.removeItem(at: dir) }
    // Tiny ring (64 bytes), large disk.
    let log = OutputLog(directory: dir, sessionID: "s_ring", ringCapacity: 64, diskCapacity: 1 << 20)

    for i in 0..<10 {
        log.append(Data(String(repeating: "\(i)", count: 32).utf8))
    }
    #expect(log.tailOffset == 320)
    #expect(log.ringStartOffset == 320 - 64)

    // Old bytes must come from disk.
    log.flush()
    let (offset, data) = log.read(fromOffset: 0, maxBytes: 32)
    #expect(offset == 0)
    #expect(String(decoding: data, as: UTF8.self) == String(repeating: "0", count: 32))

    // Recent bytes from the ring.
    let (offset2, data2) = log.read(fromOffset: 320 - 32, maxBytes: 32)
    #expect(offset2 == 320 - 32)
    #expect(String(decoding: data2, as: UTF8.self) == String(repeating: "9", count: 32))
}

@Test func outputLogDiskRotationKeepsOffsetsMonotonic() throws {
    let dir = try tempDir()
    defer { try? FileManager.default.removeItem(at: dir) }
    let log = OutputLog(directory: dir, sessionID: "s_rot", ringCapacity: 128, diskCapacity: 1024)

    // Write 2 KiB → forces at least one half-truncation.
    for i in 0..<64 {
        log.append(Data(String(format: "%032d", i).utf8))
    }
    #expect(log.tailOffset == 2048)
    // Tail still readable, offsets intact.
    let (offset, data) = log.read(fromOffset: 2048 - 32, maxBytes: 32)
    #expect(offset == 2048 - 32)
    #expect(String(decoding: data, as: UTF8.self) == String(format: "%032d", 63))
    // Ancient offsets clamp forward instead of erroring.
    let (clamped, _) = log.read(fromOffset: 0, maxBytes: 32)
    #expect(clamped > 0)
}

@Test func outputLogSyncPointsAndReplayStart() throws {
    let dir = try tempDir()
    defer { try? FileManager.default.removeItem(at: dir) }
    let log = OutputLog(directory: dir, sessionID: "s_sync")

    log.append(Data("some scrollback junk".utf8))
    let clearOffset = log.tailOffset
    log.append(Data("\u{1B}[2J\u{1B}[Hfresh screen".utf8))
    #expect(log.syncPoints.contains(clearOffset))
    #expect(log.preferredReplayStart(budget: 1000) == clearOffset)

    // ESC c reset split across two chunks is still found.
    let esc = log.tailOffset
    log.append(Data([0x1B]))
    log.append(Data([0x63]) + Data("post-reset".utf8))
    #expect(log.syncPoints.contains(esc))
}

@Test func outputLogPersistsAcrossReopen() throws {
    let dir = try tempDir()
    defer { try? FileManager.default.removeItem(at: dir) }
    do {
        let log = OutputLog(directory: dir, sessionID: "s_persist")
        log.append(Data("persisted bytes".utf8))
        log.flush()
    }
    let reopened = OutputLog(directory: dir, sessionID: "s_persist")
    #expect(reopened.tailOffset == 15)
    let (offset, data) = reopened.read(fromOffset: 0, maxBytes: 100)
    #expect(offset == 0)
    #expect(String(decoding: data, as: UTF8.self) == "persisted bytes")
}

@Test func readOnlyOutputLogReusesItsReaderAcrossAppends() throws {
    let dir = try tempDir()
    defer { try? FileManager.default.removeItem(at: dir) }
    let writer = OutputLog(directory: dir, sessionID: "s_live")
    writer.append(Data("first".utf8))
    writer.flush()

    let reader = OutputLog(directory: dir, sessionID: "s_live", readOnly: true)
    let first = reader.read(fromOffset: 0, maxBytes: 32)
    #expect(String(decoding: first.data, as: UTF8.self) == "first")

    writer.append(Data("-second".utf8))
    writer.flush()
    #expect(reader.refreshFromDisk())
    let second = reader.read(fromOffset: 5, maxBytes: 32)
    #expect(String(decoding: second.data, as: UTF8.self) == "-second")
}
