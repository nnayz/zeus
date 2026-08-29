//! Spawn-time config injection: what turns a bare agent CLI into a
//! Zeus-connected session.
//!
//! Ported from the local half of `InjectionBuilder`. The daemon writes two
//! files at startup — a Claude hooks settings file and a Claude MCP config —
//! whose contents reference `$ZEUS_CLI` / the CLI's sibling `zeus-mcp`,
//! then appends per-launch flags (`--settings`, `--mcp-config`, Codex `-c`
//! overrides) for whichever mechanisms the agent's manifest opted into. This
//! is what makes a Claude session hook-driven rather than screen-detected,
//! and what gives every agent the `zeus` MCP tools.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::json;
use zeus_proto::orchestration::HOSTED_SESSION_POLICY;

use crate::agent::InjectionSpec;

/// Environment variable names shared with every hook and MCP shim.
pub const SESSION_ID_ENV: &str = "ZEUS_SESSION_ID";
pub const SOCKET_ENV: &str = "ZEUS_SOCKET";
pub const CLI_ENV: &str = "ZEUS_CLI";

/// A random v4 UUID in the lowercase-hex form Claude accepts as
/// `--session-id`. Minting it ourselves is what makes resume possible later
/// without the agent ever reporting an id.
pub fn uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("random");
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
    let h = |range: std::ops::Range<usize>| {
        bytes[range]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    format!(
        "{}-{}-{}-{}-{}",
        h(0..4),
        h(4..6),
        h(6..8),
        h(8..10),
        h(10..16)
    )
}

/// The static hooks file injected into every Claude session via `--settings`.
/// Commands read `$ZEUS_CLI` / `$ZEUS_SOCKET` from the PTY env, so the
/// file content is identical for all sessions and safe to write once.
pub fn write_claude_hooks_file(inject_dir: &Path) -> io::Result<()> {
    const EVENTS: [&str; 9] = [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PermissionRequest",
        "Notification",
        "Stop",
        "SubagentStart",
        "SubagentStop",
        "SessionEnd",
    ];
    let mut hooks = serde_json::Map::new();
    for event in EVENTS {
        hooks.insert(
            event.to_string(),
            json!([{
                "hooks": [{
                    "type": "command",
                    "command": format!("\"${CLI_ENV}\" hook {event}"),
                    "timeout": 10,
                }]
            }]),
        );
    }
    write_atomic(
        &inject_dir.join("claude-hooks.json"),
        &serde_json::to_vec(&json!({ "hooks": hooks }))?,
    )
}

/// The Claude `--mcp-config` file: the `zeus` stdio server backed by the
/// CLI's sibling `zeus-mcp` proxy (or the CLI itself as a fallback).
pub fn write_claude_mcp_file(inject_dir: &Path, cli_path: &Path) -> io::Result<()> {
    let (command, args) = mcp_launch(cli_path);
    write_atomic(
        &inject_dir.join("claude-mcp.json"),
        &serde_json::to_vec_pretty(&json!({
            "mcpServers": {
                "zeus": { "type": "stdio", "command": command, "args": args }
            }
        }))?,
    )
}

fn mcp_launch(cli_path: &Path) -> (String, Vec<String>) {
    let proxy = cli_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("zeus-mcp");
    if is_executable(&proxy) {
        (proxy.to_string_lossy().into_owned(), Vec::new())
    } else {
        (
            cli_path.to_string_lossy().into_owned(),
            vec!["mcp-stdio".into()],
        )
    }
}

/// `/usr/bin/env ZEUS_*=… zeus-mcp` so identity survives agents that scrub
/// `ZEUS_*` from their own process environment.
fn mcp_launch_for_session(cli_path: &Path, identity: &CursorInject<'_>) -> (String, Vec<String>) {
    let (command, args) = mcp_launch(cli_path);
    let mut wrapped = vec![
        format!("ZEUS_SESSION_ID={}", identity.session_id),
        format!("ZEUS_SOCKET={}", identity.socket_path.display()),
        format!("ZEUS_CLI={}", cli_path.display()),
        command,
    ];
    wrapped.extend(args);
    ("/usr/bin/env".to_owned(), wrapped)
}

