import DirijorCore
import Foundation
import Testing

@testable import DirijorProtocol

@Test func hostsConfigParsesDocumentedSchema() throws {
    let json = """
        {
          "hosts": [
            {
              "id": "forge", "name": "Forge", "ssh": "cristi@forge", "defaultCwd": "~/code",
              "node": {
                "endpoint": "tcp://100.64.0.2:7337",
                "tokenFile": "~/.config/dirijor/forge.token",
                "nodeId": "node-forge"
              }
            },
            { "id": "studio", "name": "Studio Mac", "ssh": "studio.local" }
          ]
        }
        """
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("hosts-test-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }
    let file = dir.appendingPathComponent("hosts.json")
    try Data(json.utf8).write(to: file)

    let config = try #require(HostsConfig.load(from: file))
    #expect(config.hosts.count == 2)
    let host = try #require(config.host(id: "forge"))
    #expect(host.displayName == "Forge")
    #expect(host.ssh == "cristi@forge")
    #expect(host.defaultCwd == "~/code")
    #expect(host.node?.endpoint == "tcp://100.64.0.2:7337")
    #expect(host.node?.tokenFile == "~/.config/dirijor/forge.token")
    #expect(host.node?.nodeID == "node-forge")
    let studio = try #require(config.host(id: "studio"))
    #expect(studio.displayName == "Studio Mac")
    #expect(studio.ssh == "studio.local")
    #expect(studio.defaultCwd == nil)
    #expect(studio.node == nil)
    #expect(config.host(id: "unknown") == nil)

    // Missing and malformed files mean "no hosts", never an error.
    #expect(HostsConfig.load(from: dir.appendingPathComponent("missing.json")) == nil)
    let malformed = dir.appendingPathComponent("bad.json")
    try Data("not json".utf8).write(to: malformed)
    #expect(HostsConfig.load(from: malformed) == nil)
}

@Test func hostEntryMinimalSchemaAndDisplayNameFallback() throws {
    let json = #"{ "hosts": [{ "id": "builder", "ssh": "root@1.2.3.4" }] }"#
    let config = try JSONDecoder().decode(HostsConfig.self, from: Data(json.utf8))
    let host = try #require(config.host(id: "builder"))
    #expect(host.displayName == "builder")
    #expect(host.defaultCwd == nil)
}

@Test func spawnParamsHostFieldIsWireCompatible() throws {
    // Legacy payload without the key decodes to nil.
    let legacy = try JSONDecoder().decode(
        SessionSpawnParams.self,
        from: Data(#"{"kind":{"shell":{}},"cwd":"/tmp"}"#.utf8))
    #expect(legacy.host == nil)
    // nil host is omitted from the encoded payload (old daemons never see it).
    let localData = try JSONEncoder().encode(legacy)
    let localObject = try #require(
        try JSONSerialization.jsonObject(with: localData) as? [String: Any])
    #expect(localObject["host"] == nil)

    let remote = SessionSpawnParams(kind: .claudeCode, cwd: "~/code", host: "forge")
    let data = try JSONEncoder().encode(remote)
    let decoded = try JSONDecoder().decode(SessionSpawnParams.self, from: data)
    #expect(decoded.host == "forge")
}

@Test func sessionRecordHostFieldSurvivesRoundTripAndLegacyDecode() throws {
    let record = SessionRecord(
        kind: .claudeCode, cwd: "~/code", projectID: ProjectID(root: "~/code"),
        title: "Remote", host: "forge")
    let data = try JSONEncoder().encode(record)
    let decoded = try JSONDecoder().decode(SessionRecord.self, from: data)
    #expect(decoded.host == "forge")

    // A record persisted before the field existed decodes with host == nil.
    var object = try #require(
        try JSONSerialization.jsonObject(with: data) as? [String: Any])
    object.removeValue(forKey: "host")
    let legacyData = try JSONSerialization.data(withJSONObject: object)
    let legacy = try JSONDecoder().decode(SessionRecord.self, from: legacyData)
    #expect(legacy.host == nil)
}
