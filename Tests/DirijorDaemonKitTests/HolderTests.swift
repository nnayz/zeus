import Darwin
import DirijorHolderKit
import Foundation
import Testing

@testable import DirijorDaemonKit

/// Runs a blocking server body on a dedicated thread, reported as a `Task`.
///
/// `HolderServer.run()` and `HolderManagerServer.run()` sit in `accept()` for
/// their whole lifetime. Launched with `Task.detached` they each occupy one
/// cooperative-pool thread — and the pool is only as wide as the machine has
/// cores. On a 3-core CI runner a handful of live holders exhausts it, after
/// which a newly spawned server never gets scheduled at all: its socket never
/// appears and whatever waits on it times out. A developer Mac has enough
/// cores to mask this entirely. A real thread costs nothing here and cannot
/// starve the pool.
func runBlockingServer(_ body: @escaping @Sendable () throws -> Void) -> Task<Void, Error> {
    Task {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            let thread = Thread {
                do {
                    try body()
                    continuation.resume()
                } catch {
                    continuation.resume(throwing: error)
                }
            }
            thread.stackSize = 1 << 20
            thread.start()
        }
    }
}

@Test func holderWritesExitMarkerAndCleansControlFiles() async throws {
    let fixture = try HolderFixture(
        name: "exit",
        argv: ["/bin/sh", "-c", "printf holder-exit-test"])
    defer { fixture.cleanup() }

    let task = fixture.run()
    try await waitUntil(timeout: .seconds(5)) {
        FileManager.default.fileExists(atPath: fixture.logURL.path)
            && !FileManager.default.fileExists(atPath: fixture.paths.socket.path)
    }
    try await task.value

    let bytes = try Data(contentsOf: fixture.logURL)
    #expect(bytes.count > HolderOutputLog.headerSize)
    let stream = Data(bytes.dropFirst(HolderOutputLog.headerSize))
    #expect(stream.range(of: Data("holder-exit-test".utf8)) != nil)
    #expect(stream.range(of: HolderExitMarker.prefix) != nil)
    #expect(!FileManager.default.fileExists(atPath: fixture.paths.pidFile.path))

    var markerBytes = stream
    let drained = HolderExitMarker.drain(&markerBytes)
    #expect(drained.exit?.reason == .exited)
    #expect(drained.exit?.code == 0)
}

@Test func holderResizeReportsKernelPTYSize() async throws {
    let fixture = try HolderFixture(name: "resize", argv: ["/bin/cat"])
    defer { fixture.cleanup() }
    let task = fixture.run()
    let client = HolderClient(socketPath: fixture.paths.socket.path)

    try await waitUntil(timeout: .seconds(5)) { client.isAlive() }
    try client.resize(cols: 91, rows: 37)
    let stat = try client.stat()
    #expect(stat.cols == 91)
    #expect(stat.rows == 37)

    try client.killTree()
    try await task.value
}

@Test func holderKillTreeKillsBackgroundDescendants() async throws {
    let fixture = try HolderFixture(
        name: "tree",
        argv: ["/bin/sh", "-c", "sleep 30 & sleep 30 & wait"])
    defer { fixture.cleanup() }
    let task = fixture.run()
    let client = HolderClient(socketPath: fixture.paths.socket.path)

    try await waitUntil(timeout: .seconds(5)) {
        guard let stat = try? client.stat() else { return false }
        return ProcessTree.enumerate(root: stat.childPID).count >= 3
    }
    let root = try client.stat().childPID
    let tree = ProcessTree.enumerate(root: root)
    #expect(tree.count >= 3)

    try client.killTree()
    try await task.value
    try await waitUntil(timeout: .seconds(5)) {
        tree.allSatisfy { ProcessTree.startTime($0.pid) != $0.startSec }
    }
    #expect(!FileManager.default.fileExists(atPath: fixture.paths.socket.path))
}

/// The per-session log survives relaunches under the same session id, so a
/// second holder incarnation must report where ITS bytes begin: exit markers
/// below that epoch belong to a previous child and the daemon must never
/// attribute them to the new one (the "revive instantly mislabeled signaled 9"
/// bug).
@Test func holderStatReportsEpochOffsetOfItsOwnIncarnation() async throws {
    let fixture = try HolderFixture(
        name: "epoch",
        argv: ["/bin/sh", "-c", "printf first-life"])
    defer { fixture.cleanup() }

    // First incarnation runs to completion, leaving output + exit marker.
    try await fixture.run().value
    let priorBytes = try Data(contentsOf: fixture.logURL).count - HolderOutputLog.headerSize
    #expect(priorBytes > 0)

    // Second incarnation on the same session id / log file.
    let task = fixture.run(argv: ["/bin/cat"])
    let client = HolderClient(socketPath: fixture.paths.socket.path)
    try await waitUntil(timeout: .seconds(5)) { client.isAlive() }
    let stat = try client.stat()
    #expect(stat.epochOffset == UInt64(priorBytes))

    try client.killTree()
    try await task.value
}

