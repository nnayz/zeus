import DirijorProtocol
import Foundation

/// A single MCP tool advertised via `tools/list`.
public struct McpToolDefinition: Sendable {
    public var name: String
    public var description: String
    /// The JSON Schema for the tool's arguments, as a raw JSON value.
    public var inputSchema: JSONValue

    public init(name: String, description: String, inputSchema: JSONValue) {
        self.name = name
        self.description = description
        self.inputSchema = inputSchema
    }
}

/// Dispatches a `tools/call`. `toolName` is the requested tool, `arguments` the
/// raw `arguments` object. The returned JSONValue is serialized into MCP text
/// content; a thrown error becomes an `isError: true` result.
public typealias McpToolHandler = @Sendable (_ toolName: String, _ arguments: JSONValue) async throws -> JSONValue

/// A minimal MCP (Model Context Protocol) server over the 2025-03-26 stdio
/// transport: newline-delimited JSON-RPC 2.0 on stdin/stdout (no Content-Length
/// framing). Message handling is pure and testable via `handle(_:)`; `run()`
/// wires it to the process's stdin/stdout.
public final class McpServer: @unchecked Sendable {
    public static let serverName = "dirijor"
    public static let serverVersion = "0.1.0"
    public static let preferredProtocolVersion = "2025-06-18"

    private let tools: [McpToolDefinition]
    private let handler: McpToolHandler

    public init(tools: [McpToolDefinition], handler: @escaping McpToolHandler) {
        self.tools = tools
        self.handler = handler
    }

    // MARK: - Run loop

    /// Reads newline-delimited JSON-RPC from stdin and writes responses to
    /// stdout until EOF.
    public func run() async {
        let stdin = FileHandle.standardInput
        var buffer = Data()

        while true {
            let chunk = stdin.availableData
            if chunk.isEmpty { break }  // EOF
            buffer.append(chunk)
            while let newline = buffer.firstIndex(of: 0x0A) {
                let line = buffer.subdata(in: buffer.startIndex..<newline)
                buffer.removeSubrange(buffer.startIndex...newline)
                await processLine(line)
            }
        }
        // Trailing line without a terminating newline.
        if !buffer.isEmpty { await processLine(buffer) }
    }

    private func processLine(_ line: Data) async {
        let trimmed = line.drop { $0 == 0x20 || $0 == 0x09 || $0 == 0x0D }
        guard !trimmed.isEmpty else { return }

        let message: JSONValue
        do {
            message = try JSONDecoder.dirijor.decode(JSONValue.self, from: Data(trimmed))
        } catch {
            writeMessage(Self.errorResponse(id: .null, code: -32700, message: "Parse error"))
            return
        }

        if let response = await handle(message) {
            writeMessage(response)
        }
    }

    private func writeMessage(_ value: JSONValue) {
        guard var data = try? JSONEncoder.dirijor.encode(value) else { return }
        data.append(0x0A)
        FileHandle.standardOutput.write(data)
    }

    // MARK: - Pure message handling

    /// Handles one decoded JSON-RPC message. Returns the response to write, or
    /// nil for notifications (and messages that warrant no reply).
    public func handle(_ message: JSONValue) async -> JSONValue? {
        guard case .object(let obj) = message else {
            return Self.errorResponse(id: .null, code: -32600, message: "Invalid Request")
        }
        guard let method = obj["method"]?.stringValue else {
            // Not a request/notification we recognize (could be a response).
            return nil
        }
        let id = obj["id"]  // nil => notification
        let params = obj["params"] ?? .null

        switch method {
        case "initialize":
            return responseFor(id: id, result: initializeResult(params: params))

        case "notifications/initialized", "initialized":
            return nil  // notification, ignore

        case "ping":
            return responseFor(id: id, result: .object([:]))

        case "tools/list":
            return responseFor(id: id, result: toolsListResult())

        case "tools/call":
            let result = await toolsCallResult(params: params)
            return responseFor(id: id, result: result)

        default:
            // Unknown notification: ignore. Unknown request: method-not-found.
            guard let id else { return nil }
            return Self.errorResponse(id: id, code: -32601, message: "Method not found: \(method)")
        }
    }

    /// Wraps a result, but only if this was a request (had an id). Notifications
    /// get no reply.
    private func responseFor(id: JSONValue?, result: JSONValue) -> JSONValue? {
        guard let id else { return nil }
        return Self.successResponse(id: id, result: result)
    }

