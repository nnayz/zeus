import Foundation
import Testing

@testable import DirijorCore

// MARK: - On-disk / on-wire compatibility
//
// `AgentKind` stopped being a Swift enum, but `SessionRecord` — which embeds it
// — is persisted to `~/Library/Application Support/Dirijor/state.json` and sent
// over the daemon protocol to a Rust client that may be older or newer than the
// daemon. These tests pin the encoding so a rework can't silently orphan every
// session on every user's machine.

/// State written by any build before the manifest-driven rework. Verbatim
/// shapes produced by Swift's synthesized `Codable` for the old enum.
private let legacyEncodings: [(json: String, expected: AgentKind)] = [
    (#"{"claudeCode":{}}"#, .claudeCode),
    (#"{"codex":{}}"#, .codex),
    (#"{"cursor":{}}"#, .cursor),
    (#"{"gemini":{}}"#, .gemini),
    (#"{"shell":{}}"#, .shell),
    (#"{"generic":{"command":"npm run dev"}}"#, .generic(command: "npm run dev")),
]

@Test func legacyAgentKindJSONStillDecodes() throws {
    for (json, expected) in legacyEncodings {
        let decoded = try JSONDecoder().decode(AgentKind.self, from: Data(json.utf8))
        #expect(decoded == expected, "\(json) decoded to \(decoded)")
    }
}

@Test func legacyAgentKindsReEncodeToTheirLegacyShape() throws {
    // Round-tripping must be byte-identical for the old kinds: a NEW daemon's
    // state file has to stay readable by an OLD daemon (downgrade / rollback),
    // and an old Rust client must keep matching on the same case names.
    for (json, kind) in legacyEncodings {
        let data = try JSONEncoder().encode(kind)
        let reparsed = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        let original = try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
        #expect(reparsed?.keys.first == original?.keys.first)
        let decoded = try JSONDecoder().decode(AgentKind.self, from: data)
        #expect(decoded == kind)
    }
}

@Test func manifestOnlyAgentsUseTheOpenCaseKey() throws {
    let amp = AgentKind(id: "amp")
    let data = try JSONEncoder().encode(amp)
    let object = try JSONSerialization.jsonObject(with: data) as? [String: Any]
    // An agent with no legacy enum case encodes under "agent". An older Rust
    // client's keyed-enum decoder falls through to its tolerant Unknown branch
    // rather than failing the whole session list.
    #expect(object?.keys.first == "agent")
    #expect(try JSONDecoder().decode(AgentKind.self, from: data) == amp)
}

@Test func unknownEncodingsDegradeInsteadOfThrowing() throws {
    // A case key from a future build: treat the key as the manifest id.
    let future = try JSONDecoder().decode(
        AgentKind.self, from: Data(#"{"someFutureAgent":{}}"#.utf8))
    #expect(future.id == "someFutureAgent")

    // Hand-written config / MCP arguments may use a bare string.
    #expect(try JSONDecoder().decode(AgentKind.self, from: Data(#""codex""#.utf8)) == .codex)
    #expect(try JSONDecoder().decode(AgentKind.self, from: Data(#""amp""#.utf8)).id == "amp")

    // "agent" with no id can't be recovered; degrade to a terminal rather than
    // failing the enclosing record — one bad kind must not lose a state file.
    let empty = try JSONDecoder().decode(AgentKind.self, from: Data(#"{"agent":{}}"#.utf8))
    #expect(empty.id == BuiltinAgentID.generic)
}

@Test func legacySessionRecordJSONDecodes() throws {
    // A whole record as an older build wrote it: legacy kind encoding, no
    // fields added since. Proves the migration works where it actually matters.
    let json = """
        {
          "id": "s_abc123",
          "kind": {"claudeCode": {}},
          "cwd": "/Users/giga/fun/dirijor",
          "projectID": "p_5c2f1a",
          "title": "Fix the sidebar",
          "titleSource": 2,
          "status": {"idle": {}},
          "resumability": "resumable",
          "createdAt": 774000000,
          "updatedAt": 774000100,
          "pinned": false,
          "foregroundAgent": {"codex": {}}
        }
        """
    let record = try JSONDecoder().decode(SessionRecord.self, from: Data(json.utf8))
    #expect(record.kind == .claudeCode)
    #expect(record.foregroundAgent == .codex)
    #expect(record.effectiveKind == .codex)
    #expect(record.title == "Fix the sidebar")
}

// MARK: - Catalog

@Test func catalogLoadsBundledManifests() {
    let catalog = AgentCatalog.shared
    // The four agents that predate the manifest-driven rework must keep their
    // exact identity — these ids appear in persisted state files.
    for id in [
        BuiltinAgentID.claudeCode, BuiltinAgentID.codex, BuiltinAgentID.cursor,
        BuiltinAgentID.gemini, BuiltinAgentID.shell, BuiltinAgentID.generic,
    ] {
        #expect(catalog.descriptors[id] != nil, "missing manifest for \(id)")
    }
    #expect(AgentKind.claudeCode.displayName == "Claude Code")
    #expect(AgentKind.claudeCode.isFirstClass)
    #expect(!AgentKind.shell.isFirstClass)
    #expect(AgentKind.claudeCode.descriptor.statusAuthority == .hooks)
    #expect(AgentKind.codex.descriptor.statusAuthority == .screen)

    // Widened breadth: agents added as pure data are fully described.
    let amp = catalog.descriptor(for: "amp")
    #expect(amp.binary == "amp")
    #expect(amp.firstClass)
}

@Test func catalogFallsBackForUnknownIDs() {
    let ghost = AgentKind(id: "not-an-agent")
    // Never crash on an id we don't have a manifest for (a record written by a
    // newer build, or a manifest the user deleted): degrade to a terminal.
    #expect(!ghost.isFirstClass)
    #expect(ghost.descriptor.statusAuthority == .process)
    #expect(ghost.descriptor.binary == nil)
    #expect(ghost.displayName == "Not An Agent")
}

@Test func catalogResolvesUserTypedNames() throws {
    let catalog = AgentCatalog.shared
    #expect(catalog.resolve(name: "claude")?.id == BuiltinAgentID.claudeCode)
    #expect(catalog.resolve(name: "claude-code")?.id == BuiltinAgentID.claudeCode)
    #expect(catalog.resolve(name: "Cursor-Agent")?.id == BuiltinAgentID.cursor)
    #expect(catalog.resolve(name: "open-code")?.id == "opencode")
    #expect(catalog.resolve(name: "nope") == nil)
}

@Test func envScrubPrefixesComeFromManifests() {
    // Precision matters: gemini declares GEMINI_CLI (its nesting marker) and
    // NOT GEMINI, because GEMINI_API_KEY is the user's own credential.
    let prefixes = AgentCatalog.shared.envScrubPrefixes
    #expect(prefixes.contains("CLAUDE"))
    #expect(prefixes.contains("CODEX"))
    #expect(prefixes.contains("GEMINI_CLI"))
    #expect(!prefixes.contains("GEMINI"))
    #expect(!prefixes.contains("CURSOR"))
}

@Test func resumeNeedsEitherAnIDSourceOrLatestSemantics() {
    // An id-carrying resume needs BOTH a command line and a way to learn the id.
    #expect(AgentKind.claudeCode.descriptor.canResume)  // --session-id + hooks
    #expect(AgentKind.codex.descriptor.canResume)  // notify reports the thread id
    #expect(AgentKind.gemini.descriptor.canResume)  // --session-id
    #expect(!AgentKind.shell.descriptor.canResume)

    // `.latest` needs no id, so requiring one demoted agents that resume fine.
    // Both of these were verified against the installed CLI's help output, not
    // inferred: `cursor-agent resume` ("Resume the latest chat session") and
    // `opencode --continue` ("continue the last session"). Cursor still cannot
    // do an id-targeted resume — its chat ids are minted server-side — which is
    // why the spec is `.latest` and not `.flag`.
    let cursor = AgentKind.cursor.descriptor
    #expect(cursor.canResume)
    #expect(cursor.resume?.style == .latest)
    #expect(cursor.resume?.argv(id: "ignored") == ["resume"])

    let opencode = AgentCatalog.shared.descriptor(for: "opencode")
    #expect(opencode.canResume)
    #expect(opencode.resume?.argv(id: "ignored") == ["--continue"])

    // None of these binaries was installed on the verification machine, so
    // every verdict below rests on the linked first-party documentation. Keep
    // each id and token explicit: a manifest edit must not quietly turn an
    // ambiguous id-taking flag into a resume button.
    //
    // antigravity: "--continue  Continue the most recent conversation"
    // https://codelabs.developers.google.com/sdd-agy-cli
    // copilot: "run copilot --continue to resume your most recent session"
    // https://docs.github.com/en/copilot/how-tos/copilot-cli/use-copilot-cli/chronicle
    // devin: "--continue | -c | Resume the most recent session in the current directory"
    // https://docs.devin.ai/cli/reference/commands
    // droid: "droid --resume [sessionId] | Resume a session (defaults to last modified)"
    // https://docs.factory.ai/droid-cli/cli-reference
    // grok: "-c, --continue | Continue the most recent session for the current directory"
    // https://docs.x.ai/build/cli/reference
    // hermes: `hermes --continue` is under "Resume the most recent CLI session"
    // https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/sessions.md
    // kilo: "Resume your last conversation from the current workspace using the --continue flag"
    // https://kilo.ai/docs/code-with-ai/platforms/cli
    // kimi: "--continue | -c | Continue the most recent session in the current working directory"
    // https://moonshotai.github.io/kimi-code/en/reference/kimi-command.html
    // kiro: `kiro-cli chat --resume` is labelled "Resume the most recent chat session"
    // https://kiro.dev/docs/cli/reference/cli-commands/
    // pi: "pi -c  # Continue most recent session"
    // https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md
    // qoder: "-c | 继续上次会话 | qodercli -c" (continue the last session)
    // https://docs.qoder.com/zh/cli/using-cli
    let documentedLatest = [
        ("antigravity", "--continue"),
        ("copilot", "--continue"),
        ("devin", "--continue"),
        ("droid", "--resume"),
        ("grok", "--continue"),
        ("hermes", "--continue"),
        ("kilo", "--continue"),
        ("kimi", "--continue"),
        ("kiro", "--resume"),
        ("pi", "-c"),
        ("qoder", "-c"),
    ]
    for (id, token) in documentedLatest {
        let descriptor = AgentCatalog.shared.descriptor(for: id)
        #expect(descriptor.canResume, "\(id) lost its verified latest-session resume")
        #expect(descriptor.resume?.style == .latest)
        #expect(descriptor.resume?.argv(id: "ignored") == [token])
    }

    // Amp's SDK says `continue: true` will "Continue most recent thread", but
    // its documented CLI spelling is the two-argument `amp threads continue`.
    // A Resume token is one argv element, and no official source documents an
    // equivalent `amp --continue`, so encoding it here would guess at a flag.
    // https://ampcode.com/manual/sdk/typescript
    // https://ampcode.com/manual
    #expect(!AgentCatalog.shared.descriptor(for: "amp").canResume)
}

@Test func resumeSpecsBuildTheRightArgv() {
    #expect(
        AgentDescriptor.Resume(style: .flag, token: "--resume").argv(id: "x") == ["--resume", "x"])
    #expect(
        AgentDescriptor.Resume(style: .subcommand, token: "resume").argv(id: "x")
            == ["resume", "x"])
    // Copilot only accepts the joined form.
    #expect(
        AgentDescriptor.Resume(style: .flagJoined, token: "--resume").argv(id: "x")
            == ["--resume=x"])
    #expect(AgentDescriptor.Resume(style: .latest, token: "resume").argv(id: "x") == ["resume"])
}
