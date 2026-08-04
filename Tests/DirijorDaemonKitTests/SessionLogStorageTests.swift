import DirijorCore
import Foundation
import Testing

@testable import DirijorDaemonKit

@Test func outputLogIsOwnerOnly() throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-log-mode-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    let log = OutputLog(directory: dir, sessionID: "s_private")
    log.append(Data("secret".utf8))

    let attrs = try FileManager.default.attributesOfItem(
        atPath: dir.appendingPathComponent("s_private.bin").path)
    #expect((attrs[.posixPermissions] as? NSNumber)?.intValue == 0o600)
}

@Test func sessionLogStorageDeletesOrphansAndEnforcesBudget() throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-log-prune-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    let keep = SessionID(rawValue: "s_keep")
    let old = SessionID(rawValue: "s_old")
    let orphan = dir.appendingPathComponent("s_orphan.bin")
    try Data(repeating: 1, count: 32).write(to: orphan)
    let oldURL = SessionLogStorage.url(directory: dir, sessionID: old)
    try Data(repeating: 2, count: 32).write(to: oldURL)
    try FileManager.default.setAttributes(
        [.modificationDate: Date.distantPast], ofItemAtPath: oldURL.path)
    let keepURL = SessionLogStorage.url(directory: dir, sessionID: keep)
    try Data(repeating: 3, count: 32).write(to: keepURL)

    SessionLogStorage.prune(directory: dir, keeping: Set([keep, old]), budget: 32)

    #expect(!FileManager.default.fileExists(atPath: orphan.path))
    #expect(!FileManager.default.fileExists(atPath: oldURL.path))
    #expect(FileManager.default.fileExists(atPath: keepURL.path))
}

@Test func sessionLogStorageNeverEvictsLiveProtectedLogs() throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-log-live-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    let live = SessionID(rawValue: "s_live")
    let closed = SessionID(rawValue: "s_closed")
    let liveURL = SessionLogStorage.url(directory: dir, sessionID: live)
    let closedURL = SessionLogStorage.url(directory: dir, sessionID: closed)
    try Data(repeating: 1, count: 32).write(to: liveURL)
    try Data(repeating: 2, count: 32).write(to: closedURL)
    try FileManager.default.setAttributes(
        [.modificationDate: Date.distantPast], ofItemAtPath: liveURL.path)

    SessionLogStorage.prune(
        directory: dir,
        keeping: Set([live, closed]),
        protectedSessionIDs: Set([live]),
        budget: 32)

    #expect(FileManager.default.fileExists(atPath: liveURL.path))
    #expect(!FileManager.default.fileExists(atPath: closedURL.path))
}
