import DirijorCore
import DirijorProtocol
import Foundation
import Testing

@testable import DirijorDaemonKit

/// Pulls exactly `count` events, so a test never hangs on a stream that stays
/// open by design (a subscription only ends when its subscriber goes away).
private func drain(_ stream: AsyncStream<EventBus.Event>, count: Int) async -> [EventBus.Event] {
    var iterator = stream.makeAsyncIterator()
    var events: [EventBus.Event] = []
    for _ in 0..<count {
        guard let event = await iterator.next() else { break }
        events.append(event)
    }
    return events
}

@Test func eventBusFansOutToEverySubscriber() async {
    let bus = EventBus()
    let first = await bus.subscribe()
    let second = await bus.subscribe()
    let third = await bus.subscribe()

    await bus.publish(name: EventName.sessionOutput, params: .string("x"))
    await bus.publish(name: EventName.sessionStatus, params: .string("y"))

    for stream in [first, second, third] {
        let got = await drain(stream, count: 2)
        #expect(got.map(\.name) == [EventName.sessionOutput, EventName.sessionStatus])
        #expect(got.map(\.seq) == [1, 2])
    }
}

@Test func eventBusFiltersBySessionAndKindServerSide() async {
    let bus = EventBus()
    let a = SessionID(rawValue: "s_a")
    let b = SessionID(rawValue: "s_b")

    let onlyA = await bus.subscribe(filter: EventBus.Filter(sessions: [a]))
    let onlyStatus = await bus.subscribe(filter: EventBus.Filter(kinds: [EventName.sessionStatus]))
    let everything = await bus.subscribe()

    await bus.publish(name: EventName.sessionStatus, params: .null, sessionID: a)
    await bus.publish(name: EventName.sessionOutput, params: .null, sessionID: b)
    await bus.publish(name: EventName.worktreeCreated, params: .null)
    await bus.publish(name: EventName.sessionOutput, params: .null, sessionID: a)

    // A session filter also suppresses events that belong to no session —
    // asking for one session and getting every worktree event would defeat it.
    let seenByA = await drain(onlyA, count: 2)
    #expect(seenByA.map(\.seq) == [1, 4])

    let seenByStatus = await drain(onlyStatus, count: 1)
    #expect(seenByStatus.map(\.seq) == [1])

    let seenByAll = await drain(everything, count: 4)
    #expect(seenByAll.map(\.seq) == [1, 2, 3, 4])
}

@Test func eventBusReplayHonorsTheFilter() async {
    let bus = EventBus(ringCapacity: 32)
    let a = SessionID(rawValue: "s_a")
    await bus.publish(name: EventName.sessionStatus, params: .null, sessionID: a)
    await bus.publish(name: EventName.sessionOutput, params: .null, sessionID: a)
    await bus.publish(name: EventName.sessionStatus, params: .null, sessionID: SessionID(rawValue: "s_b"))

    let stream = await bus.subscribe(
        sinceSeq: 0, filter: EventBus.Filter(sessions: [a], kinds: [EventName.sessionStatus]))
    let replayed = await drain(stream, count: 1)
    #expect(replayed.map(\.seq) == [1])
}

/// The backpressure contract: a subscriber that stops reading is bounded, never
/// blocks the publisher or its peers, and is told the exact seq range it lost
/// once it catches up.
@Test func eventBusDropsOldestForASlowSubscriberAndMarksTheGap() async {
    let bus = EventBus(ringCapacity: 4, subscriberCapacity: 4)
    let slow = await bus.subscribe()

    for index in 0..<10 {
        await bus.publish(name: "e\(index)", params: .null)
    }

    var iterator = slow.makeAsyncIterator()
    var received: [String] = []
    for _ in 0..<4 {
        guard let event = await iterator.next() else { break }
        received.append(event.name)
    }
    // The four NEWEST survive: current state matters more than history.
    #expect(received == ["e6", "e7", "e8", "e9"])

    // The marker rides out on the first enqueue that no longer evicts, i.e.
    // once the consumer has caught up.
    await bus.publish(name: "e10", params: .null)
    #expect(await iterator.next()?.name == "e10")

    let marker = await iterator.next()
    #expect(marker?.name == EventName.eventsDropped)
    // seq 0 is outside the published seq space, so a consumer tracking lastSeq
    // for gapless resume can ignore the marker without special-casing it.
    #expect(marker?.seq == 0)
    let payload = try? #require(marker?.params).decoded(as: EventsDroppedEvent.self)
    #expect(payload?.dropped == 6)
    #expect(payload?.fromSeq == 1)
    #expect(payload?.toSeq == 6)

    // And exactly one marker per burst, not one per lost event.
    await bus.publish(name: "e11", params: .null)
    #expect(await iterator.next()?.name == "e11")
}

