import DirijorCore
import DirijorHolderKit
import DirijorProtocol
import Foundation
import Testing

@testable import DirijorDaemonKit

/// Collects frames delivered by an AgentSession for assertions.
final class CollectingSink: SessionOutputSink, @unchecked Sendable {
    let sinkID = UUID()
    private let lock = NSLock()
    private var _frames: [Frame] = []

    var pendingBytes: Int { 0 }

    var frames: [Frame] {
        lock.lock()
        defer { lock.unlock() }
        return _frames
    }

    var outputText: String {
        let data = frames.compactMap { $0.outputPayload?.bytes }.reduce(Data(), +)
        return String(decoding: data, as: UTF8.self)
    }

    func deliver(_ frame: Frame) {
        lock.lock()
        _frames.append(frame)
        lock.unlock()
    }

    func closeSink() {}
}

@Test func realPTYSessionEchoAndExit() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-pty-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    let exited = Expectation()
    let session = AgentSession(
        id: SessionID(rawValue: "s_cat"), kind: .shell, logDirectory: dir)
    try await session.start(
        argv: ["/bin/cat"], cwd: "/tmp", extraEnv: [:]
    ) { _, info in
        exited.fulfill(with: info)
    }

    // Attach a sink; replay of an empty log is just REPLAY_BEGIN/END.
    let sink = CollectingSink()
    await session.attach(sink, fromOffset: nil)

    await session.write(Data("hello pty\r".utf8))

    // The mosh/grid model intentionally does not send legacy raw-output frames
    // to attached clients. Wait on the daemon's authoritative screen instead.
    try await waitUntil(timeout: .seconds(5)) {
        let snapshot = await session.makeSnapshot()
        return snapshot.lines.joined().contains("hello pty")
    }
    let tail = await session.logTailOffset
    #expect(tail > 0)

    // Screen snapshot saw the echoed bytes.
    let snapshot = await session.makeSnapshot()
    #expect(snapshot.lines.joined().contains("hello pty"))
    #expect(snapshot.contentSeq > 0)

    await session.terminate(graceSeconds: 0.2)
    let info = try await exited.value(timeout: .seconds(5))
    #expect(info.reason == .signaled || info.reason == .exited)

    let running = await session.isRunning
    #expect(!running)
}

/// A detached session still keeps its emulator authoritative for snapshots and
/// later attachment, but it must not continuously convert the whole screen into
/// client grid cells when there is no client to consume those cells.
@Test func detachedOutputSkipsGridExtractionUntilAClientAttaches() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-detached-grid-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    let session = AgentSession(
        id: SessionID(rawValue: "s_detached_grid"), kind: .shell, logDirectory: dir)
    try await session.start(argv: ["/bin/cat"], cwd: "/tmp", extraEnv: [:]) { _, _ in }
    defer { Task { await session.terminate(graceSeconds: 0.2) } }

    try await waitUntil(timeout: .seconds(5)) { await session.isRunning }
    await session.write(Data("detached output\r".utf8))
    try await waitUntil(timeout: .seconds(5)) {
        let snapshot = await session.makeSnapshot()
        return snapshot.lines.joined().contains("detached output")
    }
    try await Task.sleep(for: .milliseconds(100))

    #expect(await session.gridExtractionCountForTesting == 0)

    let sink = CollectingSink()
    await session.attach(sink, fromOffset: nil)
    #expect(await session.gridExtractionCountForTesting == 1)
    #expect(sink.frames.compactMap(\.gridPayload).last?.isFullSnapshot == true)
}

