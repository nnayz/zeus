//! Running a session on another machine, over ssh and tmux.
//!
//! The local PTY runs `ssh`, which runs `tmux` on the remote host. tmux is what
//! keeps the agent alive across an SSH drop, and `new-session -A` makes the
//! exact same argv a *reattach* when that tmux session already exists — which
//! is what turns a daemon restart, a dropped connection and an explicit resume
//! into one code path.
//!
//! Quoting is the whole difficulty here. ssh joins its command arguments with
//! spaces and hands the result to the remote login shell, so every element has
//! to survive that second parse. See [`remote_argv`].
//!
//! Ported from the remote half of the Swift `InjectionBuilder`.

use diri_proto::HostEntry;

use crate::agent::AgentDescriptor;

/// The tmux session name on the remote host.
///
/// Derived from the persisted session id, so respawning the same diri session
/// reattaches its remote tmux rather than starting a second agent beside it.
pub fn remote_tmux_session_name(session_id: &str) -> String {
    let raw = session_id.strip_prefix("s_").unwrap_or(session_id);
    format!("diri-{}", raw.chars().take(8).collect::<String>())
}

/// Single-quotes a string for a POSIX shell.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Quotes a path while keeping a leading `~` expandable by the remote shell.
///
/// `~/my code` has to become `~/'my code'`: quoting the whole thing would send
/// a literal tilde and land the session in a directory called `~`.
pub fn shell_quote_path(path: &str) -> String {
    if path == "~" {
        return path.to_string();
    }
    match path.strip_prefix("~/") {
        Some(rest) => format!("~/{}", shell_quote(rest)),
        None => shell_quote(path),
    }
}

/// The plain agent command to run on the remote host.
///
/// Deliberately without hook or MCP injection: those flags reference local
/// paths that do not exist on the other machine. `None` means "no command", and
/// tmux then starts the remote user's login shell — exactly what a shell
/// session wants.
pub fn remote_agent_command(
    manifest_id: &str,
    descriptor: &AgentDescriptor,
    agent_session_id: Option<&str>,
    resume: bool,
) -> Option<String> {
    if manifest_id == "shell" {
        return None;
    }
    let binary = descriptor.binary.clone()?;
    let mut words = vec![binary];

    if let Some(id) = agent_session_id {
        if resume {
            if let Some(arguments) = descriptor.resume_args(Some(id)) {
                words.extend(arguments);
            }
        } else if let Some(flag) = &descriptor.session_id_flag {
            words.push(flag.clone());
            words.push(id.to_string());
        }
    }

    let command = words.join(" ");
    // Exiting the agent should drop to a shell in the same tmux window rather
    // than ending the window and with it the session.
    Some(if descriptor.return_to_login_shell {
        format!("{command}; exec \"${{SHELL:-bash}}\" -l")
    } else {
        command
    })
}

/// argv for the local PTY: ssh runs tmux on the remote host.
pub fn remote_argv(
    manifest_id: &str,
    descriptor: &AgentDescriptor,
    session_id: &str,
    host: &HostEntry,
    remote_cwd: &str,
    agent_session_id: Option<&str>,
    resume: bool,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "ssh".into(),
        "-t".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        // Keepalives, so an idle session survives NAT and Tailscale idle
        // timeouts instead of dying with "connection reset by peer". tmux
        // outlives a genuine disconnect; this prevents the gratuitous ones.
        "-o".into(),
        "ServerAliveInterval=20".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
        "-o".into(),
        "TCPKeepAlive=yes".into(),
        host.ssh.clone(),
        "--".into(),
        "tmux".into(),
        "new-session".into(),
        "-A".into(),
        "-s".into(),
        remote_tmux_session_name(session_id),
        "-c".into(),
        shell_quote_path(remote_cwd),
    ];

    if let Some(command) = remote_agent_command(manifest_id, descriptor, agent_session_id, resume) {
        // One pre-quoted word, because the remote shell parses this string
        // again before tmux ever sees it.
        argv.push(shell_quote(&command));
    }
    // `\;` survives the remote shell as a literal `;`, which is how tmux is
    // given a second command — here, hiding its status bar inside our PTY.
    argv.extend(["\\;".into(), "set".into(), "status".into(), "off".into()]);
    argv
}