@Test func eventBusSlowSubscriberDoesNotStallOthers() async {
    let bus = EventBus(ringCapacity: 8, subscriberCapacity: 8)
    let stalled = await bus.subscribe()  // deliberately never read
    let healthy = await bus.subscribe()

    // Far more than either queue can hold. A publisher that waited on the
    // stalled consumer would never reach the end of this loop.
    for index in 0..<5_000 {
        await bus.publish(name: "e\(index)", params: .null)
    }

    let seen = await drain(healthy, count: 8)
    #expect(seen.count == 8)
    #expect(seen.first?.name == "e4992")
    #expect(await bus.currentSeq == 5_000)
    withExtendedLifetime(stalled) {}
}

@Test func eventBusDropMarkerReachesEvenANarrowlyFilteredSubscriber() async {
    let bus = EventBus(ringCapacity: 4, subscriberCapacity: 2)
    let narrow = await bus.subscribe(filter: EventBus.Filter(kinds: [EventName.sessionStatus]))

    for _ in 0..<6 {
        await bus.publish(name: EventName.sessionStatus, params: .null)
    }
    var iterator = narrow.makeAsyncIterator()
    _ = await iterator.next()
    _ = await iterator.next()

    await bus.publish(name: EventName.sessionStatus, params: .null)
    #expect(await iterator.next()?.name == EventName.sessionStatus)
    // `events.dropped` is not in the filter, and must arrive anyway: a narrow
    // subscriber still has to learn that its narrow slice has a hole.
    #expect(await iterator.next()?.name == EventName.eventsDropped)
}

@Test func eventBusTerminatedSubscriberIsForgotten() async {
    let bus = EventBus()
    do {
        let stream = await bus.subscribe()
        #expect(await bus.subscriberCount == 1)
        _ = stream
    }
    // Termination is delivered asynchronously by the stream's deinit hook.
    for _ in 0..<50 where await bus.subscriberCount != 0 {
        try? await Task.sleep(for: .milliseconds(10))
    }
    #expect(await bus.subscriberCount == 0)
}

// MARK: - derived session events