/// A cursor-only change (space over a blank cell, arrow keys at a prompt) must
/// still reach clients: the cursor rides inside GridUpdate, and diffs with no
/// changed rows used to be dropped wholesale — the caret froze until the next
/// glyph changed.
@Test func cursorOnlyMoveStillBroadcastsGridFrame() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-cursor-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    let session = AgentSession(
        id: SessionID(rawValue: "s_cursor"), kind: .shell, logDirectory: dir)
    try await session.start(argv: ["/bin/cat"], cwd: "/tmp", extraEnv: [:]) { _, _ in }
    defer { Task { await session.terminate(graceSeconds: 0.2) } }

    let sink = CollectingSink()
    await session.attach(sink, fromOffset: nil)

    // The exec is deferred (debounced on client resizes); a write before the
    // PTY exists is dropped. Wait for the real spawn first.
    try await waitUntil(timeout: .seconds(5)) { await session.isRunning }

    // First establish the diff baseline past the launch-resize full snapshot:
    // an echoed "x" is a content change, so its frame arrives regardless.
    await session.write(Data("x".utf8))
    try await waitUntil(timeout: .seconds(5)) {
        sink.frames.compactMap(\.gridPayload).contains { $0.cursorCol == 1 }
    }

    // Now the echoed space writes a space over an already-blank cell: NO row
    // content changes, only the cursor advances. The frame must still arrive.
    await session.write(Data(" ".utf8))
    try await waitUntil(timeout: .seconds(5)) {
        sink.frames.compactMap(\.gridPayload).contains { $0.cursorCol == 2 }
    }
    // And it really was cursor-only — the regression this guards against is
    // dropping exactly these frames.
    let frame = sink.frames.compactMap(\.gridPayload).last { $0.cursorCol == 2 }
    #expect(frame?.changedRows.isEmpty == true)
}

/// Session hand-off: a phone that attaches TAKES CONTROL (PTY resizes to the
/// phone, remoteActive true); the Mac can reclaim geometry with setOwner(.desktop)
/// while the phone stays attached; the phone reclaims with setOwner(.mobile); and
/// the phone leaving auto-hands control back to the Mac.
@Test func sessionHandoffGeometry() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-geom-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    // Capture every remoteActive flip fired via the callback.
    let flips = RemoteActiveRecorder()

    let session = AgentSession(
        id: SessionID(rawValue: "s_geom"), kind: .shell, logDirectory: dir)
    await session.setRemoteActiveCallback { _, active in flips.record(active) }
    try await session.start(
        argv: ["/bin/cat"], cwd: "/tmp", extraEnv: [:]) { _, _ in }
    defer { Task { await session.terminate(graceSeconds: 0.2) } }

    // Desktop attaches and sizes the session; the deferred launch fires at 120x40.
    let desktop = CollectingSink()
    await session.attach(desktop, fromOffset: nil, role: .desktop)
    await session.resize(cols: 120, rows: 40, role: .desktop)
    try await waitUntil(timeout: .seconds(5)) {
        let s = await session.makeSnapshot()
        return s.cols == 120 && s.rows == 40
    }
    var active = await session.remoteActive
    #expect(!active)   // no phone yet

    // A phone attaches and reports its own size → it TAKES CONTROL: the PTY
    // reflows to the phone geometry and remoteActive becomes true.
    let phone = CollectingSink()
    await session.attach(phone, fromOffset: nil, role: .mobile)
    await session.resize(cols: 50, rows: 30, role: .mobile)
    try await waitUntil(timeout: .seconds(5)) {
        let s = await session.makeSnapshot()
        return s.cols == 50 && s.rows == 30
    }
    active = await session.remoteActive
    #expect(active)

    // The Mac reclaims while the phone is still attached: geometry returns to the
    // desktop size and remoteActive goes false.
    await session.setOwner(role: .desktop)
    try await waitUntil(timeout: .seconds(5)) {
        let s = await session.makeSnapshot()
        return s.cols == 120 && s.rows == 40
    }
    active = await session.remoteActive
    #expect(!active)

    // The phone taps "Resume": it reclaims control, PTY back to phone size.
    await session.setOwner(role: .mobile)
    try await waitUntil(timeout: .seconds(5)) {
        let s = await session.makeSnapshot()
        return s.cols == 50 && s.rows == 30
    }
    active = await session.remoteActive
    #expect(active)

    // The phone leaves: auto hand-back to the Mac (desktop size, remoteActive false).
    await session.detach(sinkID: phone.sinkID)
    try await waitUntil(timeout: .seconds(5)) {
        let s = await session.makeSnapshot()
        return s.cols == 120 && s.rows == 40
    }
    active = await session.remoteActive
    #expect(!active)

    // The callback saw exactly the flips true, false, true, false.
    try await waitUntil(timeout: .seconds(2)) { flips.values.count == 4 }
    #expect(flips.values == [true, false, true, false])
}

