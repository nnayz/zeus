//! Shared policy and MCP metadata for sessions hosted by Zeus.
//!
//! The Engine, CLI fallback server, standalone MCP proxy, and provider launch
//! shims all consume this module. Keeping the model-facing names and wording
//! here prevents one entry point from drifting back to a provider-native
//! subagent path.

use serde_json::{Value, json};

/// The only advertised tool name for creating a visible child session.
pub const CREATE_ZEUS_SESSION_TOOL: &str = "create_zeus_session";
/// Accepted for integrations that predate [`CREATE_ZEUS_SESSION_TOOL`].
pub const LEGACY_SPAWN_AGENT_TOOL: &str = "spawn_agent";

/// High-priority policy injected through each hosted provider's supported
/// instruction mechanism and repeated in MCP initialization instructions.
pub const HOSTED_SESSION_POLICY: &str = "This process is hosted by Zeus. When the user asks to spawn, delegate, parallelize, or open another agent or session, call the Zeus MCP `create_zeus_session` tool. Do not use provider-native subagents unless the user explicitly requests an internal hidden worker. Create one Zeus child for each explicitly requested parallel task, and preserve the current session as its parent. If the user does not name an agent kind, omit `kind` so Zeus uses the configured default. The old `spawn_agent` name is a compatibility alias; do not select it for new calls.";

#[derive(Clone, Debug)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Maps the unadvertised compatibility alias to the canonical tool name.
pub fn canonical_tool_name(name: &str) -> &str {
    if name == LEGACY_SPAWN_AGENT_TOOL {
        CREATE_ZEUS_SESSION_TOOL
    } else {
        name
    }
}

/// The one shared definition of the visible Zeus session creation tool.
pub fn create_session_tool_definition(kinds: &[String], display_names: &str) -> ToolDefinition {
    let kind_enum: Vec<Value> = kinds.iter().map(|kind| json!(kind)).collect();
    ToolDefinition {
        name: CREATE_ZEUS_SESSION_TOOL.to_owned(),
        description: format!(
            "Open ONE visible Zeus child session nested under this one, running {display_names} locally or on a configured remote host. Use this whenever the user asks to spawn, delegate, parallelize, or open another agent, session, tab, or terminal. Provider-native subagents stay hidden inside this terminal and must only be used when the user explicitly requests an internal hidden worker. Make one call per explicitly requested parallel task. If the user does not name a kind, omit kind so Zeus uses the configured default; never guess, probe, or fan out across agent kinds."
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": kind_enum, "description": "Which agent to run. Omit to use Zeus's configured default agent." },
                "cwd": { "type": "string", "description": "Working directory." },
                "host": { "type": "string", "description": "Configured Zeus host id. Omit for a local child." },
                "worktree": { "type": "boolean", "description": "Create a fresh git worktree off cwd and run there. Local only." },
                "prompt": { "type": "string", "description": "Initial prompt to send once the agent is ready." },
                "name": { "type": "string", "description": "Session title." }
            },
            "required": ["cwd"]
        }),
    }
}

/// Shared MCP initialization guidance. MCP is reinforcement for the injected
/// provider policy, but uses the same canonical names and orchestration flow.
pub fn mcp_instructions(test_run_available: bool) -> String {
    let browser = if test_run_available {
        " To test a web feature, use test_run with a preview URL from get_artifacts."
    } else {
        ""
    };
    format!(
        "{HOSTED_SESSION_POLICY}\n\nThese tools control Zeus. Use them proactively to inspect, coordinate, or close sessions and to parallelize work across git worktrees; no extra confirmation of intent is needed. Typical flow: {CREATE_ZEUS_SESSION_TOOL} (optionally worktree:true and an initial prompt) → wait_for_agent(until:\"done\") → read_output → send_prompt for follow-ups → release_agent when finished. get_artifacts returns PR, issue, preview, and listening-port artifacts.{browser}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_spawn_name_normalizes_without_being_advertised() {
        assert_eq!(
            canonical_tool_name(LEGACY_SPAWN_AGENT_TOOL),
            CREATE_ZEUS_SESSION_TOOL
        );
        assert_eq!(canonical_tool_name("list_agents"), "list_agents");

        let definition = create_session_tool_definition(&["codex".into()], "Codex");
        assert_eq!(definition.name, CREATE_ZEUS_SESSION_TOOL);
        assert_ne!(definition.name, LEGACY_SPAWN_AGENT_TOOL);
    }

    #[test]
    fn policy_and_mcp_guidance_use_only_the_canonical_name_for_new_calls() {
        let instructions = mcp_instructions(false);
        assert!(instructions.contains("create_zeus_session"));
        assert!(instructions.contains("compatibility alias"));
        assert!(!instructions.contains("Typical flow: spawn_agent"));
    }
}