/// The narrow events are derived from the registry's existing publish funnel,
/// not from a second notification path — this pins that they actually come out
/// of a real spawn/rename/archive sequence, in the right order.
@Test func registryDerivesNarrowEventsFromTheSamePublishFunnel() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-events-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }

    let config = DaemonConfig(
        socketPath: dir.appendingPathComponent("d.sock").path,
        cliPath: "/usr/bin/true",
        injectDir: dir,
        logsDir: dir,
        stateFile: dir.appendingPathComponent("state.json"))
    let bus = EventBus()
    let registry = SessionRegistry(config: config, events: bus)

    let record = try await registry.spawn(
        SessionSpawnParams(kind: .shell, cwd: "/tmp", title: "eventful"))
    try await registry.rename(sessionID: record.id, title: "renamed")
    try await registry.archive(sessionID: record.id)

    // Replay from the ring rather than racing the mutations, then bound the
    // read with a sentinel so the test can't hang on a stream that stays open.
    let stream = await bus.subscribe(
        sinceSeq: 0, filter: EventBus.Filter(sessions: [record.id]))
    await bus.publish(name: "test.sentinel", params: .null, sessionID: record.id)

    var events: [EventBus.Event] = []
    var iterator = stream.makeAsyncIterator()
    while let event = await iterator.next(), event.name != "test.sentinel" {
        events.append(event)
    }
    let names = events.map(\.name)

    // The coarse record event still leads, so anything reacting to a narrow
    // event and re-reading state sees the new record, never the old one.
    #expect(names.first == EventName.sessionUpdated)
    #expect(names.filter { $0 == EventName.sessionSpawned }.count == 1)
    #expect(names.contains(EventName.sessionArchived))

    // A rename changes no status and must not manufacture a transition.
    let transitions = try events
        .filter { $0.name == EventName.sessionStatus }
        .map { try $0.params.decoded(as: SessionStatusEvent.self) }
    let archived = try #require(transitions.last)
    #expect(archived.label == "exited:archived")
    #expect(archived.id == record.id)
    #expect(transitions.allSatisfy { $0.from != $0.to })

    try await registry.remove(sessionID: record.id)
    let after = await bus.subscribe(sinceSeq: 0, filter: EventBus.Filter(kinds: [EventName.sessionRemoved]))
    await bus.publish(name: EventName.sessionRemoved, encoding: SessionRemovedEvent(id: SessionID(rawValue: "sentinel")))
    var removals: [SessionRemovedEvent] = []
    var removalIterator = after.makeAsyncIterator()
    while let event = await removalIterator.next() {
        let decoded = try event.params.decoded(as: SessionRemovedEvent.self)
        if decoded.id.rawValue == "sentinel" { break }
        removals.append(decoded)
    }
    #expect(removals.map(\.id) == [record.id])
    #expect(removals.first?.reason == "released")
}

@Test func resourceSamplingPublishesACompactResourceEventNotAFullRecord() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-resource-events-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }
    let config = DaemonConfig(
        socketPath: dir.appendingPathComponent("d.sock").path,
        cliPath: "/usr/bin/true",
        injectDir: dir,
        logsDir: dir,
        stateFile: dir.appendingPathComponent("state.json"))
    let bus = EventBus()
    let registry = SessionRegistry(config: config, events: bus)
    let record = try await registry.spawn(
        SessionSpawnParams(kind: .shell, cwd: "/tmp", title: "sampled"))
    let before = await bus.currentSeq

    await registry.applyResourceSample(
        sessionID: record.id,
        memoryBytes: 42_000_000,
        ports: nil,
        artifacts: nil)
    let stream = await bus.subscribe(sinceSeq: before)
    await bus.publish(name: "test.sentinel", params: .null)
    var iterator = stream.makeAsyncIterator()
    let resource = try #require(await iterator.next())

    #expect(resource.name == EventName.sessionResources)
    #expect(resource.name != EventName.sessionUpdated)
    let payload = try resource.params.decoded(as: SessionResourcesEvent.self)
    #expect(payload.id == record.id)
    #expect(payload.memoryBytes == 42_000_000)
}

// MARK: - wait-target vocabulary

@Test func waitTargetsResolveTheAliasVocabulary() {
    #expect(SessionStatus.idle.satisfies(waitTarget: "done"))
    #expect(SessionStatus.idle.satisfies(waitTarget: "idle"))
    #expect(!SessionStatus.idle.satisfies(waitTarget: "working"))
    #expect(SessionStatus.needsInput(.permission).satisfies(waitTarget: "needs_me"))
    #expect(SessionStatus.needsInput(.question).satisfies(waitTarget: "needs-input"))
    #expect(SessionStatus.needsInput(.question).satisfies(waitTarget: "blocked"))
    #expect(SessionStatus.exited(ExitInfo(reason: .archived)).satisfies(waitTarget: "exited"))
    // An unknown target matches nothing rather than silently meaning "idle" —
    // the bug the MCP bridge's local alias switch used to have.
    #expect(!SessionStatus.idle.satisfies(waitTarget: "nonsense"))
}

@Test func statusLabelsAreStableAcrossTheWire() {
    #expect(SessionStatus.needsInput(.permission).label == "needsInput:permission")
    #expect(SessionStatus.exited(ExitInfo(reason: .archived)).label == "exited:archived")
    #expect(SessionStatus.working.label == "working")
}