@Test func registryPersistenceRoundTrip() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-reg-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    let config = DaemonConfig(
        socketPath: dir.appendingPathComponent("d.sock").path,
        cliPath: "/usr/bin/true",
        injectDir: dir,
        logsDir: dir,
        stateFile: dir.appendingPathComponent("state.json")
    )

    // First "daemon run": spawn a short shell session, persist.
    do {
        let events = EventBus()
        let registry = SessionRegistry(config: config, events: events)
        let record = try await registry.spawn(
            SessionSpawnParams(kind: .shell, cwd: "/tmp", title: "my shell"))
        #expect(record.title == "my shell")
        #expect(record.status.isRunning)
        try await registry.kill(sessionID: record.id)
        await registry.prepareShutdown()
    }

    // Second "daemon run": restore; the session must be exited + present.
    let events2 = EventBus()
    let registry2 = SessionRegistry(config: config, events: events2)
    await registry2.restoreFromDisk()
    let list = await registry2.list()
    #expect(list.sessions.count == 1)
    let restored = try #require(list.sessions.first)
    #expect(restored.title == "my shell")
    #expect(!restored.status.isRunning)
    #expect(list.projects.count == 1)
}

@Test func registryReattachesLiveHolderAcrossDaemonRestart() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-reattach-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    let config = DaemonConfig(
        socketPath: dir.appendingPathComponent("d.sock").path,
        cliPath: "/usr/bin/true",
        injectDir: dir,
        logsDir: dir.appendingPathComponent("logs"),
        stateFile: dir.appendingPathComponent("state.json")
    )

    let record: SessionRecord
    var firstOffset: UInt64 = 0
    do {
        let registry = SessionRegistry(config: config, events: EventBus())
        record = try await registry.spawn(
            SessionSpawnParams(kind: .shell, cwd: "/tmp", title: "persistent shell"))
        let session = try #require(await registry.session(record.id))
        try await waitUntil(timeout: .seconds(5)) { await session.isRunning }

        await session.write(Data("before-restart\r".utf8))
        try await waitUntil(timeout: .seconds(5)) {
            await session.screenText().contains("before-restart")
        }
        firstOffset = await session.logTailOffset
        #expect(firstOffset > 0)
        await registry.prepareShutdown()
        // Scope exit drops the entire old registry/session graph. No kill is
        // sent: the holder and PTY child must remain alive.
    }

    let restoredRegistry = SessionRegistry(config: config, events: EventBus())
    await restoredRegistry.restoreFromDisk()
    let restoredRecord = try #require(await restoredRegistry.record(record.id))
    #expect(restoredRecord.status.isRunning)
    #expect(restoredRecord.resumability == .live)

    let restored = try #require(await restoredRegistry.session(record.id))
    #expect(await restored.isRunning)
    #expect(await restored.logTailOffset >= firstOffset)
    try await waitUntil(timeout: .seconds(5)) {
        await restored.screenText().contains("before-restart")
    }

    let sink = CollectingSink()
    await restored.attach(sink, fromOffset: firstOffset)
    await restored.write(Data("after-restart\r".utf8))
    try await waitUntil(timeout: .seconds(5)) {
        await restored.screenText().contains("after-restart")
    }
    let resumedFrame = try #require(
        sink.frames.compactMap(\.outputPayload).first {
            String(decoding: $0.bytes, as: UTF8.self).contains("after-restart")
        })
    #expect(resumedFrame.offset >= firstOffset)
    #expect(await restored.logTailOffset > firstOffset)

    try await restoredRegistry.kill(sessionID: record.id)
    try await waitUntil(timeout: .seconds(5)) { await restored.isRunning == false }
}