/// argv that copies a file to or from a host with `scp`.
///
/// Used by session handoff to move a transcript between machines, so a
/// conversation continues rather than restarting.
pub fn copy_argv(
    from: &str,
    from_host: Option<&HostEntry>,
    to: &str,
    to_host: Option<&HostEntry>,
) -> Vec<String> {
    let endpoint = |path: &str, host: Option<&HostEntry>| match host {
        Some(host) => format!("{}:{}", host.ssh, path),
        None => path.to_string(),
    };
    vec![
        "scp".into(),
        "-q".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        endpoint(from, from_host),
        endpoint(to, to_host),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{ResumeSpec, StatusAuthority};

    fn host() -> HostEntry {
        HostEntry {
            id: "forge".into(),
            name: Some("Forge".into()),
            ssh: "someone@forge".into(),
            default_cwd: Some("~/code".into()),
            node: None,
        }
    }

    fn claude_like() -> AgentDescriptor {
        AgentDescriptor {
            binary: Some("claude".into()),
            return_to_login_shell: true,
            session_id_flag: Some("--session-id".into()),
            resume: Some(ResumeSpec {
                style: "flag".into(),
                token: Some("--resume".into()),
            }),
            status_authority: Some(StatusAuthority::Hooks),
            ..Default::default()
        }
    }

    #[test]
    fn the_tmux_name_derives_from_the_session_so_respawn_reattaches() {
        // Same session id must produce the same tmux name, or a restart starts
        // a second agent beside the live one instead of reattaching.
        let first = remote_tmux_session_name("s_4b99600fd4f1");
        let second = remote_tmux_session_name("s_4b99600fd4f1");
        assert_eq!(first, second);
        assert_eq!(first, "diri-4b99600f");
        assert!(!first.contains("s_"), "the prefix is stripped");
    }

    #[test]
    fn a_tilde_path_stays_expandable() {
        // Quoting the whole thing would send a literal tilde and land the
        // session in a directory named `~`.
        assert_eq!(shell_quote_path("~/code/app"), "~/'code/app'");
        assert_eq!(shell_quote_path("~"), "~");
        assert_eq!(shell_quote_path("/abs/path"), "'/abs/path'");
    }

    #[test]
    fn a_path_with_a_quote_cannot_break_out() {
        let quoted = shell_quote_path("~/it's here");
        assert_eq!(quoted, r"~/'it'\''s here'");

        // What a shell actually sees: '...' + escaped quote + '...' → one word.
        let echoed = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("printf %s {quoted}")])
            .env("HOME", "/tmp")
            .output()
            .expect("sh");
        assert_eq!(
            String::from_utf8_lossy(&echoed.stdout),
            "/tmp/it's here",
            "the quoting must round-trip through a real shell"
        );
    }

    #[test]
    fn a_remote_claude_gets_a_plain_command_with_no_local_injection() {
        // Hook and MCP flags reference local paths that do not exist on the
        // other machine; sending them would fail the launch.
        let command = remote_agent_command("claude-code", &claude_like(), Some("abc"), false)
            .expect("a command");
        assert!(command.starts_with("claude --session-id abc"));
        assert!(
            !command.contains("--settings"),
            "no hook injection: {command}"
        );
        assert!(!command.contains("mcp"), "no MCP injection: {command}");
        assert!(
            command.ends_with("; exec \"${SHELL:-bash}\" -l"),
            "exiting the agent should leave a shell behind: {command}"
        );
    }

    #[test]
    fn resuming_uses_the_resume_flag_rather_than_a_fresh_id() {
        let command = remote_agent_command("claude-code", &claude_like(), Some("abc"), true)
            .expect("command");
        assert!(command.contains("--resume abc"), "{command}");
        assert!(!command.contains("--session-id"), "{command}");
    }

    #[test]
    fn a_remote_shell_gets_no_command_at_all() {
        // tmux then starts the remote user's own login shell.
        let descriptor = AgentDescriptor {
            status_authority: Some(StatusAuthority::Process),
            ..Default::default()
        };
        assert!(remote_agent_command("shell", &descriptor, None, false).is_none());
    }

    #[test]
    fn the_argv_reattaches_and_hides_the_status_bar() {
        let argv = remote_argv(
            "claude-code",
            &claude_like(),
            "s_abcdef123456",
            &host(),
            "~/code/app",
            Some("uuid-1"),
            false,
        );

        assert_eq!(argv[0], "ssh");
        assert!(argv.contains(&"someone@forge".to_string()));
        assert!(
            argv.windows(3)
                .any(|window| window == ["tmux", "new-session", "-A"]),
            "-A is what makes a respawn reattach: {argv:?}"
        );
        assert!(argv.contains(&"diri-abcdef12".to_string()));
        assert!(argv.contains(&"~/'code/app'".to_string()));
        assert_eq!(
            &argv[argv.len() - 4..],
            &["\\;", "set", "status", "off"],
            "the literal ; reaches tmux as a command separator"
        );
    }

    #[test]
    fn keepalives_are_set_so_idle_sessions_survive() {
        let argv = remote_argv(
            "shell",
            &AgentDescriptor::default(),
            "s_1",
            &host(),
            "~",
            None,
            false,
        );
        assert!(argv.contains(&"ServerAliveInterval=20".to_string()));
        assert!(argv.contains(&"TCPKeepAlive=yes".to_string()));
    }

    #[test]
    fn the_agent_command_is_one_quoted_word() {
        // ssh hands its arguments to the remote shell as one string; an
        // unquoted command would be re-split there.
        let argv = remote_argv(
            "claude-code",
            &claude_like(),
            "s_1",
            &host(),
            "~",
            Some("uuid"),
            false,
        );
        let command = argv
            .iter()
            .find(|word| word.contains("claude"))
            .expect("the agent command");
        assert!(command.starts_with('\''), "not quoted: {command}");
        assert!(command.ends_with('\''), "not quoted: {command}");
    }

    #[test]
    fn copy_argv_addresses_each_side_correctly() {
        let forge = host();

        let push = copy_argv("/local/file", None, "~/remote/file", Some(&forge));
        assert_eq!(push.last().unwrap(), "someone@forge:~/remote/file");
        assert_eq!(push[push.len() - 2], "/local/file");

        let pull = copy_argv("~/remote/file", Some(&forge), "/local/file", None);
        assert_eq!(pull[pull.len() - 2], "someone@forge:~/remote/file");
        assert_eq!(pull.last().unwrap(), "/local/file");

        let across = copy_argv("/a", Some(&forge), "/b", Some(&forge));
        assert_eq!(across[across.len() - 2], "someone@forge:/a");
        assert_eq!(across.last().unwrap(), "someone@forge:/b");
    }
}