/// Session identity needed to bake Cursor's per-launch plugin (MCP env + hooks).
#[derive(Clone, Copy)]
pub struct CursorInject<'a> {
    pub session_id: &'a str,
    pub socket_path: &'a Path,
}

/// Per-launch injection arguments for the mechanisms a manifest opted into.
pub fn injection_args(
    injection: &InjectionSpec,
    inject_dir: &Path,
    cli_path: &Path,
) -> Vec<String> {
    injection_args_with_cursor(injection, inject_dir, cli_path, None)
}

/// Like [`injection_args`], and when `cursor` is set also writes/appends the
/// session-local Cursor `--plugin-dir` (MCP + stop hook).
pub fn injection_args_with_cursor(
    injection: &InjectionSpec,
    inject_dir: &Path,
    cli_path: &Path,
    cursor: Option<CursorInject<'_>>,
) -> Vec<String> {
    let mut argv = Vec::new();
    if injection.claude_hooks {
        let hooks = inject_dir.join("claude-hooks.json");
        if hooks.exists() {
            argv.push("--settings".into());
            argv.push(hooks.to_string_lossy().into_owned());
        }
    }
    if injection.claude_mcp {
        let mcp = match &cursor {
            Some(identity) => write_session_claude_mcp(inject_dir, cli_path, identity)
                .unwrap_or_else(|_| inject_dir.join("claude-mcp.json")),
            None => inject_dir.join("claude-mcp.json"),
        };
        if mcp.exists() {
            argv.push("--mcp-config".into());
            argv.push(mcp.to_string_lossy().into_owned());
        }
        argv.push("--append-system-prompt".into());
        argv.push(HOSTED_SESSION_POLICY.into());
        // Claude's Agent/Task tools create provider-native workers inside the
        // parent PTY. The injected policy tells the model where to redirect.
        argv.push("--disallowedTools".into());
        argv.push("Agent,Task".into());
    }
    if injection.codex_notify {
        argv.push("-c".into());
        argv.push(format!(
            "notify=[{}, \"notify\"]",
            toml_string(&cli_path.to_string_lossy())
        ));
    }
    if injection.codex_mcp {
        let (command, args) = match &cursor {
            Some(identity) => mcp_launch_for_session(cli_path, identity),
            None => mcp_launch(cli_path),
        };
        let encoded_args = args
            .iter()
            .map(|arg| toml_string(arg))
            .collect::<Vec<_>>()
            .join(",");
        argv.push("-c".into());
        argv.push(format!(
            "mcp_servers.zeus.command={}",
            toml_string(&command)
        ));
        argv.push("-c".into());
        argv.push(format!("mcp_servers.zeus.args=[{encoded_args}]"));
        // Codex's built-in collaboration `spawn_agent` creates `/root/…`
        // workers inside this PTY. Those never become Zeus sessions, so the
        // sidebar stays empty. Disable it and direct delegation to the
        // canonical Zeus MCP session-creation tool.
        argv.push("-c".into());
        argv.push("features.multi_agent=false".into());
        argv.push("-c".into());
        argv.push(format!(
            "developer_instructions={}",
            toml_string(HOSTED_SESSION_POLICY)
        ));
    }
    if injection.grok_mcp {
        // Grok exposes both an append-only rules channel and a hard native
        // subagent gate, so hosted sessions can enforce both halves.
        argv.push("--rules".into());
        argv.push(HOSTED_SESSION_POLICY.into());
        argv.push("--no-subagents".into());
    }
    if (injection.cursor_mcp || injection.cursor_hooks)
        && let Some(cursor) = cursor
        && let Ok(plugin_dir) = write_cursor_plugin(
            inject_dir,
            cli_path,
            cursor.session_id,
            cursor.socket_path,
            injection.cursor_mcp,
            injection.cursor_hooks,
        )
    {
        argv.push("--plugin-dir".into());
        argv.push(plugin_dir.to_string_lossy().into_owned());
        if injection.cursor_mcp {
            argv.push("--approve-mcps".into());
        }
    }
    argv
}