@Test func agentSessionsShareOneExternalHolderManager() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-shared-holder-\(UUID().uuidString)")
    let logs = dir.appendingPathComponent("logs")
    let holders = dir.appendingPathComponent("holders")
    try FileManager.default.createDirectory(at: logs, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    let firstID = SessionID(rawValue: "shared_a")
    let secondID = SessionID(rawValue: "shared_b")
    let first = AgentSession(
        id: firstID,
        kind: .shell,
        logDirectory: logs,
        holderDirectory: holders)
    let second = AgentSession(
        id: secondID,
        kind: .shell,
        logDirectory: logs,
        holderDirectory: holders)
    try await first.start(argv: ["/bin/cat"], cwd: "/tmp", extraEnv: [:]) { _, _ in }
    try await second.start(argv: ["/bin/cat"], cwd: "/tmp", extraEnv: [:]) { _, _ in }

    try await waitUntil(timeout: .seconds(5)) {
        let firstRunning = await first.isRunning
        let secondRunning = await second.isRunning
        return firstRunning && secondRunning
    }
    let firstPaths = HolderPaths(directory: holders, sessionID: firstID.rawValue)
    let secondPaths = HolderPaths(directory: holders, sessionID: secondID.rawValue)
    let firstManagerPID = try String(contentsOf: firstPaths.pidFile, encoding: .utf8)
        .trimmingCharacters(in: .whitespacesAndNewlines)
    let secondManagerPID = try String(contentsOf: secondPaths.pidFile, encoding: .utf8)
        .trimmingCharacters(in: .whitespacesAndNewlines)
    #expect(firstManagerPID == secondManagerPID)

    let firstChild = await first.pid
    let secondChild = await second.pid
    #expect(firstChild > 1)
    #expect(secondChild > 1)
    #expect(firstChild != secondChild)

    await first.terminate(graceSeconds: 0.2)
    try await waitUntil(timeout: .seconds(5)) { await first.isRunning == false }
    #expect(await second.isRunning)

    await second.terminate(graceSeconds: 0.2)
    try await waitUntil(timeout: .seconds(5)) { await second.isRunning == false }
}

/// Regression (remote revive mislabeled "exited: signaled 9"): the per-session
/// log survives relaunches under the same session id, so it still contains the
/// PREVIOUS incarnation's exit marker (written when migrate/terminate killed
/// the old agent). Replaying it must never be attributed to the new child —
/// the revived session stays running and its own exit still lands.
@Test func relaunchSameSessionIgnoresPriorIncarnationExitMarker() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-relaunch-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }
    let id = SessionID(rawValue: "s_relaunch")

    // First incarnation: killed, so the holder writes a signaled exit marker
    // into the shared <sessionID>.bin log.
    let firstExit = Expectation()
    let first = AgentSession(id: id, kind: .shell, logDirectory: dir)
    try await first.start(argv: ["/bin/cat"], cwd: "/tmp", extraEnv: [:]) { _, info in
        firstExit.fulfill(with: info)
    }
    try await waitUntil(timeout: .seconds(5)) { await first.isRunning }
    await first.terminate(graceSeconds: 0.2)
    _ = try await firstExit.value(timeout: .seconds(5))

    // Second incarnation, SAME session id: the old marker is replayed for
    // screen content only. Before the fix this exit fired within ~2ms.
    let secondExit = Expectation()
    let second = AgentSession(id: id, kind: .shell, logDirectory: dir)
    try await second.start(argv: ["/bin/cat"], cwd: "/tmp", extraEnv: [:]) { _, info in
        secondExit.fulfill(with: info)
    }
    try await waitUntil(timeout: .seconds(5)) { await second.isRunning }
    try await Task.sleep(for: .seconds(1))
    #expect(await second.exitInfo == nil)
    #expect(await second.isRunning)

    // The revived incarnation is genuinely usable...
    await second.write(Data("second-life\r".utf8))
    try await waitUntil(timeout: .seconds(5)) {
        await second.screenText().contains("second-life")
    }

    // ...and its OWN exit is still detected.
    await second.terminate(graceSeconds: 0.2)
    let info = try await secondExit.value(timeout: .seconds(5))
    #expect(info.reason == .signaled || info.reason == .exited)
}

