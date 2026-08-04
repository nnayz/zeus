import DirijorCore
import DirijorDetection
import DirijorProtocol
import Foundation

/// Side-metadata a hook/notify payload carries beyond the status signal:
/// identity binding, titles, needs-me detail.
struct HookMetadata {
    var agentSessionID: String?
    var transcriptPath: String?
    var firstPromptTitle: String?
    var needsInput: NeedsInputDetail?
}

enum HookParsing {
    /// Parses a Claude hook payload (raw stdin JSON forwarded by `dirijor hook`).
    static func parseClaudeHook(event: String, payload: JSONValue) -> (StatusSignal, HookMetadata)? {
        var meta = HookMetadata()
        let isSubagent = payload["agent_id"]?.stringValue != nil
        // Every Claude hook payload carries the session identity — and the
        // transcript MOVES to another project dir when the agent enters a
        // worktree mid-session, so capture it on every event (SessionStart
        // alone leaves the record pointing at the pre-worktree path forever).
        meta.agentSessionID = payload["session_id"]?.stringValue
        meta.transcriptPath = payload["transcript_path"]?.stringValue

        let signal: ClaudeHookSignal
        switch event {
        case "SessionStart":
            signal = .sessionStart(
                source: payload["source"]?.stringValue,
                agentSessionID: meta.agentSessionID,
                transcriptPath: meta.transcriptPath
            )
        case "UserPromptSubmit":
            let prompt = payload["prompt_text"]?.stringValue ?? payload["prompt"]?.stringValue
            if let prompt, !isSubagent {
                meta.firstPromptTitle = TitleMaker.fromFirstPrompt(prompt)
            }
            signal = .userPromptSubmit(promptText: prompt)
        case "PreToolUse":
            let (tool, summary) = toolSummary(payload)
            signal = .preToolUse(toolName: tool, toolInputSummary: summary)
        case "PermissionRequest":
            let (tool, summary) = toolSummary(payload)
            if !isSubagent {
                meta.needsInput = NeedsInputDetail(
                    kind: .permission,
                    source: .claudePermissionHook,
                    toolName: tool,
                    summary: permissionSummary(tool: tool, inputSummary: summary),
                    riskHint: RiskHint.classify(summary ?? "")
                )
            }
            signal = .permissionRequest(toolName: tool, toolInputSummary: summary)
        case "Notification":
            let type = payload["notification_type"]?.stringValue
            let message = payload["message"]?.stringValue
            if !isSubagent, let kind = needsInputKind(forNotificationType: type) {
                meta.needsInput = NeedsInputDetail(
                    kind: kind,
                    source: .claudeNotificationHook,
                    summary: Redaction.redact(message ?? "Claude needs your input")
                )
            }
            signal = .notification(notificationType: type, message: message)
        case "Stop":
            signal = .stop
        case "SubagentStart":
            signal = .subagentStart(agentID: payload["agent_id"]?.stringValue ?? "?")
        case "SubagentStop":
            signal = .subagentStop(agentID: payload["agent_id"]?.stringValue ?? "?")
        case "SessionEnd":
            signal = .sessionEnd(reason: payload["reason"]?.stringValue)
        default:
            return nil
        }
        return (.claudeHook(signal, isSubagent: isSubagent), meta)
    }

    /// Parses a Codex notify payload (single JSON argv forwarded by `dirijor notify`).
    static func parseCodexNotify(payload: JSONValue) -> (StatusSignal, HookMetadata)? {
        guard payload["type"]?.stringValue == "agent-turn-complete" else { return nil }
        var meta = HookMetadata()
        meta.agentSessionID = payload["thread-id"]?.stringValue
        if case .array(let inputs)? = payload["input-messages"],
            let first = inputs.first?.stringValue
        {
            meta.firstPromptTitle = TitleMaker.fromFirstPrompt(first)
        }
        let last = payload["last-assistant-message"]?.stringValue
        return (.codexTurnComplete(lastAssistantMessage: last), meta)
    }

    // MARK: Helpers

    private static func needsInputKind(forNotificationType type: String?) -> NeedsInputKind? {
        switch type {
        case "permission_prompt": .permission
        case "idle_prompt", "agent_needs_input", "elicitation_dialog": .question
        default: nil
        }
    }

    /// (tool_name, human summary of tool_input).
    static func toolSummary(_ payload: JSONValue) -> (String?, String?) {
        let tool = payload["tool_name"]?.stringValue
        guard let input = payload["tool_input"]?.objectValue else { return (tool, nil) }
        let summary: String? =
            switch tool {
            case "Bash":
                input["command"]?.stringValue
            case "Edit", "Write", "Read", "NotebookEdit":
                input["file_path"]?.stringValue
            case "WebFetch", "WebSearch":
                input["url"]?.stringValue ?? input["query"]?.stringValue
            default:
                input.values.compactMap(\.stringValue).first
            }
        return (tool, summary.map { Redaction.redact(String($0.prefix(120))) })
    }

    static func permissionSummary(tool: String?, inputSummary: String?) -> String {
        switch tool {
        case "Bash":
            "wants to run `\(inputSummary ?? "a command")`"
        case "Edit", "Write":
            "wants to edit \(shortPath(inputSummary))"
        case "WebFetch":
            "wants to fetch \(inputSummary ?? "a URL")"
        case .some(let name) where name.hasPrefix("mcp__"):
            "wants to use \(name.dropFirst(5).replacingOccurrences(of: "__", with: ":"))"
        case .some(let name):
            "wants to use \(name)" + (inputSummary.map { ": \($0)" } ?? "")
        case nil:
            "needs permission" + (inputSummary.map { ": \($0)" } ?? "")
        }
    }

    private static func shortPath(_ path: String?) -> String {
        guard let path else { return "a file" }
        let parts = path.split(separator: "/")
        return parts.suffix(2).joined(separator: "/")
    }
}
