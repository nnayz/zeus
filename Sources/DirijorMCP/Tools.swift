import DirijorCore
import DirijorProtocol
import Foundation

/// The v1 Dirijor MCP tool surface. Schemas are raw JSON Schema (draft 2020-12
/// compatible object schemas). The daemon wiring lives in the CLI.
public enum DirijorMCPTools {
    /// Spawnable agent names, from the manifest catalog rather than a literal
    /// list — a new agent manifest becomes MCP-spawnable with no code change.
    /// Matches what `MCPBridge.spawnAgent` resolves.
    static var spawnableKinds: [String] {
        AgentCatalog.shared.launchable.map(\.shortLabel) + [BuiltinAgentID.shell]
    }

    /// The catalog's display names, for the tool description prose.
    static var spawnableDescription: String {
        let names = AgentCatalog.shared.launchable.map(\.displayName)
        guard let last = names.last, names.count > 1 else { return names.first ?? "an agent" }
        return names.dropLast().joined(separator: ", ") + ", \(last), or a shell"
    }

    public static var all: [McpToolDefinition] {
        let kindEnum = spawnableKinds
            .map { "\"\($0)\"" }
            .joined(separator: ", ")
        let tools = [
            tool(
                "spawn_agent",
                "Open a NEW session (tab) in Dirijor running \(spawnableDescription) — locally or on a configured remote host. USE THIS whenever the user asks to open/start/spawn/launch another agent, session, or terminal. Optionally create a fresh git worktree for it and give it an initial prompt.",
                """
                {
                  "type": "object",
                  "properties": {
                    "kind": { "type": "string", "enum": [\(kindEnum)], "description": "Which agent to run." },
                    "cwd": { "type": "string", "description": "Working directory (a repo path when worktree is true; a REMOTE path on the host when host is set, e.g. ~/code/repo)." },
                    "host": { "type": "string", "description": "Host id from hosts.json (e.g. \\"forge\\") to spawn on that remote host over ssh+tmux. Omit for local. Incompatible with worktree." },
                    "worktree": { "type": "boolean", "description": "Create a fresh git worktree off cwd and run there. Local only." },
                    "prompt": { "type": "string", "description": "Initial prompt to send once the agent is ready." },
                    "name": { "type": "string", "description": "Session title." }
                  },
                  "required": ["kind", "cwd"]
                }
                """
            ),
            tool(
                "list_agents",
                "List all active agent sessions with id, kind, title, status, parent, and cwd.",
                """
                { "type": "object", "properties": {} }
                """
            ),
            tool(
                "get_status",
                "Current status of one session: working / idle / needsInput (with detail) / exited, plus title and cwd.",
                """
                {
                  "type": "object",
                  "properties": {
                    "session_id": { "type": "string", "description": "The session id." }
                  },
                  "required": ["session_id"]
                }
                """
            ),
            tool(
                "send_prompt",
                "Type into another session's terminal and press Enter — send a prompt to an agent, answer its question, or run a shell command there. Set submit:false to type without submitting (e.g. answering a numbered picker).",
                """
                {
                  "type": "object",
                  "properties": {
                    "session_id": { "type": "string" },
                    "text": { "type": "string" },
                    "submit": { "type": "boolean", "description": "Press Enter after typing (default true)." }
                  },
                  "required": ["session_id", "text"]
                }
                """
            ),
            tool(
                "wait_for_agent",
                "Block until an agent session reaches a target condition or the timeout elapses.",
                """
                {
                  "type": "object",
                  "properties": {
                    "session_id": { "type": "string" },
                    "until": { "type": "string", "enum": ["done", "needs_me", "idle", "exited"], "description": "Condition to wait for." },
                    "timeout_s": { "type": "number", "default": 600, "description": "Timeout in seconds." }
                  },
                  "required": ["session_id"]
                }
                """
            ),
            tool(
                "read_output",
                "Read an agent session's output — the current screen or a tail of recent lines.",
                """
                {
                  "type": "object",
                  "properties": {
                    "session_id": { "type": "string" },
                    "mode": { "type": "string", "enum": ["screen", "tail"], "description": "screen = current VT snapshot; tail = recent lines." },
                    "lines": { "type": "number", "default": 50 }
                  },
                  "required": ["session_id"]
                }
                """
            ),
            tool(
                "create_worktree",
                "Create a new git worktree in a repository. A friendly branch name is generated when none is given.",
                """
                {
                  "type": "object",
                  "properties": {
                    "repo": { "type": "string", "description": "Path to the git repository." },
                    "branch": { "type": "string", "description": "Branch name for the worktree." }
                  },
                  "required": ["repo"]
                }
                """
            ),
            tool(
                "list_worktrees",
                "List the git worktrees of a repository.",
                """
                {
                  "type": "object",
                  "properties": {
                    "repo": { "type": "string" }
                  },
                  "required": ["repo"]
                }
                """
            ),
            tool(
                "remove_worktree",
                "Remove a git worktree from a repository.",
                """
                {
                  "type": "object",
                  "properties": {
                    "repo": { "type": "string" },
                    "path": { "type": "string", "description": "The worktree path to remove." },
                    "force": { "type": "boolean" }
                  },
                  "required": ["repo", "path"]
                }
                """
            ),
            tool(
                "get_artifacts",
                "PR links, Linear issues, preview URLs and ports captured from the session's output. "
                    + "PR artifacts carry live GitHub stats (state, review decision, mergeability, "
                    + "CI checks, comment counts, +/- lines) under a 'pr' key.",
                """
                {
                  "type": "object",
                  "properties": {
                    "session_id": { "type": "string", "description": "The session id." }
                  },
                  "required": ["session_id"]
                }
                """
            ),
            tool(
                "release_agent",
                "Terminate an agent session.",
                """
                {
                  "type": "object",
                  "properties": {
                    "session_id": { "type": "string" }
                  },
                  "required": ["session_id"]
                }
                """
            ),
            tool(
                "test_run",
                "Run a web feature test flow across real browser engines (chromium, webkit "
                    + "= Safari's engine, firefox) via Dirijor's shared browser pool — no per-call "
                    + "browser spawn. The whole flow runs in ONE call across all engines in parallel; "
                    + "returns per-engine pass/fail with a compact accessibility snapshot, console "
                    + "errors, and a screenshot FILE PATH only on failure (never inline image bytes). "
                    + "Point it at a preview URL from get_artifacts (e.g. http://localhost:3000). "
                    + "Each step is a one-key object: {click,dblclick,fill,type,press,hover,select,"
                    + "check,uncheck,drag,scroll,waitFor,assert}. Examples: {\\\"click\\\":\\\"#login\\\"}, "
                    + "{\\\"fill\\\":[\\\"#user\\\",\\\"me\\\"]}, {\\\"drag\\\":[\\\"#a\\\",\\\"#b\\\"]}, "
                    + "{\\\"assert\\\":{\\\"selector\\\":\\\"#out\\\",\\\"text\\\":\\\"OK\\\"}}. "
                    + "For AUTHED flows, pass auth to seed a session token/cookie into every "
                    + "engine before the first navigation (obtain it yourself first, e.g. via "
                    + "the app's dev login endpoint); add profile to persist it across runs.",
                """
                {
                  "type": "object",
                  "properties": {
                    "url": { "type": "string", "description": "The page to test, e.g. http://localhost:3000" },
                    "engines": {
                      "type": "array",
                      "items": { "type": "string", "enum": ["chromium", "webkit", "firefox"] },
                      "description": "Engines to run across. Default: chromium only."
                    },
                    "steps": {
                      "type": "array",
                      "items": { "type": "object" },
                      "description": "Ordered one-key step objects (click/type/drag/select/assert/…)."
                    },
                    "observe": {
                      "type": "string",
                      "enum": ["a11y", "screenshot"],
                      "description": "a11y (default, token-cheap) or screenshot-every-step."
                    },
                    "profile": {
                      "type": "string",
                      "description": "Named profile to persist cookies + localStorage across runs — log in once, stay logged in. Omit for a clean isolated context each run."
                    },
                    "auth": {
                      "type": "object",
                      "properties": {
                        "cookies": {
                          "type": "array",
                          "items": { "type": "object" },
                          "description": "Playwright cookie objects ({name, value, url? or domain+path?}); url defaults to the test URL."
                        },
                        "localStorage": {
                          "type": "object",
                          "description": "{key: value} seeded into localStorage at the test URL's origin before any page script runs (or {origin: {key: value}} for other origins)."
                        }
                      },
                      "description": "Auth hand-off for authed flows: state seeded into EVERY engine's context (incl. webkit/firefox nobody logged into) before the first navigation. Obtain the token/cookie yourself first — e.g. curl the app's dev login endpoint — then pass it here. Combine with profile to persist it."
                    }
                  },
                  "required": ["url", "steps"]
                }
                """
            ),
            tool(
                "browser",
                "Drive a real browser isolated to THIS session — cookies and logins are yours alone. "
                    + "Core loop: 'open' a URL, read the returned snapshot for element refs like @e1, act by "
                    + "ref ('click' / 'fill' / 'type' / 'select' / 'check'), and read the fresh snapshot that "
                    + "every action returns. Refs go stale when the page navigates or re-renders — take a new "
                    + "snapshot rather than reusing an old ref. Prefer refs over CSS selectors: a ref is a "
                    + "handle the page just handed you, a selector is a guess about markup you cannot see. "
                    + "'screenshot' saves a file and returns its path (annotate:true labels each ref in the "
                    + "image); 'console' shows errors when a page misbehaves. This is for exploring and "
                    + "operating a live app — use test_run instead to assert a known flow across engines.",
                """
                {
                  "type": "object",
                  "properties": {
                    "action": {
                      "type": "string",
                      "enum": ["open", "snapshot", "click", "fill", "type", "press", "hover", "select", "check", "scroll", "get", "wait", "screenshot", "console", "back", "close", "list"],
                      "description": "open = navigate (launches the browser on first use) and snapshot; snapshot = re-read the page; click/fill/type/press/hover/select/check/scroll = act, each returning a fresh snapshot; get = read url|title|text|html|value|count; wait = for a selector, load state, or fixed ms; screenshot = save an image, return its path; console = recent console + page errors; back = history back; close = end this session's browser; list = open browsers."
                    },
                    "url": { "type": "string", "description": "For open." },
                    "ref": { "type": "string", "description": "Element handle from the last snapshot, e.g. \\"e3\\" or \\"@e3\\". Preferred over selector." },
                    "selector": { "type": "string", "description": "CSS fallback for what a ref can't express (e.g. counting matches)." },
                    "text": { "type": "string", "description": "For fill (replaces the value) and type (real keystrokes, appends)." },
                    "key": { "type": "string", "description": "For press, e.g. Enter, Tab, Escape, Control+a." },
                    "value": { "type": "string", "description": "For select (option value); for check pass \\"false\\" to uncheck." },
                    "what": { "type": "string", "enum": ["url", "title", "text", "html", "value", "count"], "description": "For get. text/html default to the whole body unless given a ref or selector." },
                    "ms": { "type": "number", "description": "For wait: fixed delay in milliseconds." },
                    "state": { "type": "string", "description": "For wait: load | domcontentloaded | networkidle, or an element state (visible, attached, hidden) when used with selector." },
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"], "description": "For scroll without a ref/selector." },
                    "amount": { "type": "number", "description": "Scroll distance in pixels (default 600), or console lines to return (default 50)." },
                    "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "For click." },
                    "double": { "type": "boolean", "description": "Double-click." },
                    "full": { "type": "boolean", "description": "snapshot: include headings and landmarks, not just interactive elements. screenshot: capture the full scrollable page." },
                    "annotate": { "type": "boolean", "description": "screenshot: draw a labelled box over every ref so the image matches the snapshot." },
                    "engine": { "type": "string", "enum": ["chromium", "webkit", "firefox"], "description": "For open. Default chromium; switching engines restarts this session's browser." },
                    "profile": { "type": "string", "description": "For open: named profile persisting cookies and logins across sessions and restarts. Omit for a clean context." }
                  },
                  "required": ["action"]
                }
                """
            ),
            tool(
                "whoami",
                "Identity and lineage of THIS session: its id, title, cwd, branch and worktree, "
                    + "the session that spawned it (if it was delegated), the full ancestor chain, "
                    + "its own children, and the rules governing writes to other sessions. "
                    + "Use this to answer \"who am I\", \"who is my parent\", or \"what did I spawn\" "
                    + "without grepping the environment or scanning list_agents.",
                """
                { "type": "object", "properties": {} }
                """
            ),
            tool(
                "list_children",
                "List the sessions YOU spawned — id, kind, title, status, branch, worktree. "
                    + "Use this to see the delegates you can drive, before waiting on or summarizing them.",
                """
                {
                  "type": "object",
                  "properties": {
                    "recursive": { "type": "boolean", "description": "Include the whole subtree, not just direct children (default false)." },
                    "include_exited": { "type": "boolean", "description": "Include children that have exited (default true)." }
                  }
                }
                """
            ),
            tool(
                "wait_for_children",
                "Block until the sessions you spawned settle, then return each one's final status. "
                    + "Use this after handing work to several delegates instead of polling them one at a time. "
                    + "'settled' (the default) counts needsInput as finished — a delegate stuck on a permission "
                    + "prompt needs you now, and waiting out the full timeout to discover that burns the budget.",
                """
                {
                  "type": "object",
                  "properties": {
                    "session_ids": { "type": "array", "items": { "type": "string" }, "description": "Subset of your children to wait for. Default: all of them." },
                    "until": { "type": "string", "enum": ["settled", "done", "exited"], "description": "settled = idle | needsInput | exited (default); done = idle | exited; exited = process gone." },
                    "timeout_s": { "type": "number", "default": 600, "description": "Give up after this many seconds and report who is still running." }
                  }
                }
                """
            ),
            tool(
                "summarize_children",
                "Compact screen tails plus status and artifacts for the sessions you spawned, so you can "
                    + "synthesize their results in one call instead of reading each one's full output. "
                    + "Returns what each delegate actually printed — it does not interpret or conclude for you.",
                """
                {
                  "type": "object",
                  "properties": {
                    "session_ids": { "type": "array", "items": { "type": "string" }, "description": "Subset of your children. Default: all of them." },
                    "rows": { "type": "number", "default": 14, "description": "Non-blank screen lines per child (max 60)." }
                  }
                }
                """
            ),
            tool(
                "report_to_parent",
                "Hand a STRUCTURED result back to the session that spawned you. Use this instead of "
                    + "send_prompt when you finish, stall, or need a decision from your delegator — the fields "
                    + "arrive labelled, so the parent doesn't have to re-derive structure you already know. "
                    + "Fails if this session has no parent (it was started by the user, not delegated).",
                """
                {
                  "type": "object",
                  "properties": {
                    "summary": { "type": "string", "description": "One-line result or status. Required." },
                    "status": { "type": "string", "enum": ["update", "done", "blocked", "failed"], "description": "Default: update." },
                    "details": { "type": "string", "description": "Anything the parent needs beyond the summary." },
                    "blockers": { "type": "array", "items": { "type": "string" }, "description": "What is stopping you: missing context, approvals, broken deps." },
                    "questions": { "type": "array", "items": { "type": "string" }, "description": "Decisions you need from the parent or user." },
                    "next_steps": { "type": "array", "items": { "type": "string" }, "description": "Suggested follow-up work." },
                    "changed_paths": { "type": "array", "items": { "type": "string" }, "description": "Files you created or modified." },
                    "artifacts": { "type": "array", "items": { "type": "string" }, "description": "PR links, preview URLs, generated file paths." },
                    "proof": { "type": "array", "items": { "type": "string" }, "description": "Commands run, tests passed, evidence the work is real." },
                    "submit": { "type": "boolean", "description": "Press Enter after delivering (default true)." }
                  },
                  "required": ["summary"]
                }
                """
            ),
        ]
        guard ProcessInfo.processInfo.environment["DIRIJOR_TEST_RUN_AVAILABLE"] == "1" else {
            return tools.filter { $0.name != "test_run" }
        }
        return tools
    }

    /// Wire-ready `tools/list` payload shared by the long-lived Swift server
    /// and the lightweight standalone stdio proxy.
    public static var listResult: JSONValue {
        .object([
            "tools": .array(all.map { tool in
                .object([
                    "name": .string(tool.name),
                    "description": .string(tool.description),
                    "inputSchema": tool.inputSchema,
                ])
            })
        ])
    }

    private static func tool(_ name: String, _ description: String, _ schemaJSON: String) -> McpToolDefinition {
        let schema: JSONValue
        if let data = schemaJSON.data(using: .utf8),
            let decoded = try? JSONDecoder.dirijor.decode(JSONValue.self, from: data)
        {
            schema = decoded
        } else {
            schema = .object(["type": .string("object")])
        }
        return McpToolDefinition(name: name, description: description, inputSchema: schema)
    }
}