/// Idempotent revive: resuming a session whose holder is still alive (e.g. a
/// revive previously mislabeled exited) must ADOPT the survivor, not spawn a
/// second holder — a second one interleaves two writers into one log and
/// stacks another ssh client onto the same remote tmux.
@Test func relaunchAdoptsLiveHolderInsteadOfSpawningASecond() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-adopt-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    let config = DaemonConfig(
        socketPath: dir.appendingPathComponent("d.sock").path,
        cliPath: "/usr/bin/true",
        injectDir: dir,
        logsDir: dir.appendingPathComponent("logs"),
        stateFile: dir.appendingPathComponent("state.json")
    )

    // Spawn a live session, then drop the registry while its holder survives.
    let record: SessionRecord
    let childPID: pid_t
    do {
        let registry = SessionRegistry(config: config, events: EventBus())
        record = try await registry.spawn(
            SessionSpawnParams(kind: .shell, cwd: "/tmp", title: "survivor"))
        let session = try #require(await registry.session(record.id))
        try await waitUntil(timeout: .seconds(5)) { await session.isRunning }
        childPID = await session.pid
        await registry.prepareShutdown()
    }

    // Rewrite the persisted record as exited (the mislabel the live system
    // exhibited) so restore does NOT adopt the holder and the record presents
    // exactly like a false-exit revive.
    let store = PersistenceStore(fileURL: config.stateFile)
    var state = try #require(await store.load())
    state.sessions = state.sessions.map { session in
        var session = session
        if session.id == record.id {
            session.status = .exited(ExitInfo(reason: .signaled, signal: 9))
        }
        return session
    }
    await store.schedule(state)
    await store.flushNow()

    let registry2 = SessionRegistry(config: config, events: EventBus())
    await registry2.restoreFromDisk()
    #expect(await registry2.session(record.id) == nil)

    // Revive under the same id. The argv would exit immediately if it were
    // actually spawned — adoption must reuse the surviving holder instead.
    let revived = try await registry2.relaunch(
        sessionID: record.id,
        argv: ["/bin/sh", "-c", "exit 0"],
        extraEnv: [:],
        spawnCwd: "/tmp")
    #expect(revived.status.isRunning)
    let adopted = try #require(await registry2.session(record.id))
    #expect(await adopted.pid == childPID)
    try await Task.sleep(for: .seconds(1))
    #expect(await adopted.isRunning)

    try await registry2.kill(sessionID: record.id)
    try await waitUntil(timeout: .seconds(5)) { await adopted.isRunning == false }
}

@Test func restoreDowngradesResumabilityWhenTranscriptIsGone() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-resume-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    let config = DaemonConfig(
        socketPath: dir.appendingPathComponent("d.sock").path,
        cliPath: "/usr/bin/true",
        injectDir: dir,
        logsDir: dir,
        stateFile: dir.appendingPathComponent("state.json")
    )

    let existingTranscript = dir.appendingPathComponent("has-transcript.jsonl")
    try Data("{}\n".utf8).write(to: existingTranscript)

    func claudeRecord(_ agentID: String, transcript: String?) -> SessionRecord {
        SessionRecord(
            kind: .claudeCode, cwd: "/tmp", projectID: ProjectID(root: "/tmp"),
            title: "t", agentSessionID: agentID, transcriptPath: transcript,
            status: .working, resumability: .live)
    }
    // A session whose transcript never reached disk (e.g. spawned by a daemon
    // poisoned with CLAUDE_CODE_CHILD_SESSION=1), one with a real transcript,
    // and a codex session with no transcript path to verify.
    let ghost = claudeRecord("ghost-id", transcript: dir.appendingPathComponent("never-written.jsonl").path)
    let real = claudeRecord("real-id", transcript: existingTranscript.path)
    var codex = SessionRecord(
        kind: .codex, cwd: "/tmp", projectID: ProjectID(root: "/tmp"), title: "c",
        status: .working, resumability: .live)
    codex.agentSessionID = "thread-1"

    let store = PersistenceStore(fileURL: config.stateFile)
    await store.schedule(
        PersistedState(projects: [Project(root: "/tmp")], sessions: [ghost, real, codex]))
    await store.flushNow()

    let registry = SessionRegistry(config: config, events: EventBus())
    await registry.restoreFromDisk()

    let ghostRestored = try #require(await registry.record(ghost.id))
    #expect(ghostRestored.resumability == .transcriptMissing)
    let realRestored = try #require(await registry.record(real.id))
    #expect(realRestored.resumability == .resumable)
    let codexRestored = try #require(await registry.record(codex.id))
    #expect(codexRestored.resumability == .resumable)

    // Resuming the ghost fails with an honest error, not a doomed spawn.
    await #expect(throws: ControlError.self) {
        try await registry.resume(sessionID: ghost.id)
    }
}