/// Extra process environment the agent needs so it *loads* the Zeus MCP
/// server. These are applied after `spawn_spec` so scrub prefixes cannot
/// drop them. Files are written here too — the env just points at them.
pub fn injection_env(
    injection: &InjectionSpec,
    inject_dir: &Path,
    cli_path: &Path,
    identity: Option<CursorInject<'_>>,
) -> Vec<(String, String)> {
    let Some(identity) = identity else {
        return Vec::new();
    };
    let mut env = Vec::new();
    if injection.grok_mcp
        && let Ok(path) = write_grok_mcp_overlay(inject_dir, cli_path, &identity)
    {
        env.push((
            "GROK_CONFIG_PATH".into(),
            path.to_string_lossy().into_owned(),
        ));
    }
    if injection.opencode_mcp
        && let Ok(path) = write_opencode_mcp_overlay(inject_dir, cli_path, &identity)
    {
        env.push((
            "OPENCODE_CONFIG".into(),
            path.to_string_lossy().into_owned(),
        ));
    }
    if injection.gemini_mcp
        && let Ok(path) = write_gemini_home(inject_dir, cli_path, &identity)
    {
        env.push((
            "GEMINI_CLI_HOME".into(),
            path.to_string_lossy().into_owned(),
        ));
    }
    env
}

fn session_inject_dir(inject_dir: &Path, session_id: &str) -> PathBuf {
    inject_dir.join("sessions").join(session_id)
}

fn write_session_claude_mcp(
    inject_dir: &Path,
    cli_path: &Path,
    identity: &CursorInject<'_>,
) -> io::Result<PathBuf> {
    let (command, args) = mcp_launch_for_session(cli_path, identity);
    let path = session_inject_dir(inject_dir, identity.session_id).join("claude-mcp.json");
    write_atomic(
        &path,
        &serde_json::to_vec_pretty(&json!({
            "mcpServers": {
                "zeus": {
                    "type": "stdio",
                    "command": command,
                    "args": args,
                }
            }
        }))?,
    )?;
    Ok(path)
}

fn write_grok_mcp_overlay(
    inject_dir: &Path,
    cli_path: &Path,
    identity: &CursorInject<'_>,
) -> io::Result<PathBuf> {
    let (command, args) = mcp_launch_for_session(cli_path, identity);
    let path = session_inject_dir(inject_dir, identity.session_id).join("grok-mcp.json");
    write_atomic(
        &path,
        &serde_json::to_vec_pretty(&json!({
            "mcp_servers": {
                "zeus": {
                    "command": command,
                    "args": args,
                }
            }
        }))?,
    )?;
    Ok(path)
}

fn write_opencode_mcp_overlay(
    inject_dir: &Path,
    cli_path: &Path,
    identity: &CursorInject<'_>,
) -> io::Result<PathBuf> {
    let (command, args) = mcp_launch_for_session(cli_path, identity);
    let mut argv = vec![command];
    argv.extend(args);
    let session_dir = session_inject_dir(inject_dir, identity.session_id);
    let policy = session_dir.join("zeus-orchestration.md");
    write_atomic(&policy, HOSTED_SESSION_POLICY.as_bytes())?;
    let path = session_dir.join("opencode.json");
    write_atomic(
        &path,
        &serde_json::to_vec_pretty(&json!({
            "instructions": [policy],
            "permission": {
                "task": "deny"
            },
            "mcp": {
                "zeus": {
                    "type": "local",
                    "command": argv,
                    "enabled": true
                }
            }
        }))?,
    )?;
    Ok(path)
}