    // MARK: - Method implementations

    private func initializeResult(params: JSONValue) -> JSONValue {
        let requested = params["protocolVersion"]?.stringValue
        // Respond with our preferred version; if the client asked for a different
        // one, echo theirs back for maximum interoperability.
        let version: String
        if let requested, requested != Self.preferredProtocolVersion {
            version = requested
        } else {
            version = Self.preferredProtocolVersion
        }
        let browserInstructions =
            ProcessInfo.processInfo.environment["DIRIJOR_TEST_RUN_AVAILABLE"] == "1"
            ? """

                To TEST a web feature, use test_run — it drives real browser
                engines through Dirijor's shared Playwright pool. Feed it a preview
                URL from get_artifacts and an ordered list of steps.
                """
            : ""
        return .object([
            "protocolVersion": .string(version),
            "capabilities": .object([
                "tools": .object([:]),
            ]),
            "serverInfo": .object([
                "name": .string(Self.serverName),
                "version": .string(Self.serverVersion),
            ]),
            // Injected into the agent's context by MCP clients — this is what
            // makes an agent reach for these tools unprompted when the user
            // says "open a codex session" instead of asking what that means.
            "instructions": .string(
                """
                This session is running INSIDE Dirijor, a macOS orchestrator for \
                coding agents. These tools control it. Use them proactively whenever \
                the user asks to open/start/spawn/close another agent, session, tab, \
                or terminal (Claude Code, Codex, Cursor, Gemini, or a shell), to check what other \
                sessions are doing, to talk to another session, or to parallelize \
                work across git worktrees — no extra confirmation of intent needed.

                Typical orchestration flow: spawn_agent (optionally worktree:true and \
                an initial prompt) → wait_for_agent(until:"done") → read_output → \
                send_prompt for follow-ups → release_agent when finished. \
                get_artifacts returns PR/Linear/preview URLs and listening ports a \
                session has produced; PR entries include live GitHub status \
                (state, review decision, checks, comment counts, +/- lines).\(browserInstructions)
                """),
        ])
    }

    private func toolsListResult() -> JSONValue {
        let defs = tools.map { tool in
            JSONValue.object([
                "name": .string(tool.name),
                "description": .string(tool.description),
                "inputSchema": tool.inputSchema,
            ])
        }
        return .object(["tools": .array(defs)])
    }

    private func toolsCallResult(params: JSONValue) async -> JSONValue {
        guard let name = params["name"]?.stringValue else {
            return Self.toolErrorContent("tools/call missing 'name'")
        }
        let arguments = params["arguments"] ?? .object([:])
        do {
            let output = try await handler(name, arguments)
            return Self.toolTextContent(output, isError: false)
        } catch {
            let message: String
            if let controlError = error as? ControlError {
                message = "\(controlError.code): \(controlError.message)"
            } else {
                message = String(describing: error)
            }
            return Self.toolErrorContent(message)
        }
    }

    // MARK: - MCP content helpers

    /// Serializes a JSONValue result into MCP text content.
    static func toolTextContent(_ value: JSONValue, isError: Bool) -> JSONValue {
        let text: String
        if case .string(let s) = value {
            text = s
        } else if let data = try? JSONEncoder.dirijor.encode(value),
            let s = String(data: data, encoding: .utf8)
        {
            text = s
        } else {
            text = "null"
        }
        return .object([
            "content": .array([
                .object(["type": .string("text"), "text": .string(text)]),
            ]),
            "isError": .bool(isError),
        ])
    }

    static func toolErrorContent(_ message: String) -> JSONValue {
        .object([
            "content": .array([
                .object(["type": .string("text"), "text": .string(message)]),
            ]),
            "isError": .bool(true),
        ])
    }

    // MARK: - JSON-RPC envelope helpers

    static func successResponse(id: JSONValue, result: JSONValue) -> JSONValue {
        .object([
            "jsonrpc": .string("2.0"),
            "id": id,
            "result": result,
        ])
    }

    static func errorResponse(id: JSONValue, code: Int, message: String) -> JSONValue {
        .object([
            "jsonrpc": .string("2.0"),
            "id": id,
            "error": .object([
                "code": .number(Double(code)),
                "message": .string(message),
            ]),
        ])
    }
}