@Test func archiveKillsLiveSessionAndKeepsRecord() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-arch-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    let config = DaemonConfig(
        socketPath: dir.appendingPathComponent("d.sock").path,
        cliPath: "/usr/bin/true",
        injectDir: dir,
        logsDir: dir,
        stateFile: dir.appendingPathComponent("state.json")
    )

    let registry = SessionRegistry(config: config, events: EventBus())
    let record = try await registry.spawn(
        SessionSpawnParams(kind: .shell, cwd: "/tmp", title: "to archive"))
    #expect(await registry.session(record.id) != nil)

    try await registry.archive(sessionID: record.id)
    let archived = try #require(await registry.record(record.id))
    #expect(archived.isArchived)
    #expect(!archived.status.isRunning)
    #expect(exitReason(of: archived) == .archived)
    #expect(await registry.session(record.id) == nil)

    // The tree's eventual real death must not rewrite the archived reason.
    try await Task.sleep(for: .seconds(1))
    let settled = try #require(await registry.record(record.id))
    #expect(settled.isArchived)
    #expect(exitReason(of: settled) == .archived)

    try await registry.unarchive(sessionID: record.id)
    let unarchived = try #require(await registry.record(record.id))
    #expect(!unarchived.isArchived)
    #expect(!unarchived.status.isRunning)

    // Archiving an already-exited session just stamps archivedAt.
    try await registry.archive(sessionID: record.id)
    let rearchived = try #require(await registry.record(record.id))
    #expect(rearchived.isArchived)
    #expect(await registry.session(record.id) == nil)

    // Archived records survive a daemon restart untouched.
    await registry.prepareShutdown()
    let registry2 = SessionRegistry(config: config, events: EventBus())
    await registry2.restoreFromDisk()
    let restored = try #require(await registry2.record(record.id))
    #expect(restored.isArchived)
    #expect(exitReason(of: restored) == .archived)
}

private func exitReason(of record: SessionRecord) -> ExitReason? {
    if case .exited(let info) = record.status { return info.reason }
    return nil
}

/// Thread-safe recorder for remoteActive callback flips.
final class RemoteActiveRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var _values: [Bool] = []

    func record(_ value: Bool) {
        lock.lock()
        _values.append(value)
        lock.unlock()
    }

    var values: [Bool] {
        lock.lock()
        defer { lock.unlock() }
        return _values
    }
}

// MARK: - Async test helpers

struct TimeoutError: Error {}

/// Multiplier applied to every `waitUntil` deadline.
///
/// These waits are wall-clock liveness checks — "has the socket come up", "has
/// the child spawned" — not performance assertions. Five seconds is lavish on a
/// developer Mac and tight on a shared CI runner, where a process spawn or a
/// socket bind can take seconds under load. Scale them all from one place
/// instead of tuning 47 call sites; CI sets `DIRIJOR_TEST_TIMEOUT_SCALE`.
let testTimeoutScale = ProcessInfo.processInfo.environment["DIRIJOR_TEST_TIMEOUT_SCALE"]
    .flatMap(Double.init) ?? 1

func waitUntil(
    timeout: Duration, interval: Duration = .milliseconds(50),
    _ condition: @escaping @Sendable () async -> Bool
) async throws {
    let deadline = ContinuousClock.now + timeout * testTimeoutScale
    while ContinuousClock.now < deadline {
        if await condition() { return }
        try await Task.sleep(for: interval)
    }
    throw TimeoutError()
}

/// Tiny single-value expectation for exit callbacks.
final class Expectation: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: ExitInfo?

    func fulfill(with info: ExitInfo) {
        lock.lock()
        stored = info
        lock.unlock()
    }

    private func current() -> ExitInfo? {
        lock.lock()
        defer { lock.unlock() }
        return stored
    }

    func value(timeout: Duration) async throws -> ExitInfo {
        let deadline = ContinuousClock.now + timeout
        while ContinuousClock.now < deadline {
            if let value = current() { return value }
            try await Task.sleep(for: .milliseconds(50))
        }
        throw TimeoutError()
    }
}