fn write_gemini_home(
    inject_dir: &Path,
    cli_path: &Path,
    identity: &CursorInject<'_>,
) -> io::Result<PathBuf> {
    let dest = inject_dir.join("gemini").join(identity.session_id);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&dest)?;
    let user = std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".gemini"));
    if let Some(user) = user.as_ref().filter(|path| path.is_dir()) {
        for entry in std::fs::read_dir(user)? {
            let entry = entry?;
            if entry.file_name() == "settings.json" || entry.file_name() == "GEMINI.md" {
                continue;
            }
            let _ = std::os::unix::fs::symlink(entry.path(), dest.join(entry.file_name()));
        }
    }
    let mut settings = user
        .as_ref()
        .map(|dir| dir.join("settings.json"))
        .filter(|path| path.is_file())
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .filter(|value| value.is_object())
        .unwrap_or_else(|| json!({}));
    let (command, args) = mcp_launch_for_session(cli_path, identity);
    settings["mcpServers"]["zeus"] = json!({
        "command": command,
        "args": args,
    });
    let user_context = user
        .as_ref()
        .map(|dir| dir.join("GEMINI.md"))
        .filter(|path| path.is_file())
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let context = if user_context.trim().is_empty() {
        format!("# Zeus hosted-session policy\n\n{HOSTED_SESSION_POLICY}\n")
    } else {
        format!(
            "# Zeus hosted-session policy\n\n{HOSTED_SESSION_POLICY}\n\n# User context\n\n{user_context}"
        )
    };
    write_atomic(&dest.join("GEMINI.md"), context.as_bytes())?;
    write_atomic(
        &dest.join("settings.json"),
        &serde_json::to_vec_pretty(&settings)?,
    )?;
    Ok(dest)
}

/// Writes `<inject>/cursor-plugin/<session>/` with plugin manifest, optional
/// `mcp.json`, and optional `hooks/hooks.json`. Returns the plugin directory.
///
/// Always stages into a fresh temp dir and replaces the live plugin tree so a
/// resume that disables MCP or hooks cannot leave the previous files active.
pub fn write_cursor_plugin(
    inject_dir: &Path,
    cli_path: &Path,
    session_id: &str,
    socket_path: &Path,
    mcp: bool,
    hooks: bool,
) -> io::Result<PathBuf> {
    let plugin_root = inject_dir.join("cursor-plugin");
    std::fs::create_dir_all(&plugin_root)?;
    let plugin_dir = plugin_root.join(session_id);
    let staging = plugin_root.join(format!(".{session_id}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(staging.join(".cursor-plugin"))?;
    write_atomic(
        &staging.join(".cursor-plugin/plugin.json"),
        &serde_json::to_vec_pretty(&json!({
            "name": "zeus",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Zeus agent orchestration for Cursor sessions",
        }))?,
    )?;

    if mcp {
        let identity = CursorInject {
            session_id,
            socket_path,
        };
        let (command, args) = mcp_launch_for_session(cli_path, &identity);
        // Cursor splits `command` on spaces before spawning it. Keep the
        // App Support path in argv, where JSON preserves it as one argument.
        write_atomic(
            &staging.join("mcp.json"),
            &serde_json::to_vec_pretty(&json!({
                "mcpServers": {
                    "zeus": {
                        "type": "stdio",
                        "command": command,
                        "args": args,
                        "env": {
                            "ZEUS_SESSION_ID": session_id,
                            "ZEUS_SOCKET": socket_path.to_string_lossy(),
                            "ZEUS_CLI": cli_path.to_string_lossy(),
                        }
                    }
                }
            }))?,
        )?;

        std::fs::create_dir_all(staging.join("rules"))?;
        write_atomic(
            &staging.join("rules/zeus-orchestration.mdc"),
            format!(
                "---\ndescription: Route delegated work to visible Zeus sessions\nalwaysApply: true\n---\n\n{HOSTED_SESSION_POLICY}\n"
            )
            .as_bytes(),
        )?;
    }

    if hooks {
        std::fs::create_dir_all(staging.join("hooks"))?;
        // Absolute CLI path: Cursor does not expand $ZEUS_CLI in hook cmds.
        let quoted = shell_single_quote(&cli_path.to_string_lossy());
        write_atomic(
            &staging.join("hooks/hooks.json"),
            &serde_json::to_vec_pretty(&json!({
                "version": 1,
                "hooks": {
                    "stop": [{ "command": format!("{quoted} hook Stop") }],
                    "beforeSubmitPrompt": [{
                        "command": format!("{quoted} hook UserPromptSubmit")
                    }],
                }
            }))?,
        )?;
    }

    let backup = plugin_root.join(format!(".{session_id}.{}.old", std::process::id()));
    let _ = std::fs::remove_dir_all(&backup);
    if plugin_dir.exists() {
        std::fs::rename(&plugin_dir, &backup)?;
    }
    match std::fs::rename(&staging, &plugin_dir) {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&backup);
            Ok(plugin_dir)
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            if backup.exists() {
                let _ = std::fs::rename(&backup, &plugin_dir);
            }
            Err(error)
        }
    }
}

fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Claude Code's project-directory slug for a working directory: `/` and `.`
/// replaced by `-`, verified against real dirs under `~/.claude/projects`.
pub fn claude_project_slug(cwd: &str) -> String {
    cwd.replace(['/', '.'], "-")
}

/// `~/.claude/projects/<slug>/<uuid>.jsonl` — predictable only for Claude,
/// because only Claude lets the caller choose the session UUID *and* derives
/// its jsonl path from the cwd.
pub fn claude_transcript_path(home: &Path, cwd: &str, session_uuid: &str) -> PathBuf {
    home.join(".claude/projects")
        .join(claude_project_slug(cwd))
        .join(format!("{session_uuid}.jsonl"))
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_are_v4_and_unique() {
        let a = uuid_v4();
        let b = uuid_v4();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'4', "version nibble: {a}");
        assert!(
            matches!(a.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
            "variant nibble: {a}"
        );
    }

    #[test]
    fn the_hooks_file_matches_the_swift_shape() {
        let temp = tempfile::tempdir().expect("temp");
        write_claude_hooks_file(temp.path()).expect("write");
        let parsed: serde_json::Value = serde_json::from_slice(
            &std::fs::read(temp.path().join("claude-hooks.json")).expect("read"),
        )
        .expect("parse");
        let stop = &parsed["hooks"]["Stop"][0]["hooks"][0];
        assert_eq!(stop["type"], "command");
        assert_eq!(stop["command"], "\"$ZEUS_CLI\" hook Stop");
        assert_eq!(stop["timeout"], 10);
        assert!(parsed["hooks"]["SubagentStop"].is_array());
    }

    #[test]
    fn the_mcp_file_prefers_the_sibling_proxy() {
        let temp = tempfile::tempdir().expect("temp");
        let cli = temp.path().join("bin/zeus");
        std::fs::create_dir_all(cli.parent().unwrap()).expect("mkdir");
        std::fs::write(&cli, "#!/bin/sh\n").expect("cli");

        // No proxy: fall back to `zeus mcp-stdio`.
        write_claude_mcp_file(temp.path(), &cli).expect("write");
        let parsed: serde_json::Value = serde_json::from_slice(
            &std::fs::read(temp.path().join("claude-mcp.json")).expect("read"),
        )
        .expect("parse");
        assert_eq!(parsed["mcpServers"]["zeus"]["args"][0], "mcp-stdio");

        // With an executable sibling, it becomes the command.
        let proxy = temp.path().join("bin/zeus-mcp");
        std::fs::write(&proxy, "#!/bin/sh\n").expect("proxy");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&proxy, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        write_claude_mcp_file(temp.path(), &cli).expect("write");
        let parsed: serde_json::Value = serde_json::from_slice(
            &std::fs::read(temp.path().join("claude-mcp.json")).expect("read"),
        )
        .expect("parse");
        assert_eq!(
            parsed["mcpServers"]["zeus"]["command"],
            proxy.to_string_lossy().as_ref()
        );
        assert_eq!(
            parsed["mcpServers"]["zeus"]["args"]
                .as_array()
                .map(Vec::len),
            Some(0)
        );
    }

    #[test]
    fn injection_args_include_provider_policy_and_native_spawn_guards() {
        let temp = tempfile::tempdir().expect("temp");
        let cli = temp.path().join("zeus");
        write_claude_hooks_file(temp.path()).expect("hooks");
        write_claude_mcp_file(temp.path(), &cli).expect("mcp");

        let claude = InjectionSpec {
            claude_hooks: true,
            claude_mcp: true,
            ..Default::default()
        };
        let args = injection_args(&claude, temp.path(), &cli);
        assert_eq!(args[0], "--settings");
        assert!(args[1].ends_with("claude-hooks.json"));
        assert_eq!(args[2], "--mcp-config");
        assert!(args[3].ends_with("claude-mcp.json"));
        assert!(
            args.windows(2).any(|pair| pair[0] == "--append-system-prompt"
                && pair[1] == HOSTED_SESSION_POLICY),
            "{args:?}"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--disallowedTools" && pair[1] == "Agent,Task"),
            "{args:?}"
        );

        let codex = InjectionSpec {
            codex_notify: true,
            codex_mcp: true,
            ..Default::default()
        };
        let args = injection_args(&codex, temp.path(), &cli);
        assert_eq!(args[0], "-c");
        assert!(args[1].starts_with("notify=["), "{args:?}");
        assert!(
            args.iter()
                .any(|arg| arg.starts_with("mcp_servers.zeus.command=")),
            "{args:?}"
        );
        assert!(
            args.iter().any(|arg| arg == "features.multi_agent=false"),
            "{args:?}"
        );
        assert!(
            args.iter()
                .any(|arg| arg.starts_with("developer_instructions=")),
            "{args:?}"
        );

        let grok = InjectionSpec {
            grok_mcp: true,
            ..Default::default()
        };
        let args = injection_args(&grok, temp.path(), &cli);
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--rules" && pair[1] == HOSTED_SESSION_POLICY),
            "{args:?}"
        );
        assert!(args.iter().any(|arg| arg == "--no-subagents"));
    }

    #[test]
    fn codex_mcp_bakes_session_env_so_created_sessions_nest_in_zeus() {
        let temp = tempfile::tempdir().expect("temp");
        let cli = temp.path().join("zeus");
        std::fs::write(&cli, b"#!/bin/sh\n").expect("cli");
        let socket = temp.path().join("d.sock");
        let spec = InjectionSpec {
            codex_mcp: true,
            ..Default::default()
        };
        let args = injection_args_with_cursor(
            &spec,
            temp.path(),
            &cli,
            Some(CursorInject {
                session_id: "s_parent",
                socket_path: &socket,
            }),
        );
        assert!(
            args.iter().any(|arg| arg == "features.multi_agent=false"),
            "{args:?}"
        );
        assert!(
            args.iter()
                .any(|arg| arg == "mcp_servers.zeus.command=\"/usr/bin/env\""),
            "{args:?}"
        );
        let encoded = args
            .iter()
            .find(|arg| arg.starts_with("mcp_servers.zeus.args="))
            .cloned()
            .unwrap_or_default();
        assert!(encoded.contains("ZEUS_SESSION_ID=s_parent"), "{encoded}");
        assert!(
            encoded.contains(&format!("ZEUS_SOCKET={}", socket.display())),
            "{encoded}"
        );
    }

    #[test]
    fn grok_opencode_and_gemini_get_session_local_zeus_mcp() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path().join("home");
        std::fs::create_dir_all(home.join(".gemini")).expect("gemini home");
        std::fs::write(home.join(".gemini/settings.json"), br#"{"theme":"dark"}"#)
            .expect("settings");
        std::fs::write(home.join(".gemini/oauth.json"), b"{}").expect("oauth");
        std::fs::write(home.join(".gemini/GEMINI.md"), b"keep user context")
            .expect("gemini context");
        let cli = temp.path().join("zeus");
        std::fs::write(&cli, b"#!/bin/sh\n").expect("cli");
        let socket = temp.path().join("d.sock");
        let identity = CursorInject {
            session_id: "s_host",
            socket_path: &socket,
        };
        let spec = InjectionSpec {
            grok_mcp: true,
            gemini_mcp: true,
            opencode_mcp: true,
            claude_mcp: true,
            ..Default::default()
        };
        write_claude_mcp_file(temp.path(), &cli).expect("static mcp");
        let previous_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", &home) };
        let env = injection_env(&spec, temp.path(), &cli, Some(identity));
        match previous_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let grok = env
            .iter()
            .find(|(key, _)| key == "GROK_CONFIG_PATH")
            .map(|(_, value)| value.clone())
            .expect("GROK_CONFIG_PATH");
        let grok_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&grok).expect("read grok")).expect("json");
        assert_eq!(grok_json["mcp_servers"]["zeus"]["command"], "/usr/bin/env");
        assert_eq!(
            grok_json["mcp_servers"]["zeus"]["args"][0],
            "ZEUS_SESSION_ID=s_host"
        );

        let opencode = env
            .iter()
            .find(|(key, _)| key == "OPENCODE_CONFIG")
            .map(|(_, value)| value.clone())
            .expect("OPENCODE_CONFIG");
        let opencode_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&opencode).expect("read opencode"))
                .expect("json");
        assert_eq!(opencode_json["mcp"]["zeus"]["enabled"], true);
        assert_eq!(opencode_json["mcp"]["zeus"]["command"][0], "/usr/bin/env");
        assert_eq!(opencode_json["permission"]["task"], "deny");
        let opencode_policy = opencode_json["instructions"][0]
            .as_str()
            .expect("policy path");
        assert_eq!(
            std::fs::read_to_string(opencode_policy).expect("policy"),
            HOSTED_SESSION_POLICY
        );

        let gemini = env
            .iter()
            .find(|(key, _)| key == "GEMINI_CLI_HOME")
            .map(|(_, value)| value.clone())
            .expect("GEMINI_CLI_HOME");
        let settings: serde_json::Value = serde_json::from_slice(
            &std::fs::read(Path::new(&gemini).join("settings.json")).expect("settings"),
        )
        .expect("json");
        assert_eq!(settings["theme"], "dark");
        assert_eq!(settings["mcpServers"]["zeus"]["command"], "/usr/bin/env");
        assert!(
            Path::new(&gemini).join("oauth.json").exists(),
            "user auth files are linked into the session home"
        );
        let gemini_context =
            std::fs::read_to_string(Path::new(&gemini).join("GEMINI.md")).expect("context");
        assert!(gemini_context.contains(HOSTED_SESSION_POLICY));
        assert!(gemini_context.contains("keep user context"));

        let claude = injection_args_with_cursor(&spec, temp.path(), &cli, Some(identity));
        let config = claude
            .iter()
            .position(|arg| arg == "--mcp-config")
            .map(|index| claude[index + 1].clone())
            .expect("claude --mcp-config");
        let claude_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config).expect("read claude")).expect("json");
        assert_eq!(
            claude_json["mcpServers"]["zeus"]["args"][0],
            "ZEUS_SESSION_ID=s_host"
        );
    }

    #[test]
    fn cursor_plugin_is_launch_scoped_with_baked_env_and_stop_hook() {
        let temp = tempfile::tempdir().expect("temp");
        let cli = temp.path().join("Application Support/zeus");
        std::fs::create_dir_all(cli.parent().unwrap()).expect("mkdir");
        std::fs::write(&cli, b"#!/bin/sh\n").expect("cli");
        let proxy = cli.with_file_name("zeus-mcp");
        std::fs::write(&proxy, b"#!/bin/sh\n").expect("proxy");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&cli).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&cli, perms).unwrap();
            std::fs::set_permissions(&proxy, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let socket = temp.path().join("d.sock");
        let cursor = InjectionSpec {
            cursor_mcp: true,
            cursor_hooks: true,
            ..Default::default()
        };
        let args = injection_args_with_cursor(
            &cursor,
            temp.path(),
            &cli,
            Some(CursorInject {
                session_id: "s_test",
                socket_path: &socket,
            }),
        );
        assert_eq!(args[0], "--plugin-dir");
        assert!(args[1].ends_with("cursor-plugin/s_test"), "{args:?}");
        assert!(args.iter().any(|arg| arg == "--approve-mcps"), "{args:?}");

        let plugin = Path::new(&args[1]);
        let mcp: serde_json::Value =
            serde_json::from_slice(&std::fs::read(plugin.join("mcp.json")).expect("mcp.json"))
                .expect("json");
        assert_eq!(mcp["mcpServers"]["zeus"]["command"], "/usr/bin/env");
        let mcp_args = mcp["mcpServers"]["zeus"]["args"].as_array().expect("args");
        assert_eq!(mcp_args[0], "ZEUS_SESSION_ID=s_test");
        assert!(
            mcp_args
                .iter()
                .any(|arg| arg == proxy.to_string_lossy().as_ref()),
            "{mcp_args:?}"
        );
        assert_eq!(
            mcp["mcpServers"]["zeus"]["env"]["ZEUS_SESSION_ID"],
            "s_test"
        );
        assert_eq!(
            mcp["mcpServers"]["zeus"]["env"]["ZEUS_SOCKET"],
            socket.to_string_lossy().as_ref()
        );
        let rule = std::fs::read_to_string(plugin.join("rules/zeus-orchestration.mdc"))
            .expect("policy rule");
        assert!(rule.contains("alwaysApply: true"));
        assert!(rule.contains(HOSTED_SESSION_POLICY));

        let hooks: serde_json::Value =
            serde_json::from_slice(&std::fs::read(plugin.join("hooks/hooks.json")).expect("hooks"))
                .expect("json");
        let stop = hooks["hooks"]["stop"][0]["command"].as_str().expect("cmd");
        assert!(stop.contains("hook Stop"), "{stop}");
        assert!(stop.contains("zeus"), "{stop}");
    }

    #[test]
    fn cursor_plugin_rewrite_drops_disabled_components() {
        let temp = tempfile::tempdir().expect("temp");
        let cli = temp.path().join("bin/zeus");
        std::fs::create_dir_all(cli.parent().unwrap()).expect("mkdir");
        std::fs::write(&cli, b"#!/bin/sh\n").expect("cli");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let socket = temp.path().join("d.sock");
        let both = InjectionSpec {
            cursor_mcp: true,
            cursor_hooks: true,
            ..Default::default()
        };
        let args = injection_args_with_cursor(
            &both,
            temp.path(),
            &cli,
            Some(CursorInject {
                session_id: "s_reuse",
                socket_path: &socket,
            }),
        );
        let plugin = Path::new(&args[1]);
        assert!(plugin.join("mcp.json").is_file());
        assert!(plugin.join("hooks/hooks.json").is_file());
        assert!(plugin.join("rules/zeus-orchestration.mdc").is_file());

        let hooks_only = InjectionSpec {
            cursor_mcp: false,
            cursor_hooks: true,
            ..Default::default()
        };
        let _ = injection_args_with_cursor(
            &hooks_only,
            temp.path(),
            &cli,
            Some(CursorInject {
                session_id: "s_reuse",
                socket_path: &socket,
            }),
        );
        assert!(
            !plugin.join("mcp.json").exists(),
            "disabled mcp.json must not survive a rewrite"
        );
        assert!(
            !plugin.join("rules/zeus-orchestration.mdc").exists(),
            "disabled MCP policy rule must not survive a rewrite"
        );
        assert!(plugin.join("hooks/hooks.json").is_file());
    }

    #[test]
    fn the_transcript_slug_matches_claudes_rule() {
        assert_eq!(
            claude_project_slug("/Users/giga/.claude/worktrees/x"),
            "-Users-giga--claude-worktrees-x"
        );
        let path = claude_transcript_path(Path::new("/Users/giga"), "/tmp/repo", "abc-123");
        assert_eq!(
            path,
            Path::new("/Users/giga/.claude/projects/-tmp-repo/abc-123.jsonl")
        );
    }
}