/// Two holders for one session would interleave two writers into one output
/// log and stack a second child (another ssh client onto the same remote
/// tmux). A holder that finds a live sibling on its socket must refuse to run
/// — before spawning anything.
@Test func holderRefusesToDoubleRunOnALiveSocket() async throws {
    let fixture = try HolderFixture(name: "dbl", argv: ["/bin/cat"])
    defer { fixture.cleanup() }
    let task = fixture.run()
    let client = HolderClient(socketPath: fixture.paths.socket.path)
    try await waitUntil(timeout: .seconds(5)) { client.isAlive() }
    let firstChild = try client.stat().childPID

    let second = HolderServer(spec: fixture.makeSpec(argv: ["/bin/cat"]))
    #expect(throws: HolderError.self) { try second.run() }

    // The survivor is untouched and still serves its original child.
    let stat = try client.stat()
    #expect(stat.alive)
    #expect(stat.childPID == firstChild)

    try client.killTree()
    try await task.value
}

@Test func holderManagerHostsIndependentSessionsInOneProcess() async throws {
    let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("dh-manager-\(UUID().uuidString.prefix(8))")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }

    let managerPaths = HolderManagerPaths(directory: directory)
    // The manager arms its idle-shutdown timer the moment it starts listening,
    // so this timeout has to outlast everything between here and the first
    // launch below — the socket coming up, two fixtures being built, two child
    // processes spawning. At 0.1s it raced on CI: the manager shut itself down
    // before the first isAlive() poll, and no amount of waiting brought back a
    // process that had already exited. The tail of the test still asserts the
    // idle shutdown, just on a deadline a loaded runner can meet.
    let manager = HolderManagerServer(
        directory: directory, idleTimeout: 2 * testTimeoutScale)
    let managerTask = runBlockingServer { try manager.run() }
    let managerClient = HolderManagerClient(socketPath: managerPaths.socket.path)
    try await waitUntil(timeout: .seconds(5)) { managerClient.isAlive() }

    let first = try HolderFixture(name: "managed-a", sessionID: "a", argv: ["/bin/cat"])
    let second = try HolderFixture(name: "managed-b", sessionID: "b", argv: ["/bin/cat"])
    defer {
        first.cleanup()
        second.cleanup()
    }
    // Manager validation deliberately requires all session control paths to
    // live beneath its directory.
    let firstSpec = first.makeSpec(in: directory)
    let secondSpec = second.makeSpec(in: directory)
    let firstClient = HolderClient(socketPath: firstSpec.socketPath)
    let secondClient = HolderClient(socketPath: secondSpec.socketPath)

    let firstManagerPID = try managerClient.launch(firstSpec)
    let secondManagerPID = try managerClient.launch(secondSpec)
    #expect(firstManagerPID == getpid())
    #expect(secondManagerPID == firstManagerPID)

    try await waitUntil(timeout: .seconds(5)) {
        firstClient.isAlive() && secondClient.isAlive()
    }
    let firstChild = try firstClient.stat().childPID
    let secondChild = try secondClient.stat().childPID
    #expect(firstChild != secondChild)

    try firstClient.killTree()
    try await waitUntil(timeout: .seconds(5)) { !firstClient.isAlive() }
    #expect(secondClient.isAlive())

    try secondClient.killTree()
    try await managerTask.value
    #expect(!FileManager.default.fileExists(atPath: managerPaths.socket.path))
    #expect(!FileManager.default.fileExists(atPath: managerPaths.pidFile.path))
}

private final class HolderFixture: @unchecked Sendable {
    let directory: URL
    let paths: HolderPaths
    let logURL: URL
    let spec: HolderLaunchSpec

    init(name: String, sessionID: String = "s", argv: [String]) throws {
        directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("dh-\(name)-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        paths = HolderPaths(directory: directory, sessionID: sessionID)
        logURL = directory.appendingPathComponent("\(sessionID).bin")
        spec = HolderLaunchSpec(
            sessionID: sessionID,
            socketPath: paths.socket.path,
            pidFilePath: paths.pidFile.path,
            logFilePath: logURL.path,
            argv: argv,
            cwd: "/tmp",
            environment: ["PATH": "/usr/bin:/bin", "TERM": "xterm-256color"],
            cols: 80,
            rows: 24)
    }

    /// The same session's paths with a different child — a new incarnation.
    func makeSpec(argv: [String]) -> HolderLaunchSpec {
        var next = spec
        next.argv = argv
        return next
    }

    func makeSpec(in managerDirectory: URL) -> HolderLaunchSpec {
        let managedPaths = HolderPaths(directory: managerDirectory, sessionID: spec.sessionID)
        var managed = spec
        managed.socketPath = managedPaths.socket.path
        managed.pidFilePath = managedPaths.pidFile.path
        return managed
    }

    func run(argv: [String]? = nil) -> Task<Void, Error> {
        let server = HolderServer(spec: argv.map(makeSpec) ?? spec)
        return runBlockingServer { try server.run() }
    }

    func cleanup() {
        let client = HolderClient(socketPath: paths.socket.path)
        try? client.killTree()
        try? FileManager.default.removeItem(at: directory)
    }
}
