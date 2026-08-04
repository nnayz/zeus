import DirijorCore
import Foundation
import Testing

@testable import DirijorProtocol

@Test func controlMessageRoundTrip() throws {
    let params = try JSONValue(encoding: SessionSpawnParams(kind: .claudeCode, cwd: "/tmp/x"))
    let request = ControlMessage.request(id: 7, method: Method.sessionSpawn, params: params)
    let encoded = try NDJSONBuffer.encode(request)

    var buffer = NDJSONBuffer()
    let decoded = try buffer.append(encoded)
    #expect(decoded.count == 1)
    guard case .request(let id, let method, let p) = decoded[0] else {
        Issue.record("expected request")
        return
    }
    #expect(id == 7)
    #expect(method == Method.sessionSpawn)
    let spawn: SessionSpawnParams = try #require(p).decoded()
    #expect(spawn.kind == .claudeCode)
    #expect(spawn.cwd == "/tmp/x")
}

@Test func controlMessagePartialLinesAndBatching() throws {
    let messages: [ControlMessage] = [
        .response(id: 1, result: .success(.object(["a": .number(1)]))),
        .response(id: 2, result: .failure(.notFound("session"))),
        .event(name: EventName.sessionRemoved, seq: 42, params: .object(["id": .string("s_x")])),
    ]
    var wire = Data()
    for message in messages { wire.append(try NDJSONBuffer.encode(message)) }

    var buffer = NDJSONBuffer()
    var decoded: [ControlMessage] = []
    // Feed one byte at a time to exercise partial-line accumulation.
    for byte in wire {
        decoded.append(contentsOf: try buffer.append(Data([byte])))
    }
    #expect(decoded.count == 3)
    guard case .response(_, .failure(let err)) = decoded[1] else {
        Issue.record("expected error response")
        return
    }
    #expect(err.code == "not_found")
    guard case .event(let name, let seq, _) = decoded[2] else {
        Issue.record("expected event")
        return
    }
    #expect(name == EventName.sessionRemoved)
    #expect(seq == 42)
}

@Test func frameCodecRoundTripAndFragmentation() throws {
    let frames: [Frame] = [
        .output(offset: 123_456_789, bytes: Data("hello \u{1B}[31mworld\u{1B}[0m".utf8)),
        .input(Data("ls -la\r".utf8)),
        .resize(cols: 120, rows: 40),
        .replayBegin(offset: 0),
        .replayEnd(offset: 987_654),
        .scroll(dir: 1, lines: 3, col: 42, row: 7),
        Frame(type: .ping),
    ]
    var wire = Data()
    for frame in frames { wire.append(FrameCodec.encode(frame)) }

    var codec = FrameCodec()
    var decoded: [Frame] = []
    // Feed in awkward 3-byte chunks to exercise reassembly.
    var index = wire.startIndex
    while index < wire.endIndex {
        let end = wire.index(index, offsetBy: 3, limitedBy: wire.endIndex) ?? wire.endIndex
        decoded.append(contentsOf: try codec.append(wire.subdata(in: index..<end)))
        index = end
    }

    #expect(decoded == frames)
    let out = try #require(decoded[0].outputPayload)
    #expect(out.offset == 123_456_789)
    let resize = try #require(decoded[2].resizePayload)
    #expect(resize.cols == 120 && resize.rows == 40)
    #expect(decoded[4].offsetPayload == 987_654)
    let scroll = try #require(decoded[5].scrollPayload)
    #expect(scroll.dir == 1 && scroll.lines == 3 && scroll.col == 42 && scroll.row == 7)
}

@Test func sessionRecordWireRoundTrip() throws {
    var record = SessionRecord(
        kind: .codex,
        cwd: "/Users/x/proj",
        projectID: ProjectID(root: "/Users/x/proj"),
        title: "Bump deps"
    )
    record.status = .needsInput(.permission)
    record.needsInput = NeedsInputDetail(
        kind: .permission, source: .screenScrape, summary: "Allow command? `git push`"
    )
    record.remoteActive = true
    // One hop through the coder normalizes Date precision (ms on the wire);
    // a normalized value must then round-trip losslessly.
    let normalized: SessionRecord = try JSONValue(encoding: record).decoded()
    let back: SessionRecord = try JSONValue(encoding: normalized).decoded()
    #expect(back == normalized)
    #expect(back.id == record.id && back.status == record.status && back.title == record.title)
    #expect(back.remoteActive)
}

@Test func sessionReadDiffWireRoundTrip() throws {
    let params = SessionReadDiffParams(
        sessionID: SessionID(rawValue: "s_remote"), base: .head)
    let encodedParams = try JSONValue(encoding: params)
    let request = ControlMessage.request(
        id: 8, method: Method.sessionReadDiff, params: encodedParams)

    var buffer = NDJSONBuffer()
    let decoded = try buffer.append(NDJSONBuffer.encode(request))
    let decodedRequest = try #require(decoded.first)
    guard case .request(_, let method, let payload) = decodedRequest else {
        Issue.record("expected diff request")
        return
    }
    #expect(method == "session.read_diff")
    let roundTripped: SessionReadDiffParams = try #require(payload).decoded()
    #expect(roundTripped.sessionID == params.sessionID)
    #expect(roundTripped.base == .head)

    let result = SessionReadDiffResult(
        patch: Data("diff --git a/a.txt b/a.txt\n+hello\n".utf8),
        repoRoot: "/srv/app",
        truncated: false,
        baseRef: "origin/main")
    let roundTrippedResult: SessionReadDiffResult = try JSONValue(encoding: result).decoded()
    #expect(roundTrippedResult.patch == result.patch)
    #expect(roundTrippedResult.repoRoot == "/srv/app")
    #expect(roundTrippedResult.baseRef == "origin/main")
    #expect(!roundTrippedResult.truncated)
}
