use crate::catalog::parse_kind;
use crate::error::CliError;
use crate::render::ALL_EVENT_KINDS;
use crate::support::WAIT_TARGETS;
use zeus_proto::AgentKind;

#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub json: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Help(String),
    SessionList {
        output: Output,
        status: Option<String>,
        all: bool,
    },
    SessionGet {
        output: Output,
        session: String,
    },
    SessionRead {
        output: Output,
        session: String,
        source: String,
        lines: Option<i64>,
    },
    SessionSend {
        output: Output,
        session: String,
        text: Vec<String>,
        no_submit: bool,
    },
    SessionWait {
        output: Output,
        session: String,
        until: Vec<String>,
        timeout: f64,
    },
    SessionSpawn {
        output: Output,
        kind: String,
        cwd: Option<String>,
        title: Option<String>,
        prompt: Option<String>,
        worktree: bool,
        branch: Option<String>,
        host: Option<String>,
    },
    SessionRelease {
        output: Output,
        session: String,
        remove: bool,
    },
    SessionArchive {
        output: Output,
        session: String,
        undo: bool,
    },
    Artifacts {
        output: Output,
        session: String,
    },
    WorktreeList {
        output: Output,
        repo: Option<String>,
    },
    WorktreeCreate {
        output: Output,
        repo: Option<String>,
        branch: Option<String>,
        base: Option<String>,
    },
    WorktreeRemove {
        output: Output,
        repo: String,
        path: String,
        force: bool,
    },
    EventsSubscribe {
        output: Output,
        session: Vec<String>,
        kind: Vec<String>,
        since_seq: Option<u64>,
        count: Option<i64>,
    },
    EventsWait {
        output: Output,
        session: Option<String>,
        until: Vec<String>,
        kind: Vec<String>,
        timeout: f64,
    },
    Ports {
        output: Output,
    },
    Forward,
    Hook {
        event: String,
    },
    Notify {
        args: Vec<String>,
    },
    McpStdio,
    McpTools,
    McpCall {
        tool: String,
    },
    Doctor,
}

impl Command {
    pub fn parse(argv: &[String]) -> Result<Self, CliError> {
        if argv.is_empty() || argv.iter().any(|arg| arg == "--help" || arg == "-h") {
            return Ok(Self::Help(USAGE.into()));
        }
        match argv[0].as_str() {
            "session" => parse_session(&argv[1..]),
            "worktree" => parse_worktree(&argv[1..]),
            "artifacts" => {
                let parsed = Flags::parse(&argv[1..], &["json"], &[])?;
                let session = parsed
                    .positionals
                    .first()
                    .cloned()
                    .ok_or_else(|| CliError::Usage("artifacts needs a session".into()))?;
                Ok(Self::Artifacts {
                    output: parsed.output(),
                    session,
                })
            }
            "events" => parse_events(&argv[1..]),
            "status" => {
                let parsed = Flags::parse(&argv[1..], &["json"], &[])?;
                Ok(Self::SessionList {
                    output: parsed.output(),
                    status: None,
                    all: true,
                })
            }
            "ports" => {
                let parsed = Flags::parse(&argv[1..], &["json"], &[])?;
                Ok(Self::Ports {
                    output: parsed.output(),
                })
            }
            "forward" => Ok(Self::Forward),
            "hook" => {
                let event = argv
                    .get(1)
                    .cloned()
                    .ok_or_else(|| CliError::Usage("hook needs an event name".into()))?;
                Ok(Self::Hook { event })
            }
            "notify" => Ok(Self::Notify {
                args: argv[1..].to_vec(),
            }),
            "mcp-stdio" => Ok(Self::McpStdio),
            "mcp-tools" => Ok(Self::McpTools),
            "mcp-call" => {
                let parsed = Flags::parse(&argv[1..], &[], &["tool"])?;
                let tool = parsed
                    .option("tool")
                    .ok_or_else(|| CliError::Usage("mcp-call needs --tool".into()))?;
                Ok(Self::McpCall { tool })
            }
            "doctor" => Ok(Self::Doctor),
            other => Err(CliError::Usage(format!("unknown command: {other}"))),
        }
    }

    pub fn parsed_kind(&self) -> Option<AgentKind> {
        match self {
            Self::SessionSpawn { kind, .. } => Some(parse_kind(kind)),
            _ => None,
        }
    }
}

const USAGE: &str = "\
zeus <resource> <action> [target] [options]

Resources:
  session     list | get | read | send | wait | spawn | release | archive
  worktree    list | create | remove
  artifacts   <session>
  events      subscribe | wait

Integration:
  status      alias of session list --all
  hook        forward a Claude hook (fail-open)
  notify      Codex notify target (fail-open)
  mcp-stdio   MCP stdio server
  mcp-tools   MCP tool catalog
  mcp-call    MCP one-shot tool
  doctor      check the daemon and agent binaries
  ports       listening ports from session records
";

struct Flags {
    positionals: Vec<String>,
    flags: Vec<String>,
    options: Vec<(String, String)>,
}

impl Flags {
    fn parse(argv: &[String], flags: &[&str], options: &[&str]) -> Result<Self, CliError> {
        let mut positionals = Vec::new();
        let mut found_flags = Vec::new();
        let mut found_options = Vec::new();
        let mut index = 0;
        while index < argv.len() {
            let arg = &argv[index];
            if let Some(name) = arg.strip_prefix("--") {
                if let Some((key, value)) = name.split_once('=') {
                    if !options.contains(&key) {
                        return Err(CliError::Usage(format!("unknown option --{key}")));
                    }
                    found_options.push((key.to_string(), value.to_string()));
                    index += 1;
                    continue;
                }
                if flags.contains(&name) {
                    found_flags.push(name.to_string());
                    index += 1;
                    continue;
                }
                if options.contains(&name) {
                    let value = argv
                        .get(index + 1)
                        .cloned()
                        .ok_or_else(|| CliError::Usage(format!("--{name} needs a value")))?;
                    found_options.push((name.to_string(), value));
                    index += 2;
                    continue;
                }
                return Err(CliError::Usage(format!("unknown option --{name}")));
            }
            if arg.starts_with('-') && arg != "-" {
                return Err(CliError::Usage(format!("unknown option {arg}")));
            }
            positionals.push(arg.clone());
            index += 1;
        }
        Ok(Self {
            positionals,
            flags: found_flags,
            options: found_options,
        })
    }

    fn output(&self) -> Output {
        Output {
            json: self.has("json"),
        }
    }

    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }

    fn option(&self, name: &str) -> Option<String> {
        self.options
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    }

    fn options(&self, name: &str) -> Vec<String> {
        self.options
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .collect()
    }
}

fn parse_session(argv: &[String]) -> Result<Command, CliError> {
    let (action, rest) = split_action(argv, "list");
    match action {
        "list" => {
            let parsed = Flags::parse(rest, &["json", "all"], &["status"])?;
            Ok(Command::SessionList {
                output: parsed.output(),
                status: parsed.option("status"),
                all: parsed.has("all"),
            })
        }
        "get" => {
            let parsed = Flags::parse(rest, &["json"], &[])?;
            Ok(Command::SessionGet {
                output: parsed.output(),
                session: require_one(&parsed.positionals, "session get")?,
            })
        }
        "read" => {
            let parsed = Flags::parse(rest, &["json"], &["source", "lines"])?;
            let source = parsed.option("source").unwrap_or_else(|| "screen".into());
            if source != "screen" && source != "scrollback" {
                return Err(CliError::Usage(
                    "--source must be \"screen\" or \"scrollback\"".into(),
                ));
            }
            Ok(Command::SessionRead {
                output: parsed.output(),
                session: require_one(&parsed.positionals, "session read")?,
                source,
                lines: parsed
                    .option("lines")
                    .map(|value| parse_i64(&value, "--lines"))
                    .transpose()?,
            })
        }
        "send" => {
            let parsed = Flags::parse(rest, &["json", "no-submit"], &[])?;
            let session = parsed
                .positionals
                .first()
                .cloned()
                .ok_or_else(|| CliError::Usage("session send needs a session".into()))?;
            Ok(Command::SessionSend {
                output: parsed.output(),
                session,
                text: parsed.positionals.iter().skip(1).cloned().collect(),
                no_submit: parsed.has("no-submit"),
            })
        }
        "wait" => {
            let parsed = Flags::parse(rest, &["json"], &["until", "timeout"])?;
            let until = parsed.options("until");
            Ok(Command::SessionWait {
                output: parsed.output(),
                session: require_one(&parsed.positionals, "session wait")?,
                until: if until.is_empty() {
                    vec!["done".into()]
                } else {
                    until
                },
                timeout: parsed
                    .option("timeout")
                    .map(|value| parse_f64(&value, "--timeout"))
                    .transpose()?
                    .unwrap_or(600.0),
            })
        }
        "spawn" => {
            let parsed = Flags::parse(
                rest,
                &["json", "worktree"],
                &["cwd", "title", "prompt", "branch", "host"],
            )?;
            Ok(Command::SessionSpawn {
                output: parsed.output(),
                kind: require_one(&parsed.positionals, "session spawn")?,
                cwd: parsed.option("cwd"),
                title: parsed.option("title"),
                prompt: parsed.option("prompt"),
                worktree: parsed.has("worktree"),
                branch: parsed.option("branch"),
                host: parsed.option("host"),
            })
        }
        "release" => {
            let parsed = Flags::parse(rest, &["json", "remove"], &[])?;
            Ok(Command::SessionRelease {
                output: parsed.output(),
                session: require_one(&parsed.positionals, "session release")?,
                remove: parsed.has("remove"),
            })
        }
        "archive" => {
            let parsed = Flags::parse(rest, &["json", "undo"], &[])?;
            Ok(Command::SessionArchive {
                output: parsed.output(),
                session: require_one(&parsed.positionals, "session archive")?,
                undo: parsed.has("undo"),
            })
        }
        other => Err(CliError::Usage(format!("unknown session action: {other}"))),
    }
}

fn parse_worktree(argv: &[String]) -> Result<Command, CliError> {
    let (action, rest) = split_action(argv, "list");
    match action {
        "list" => {
            let parsed = Flags::parse(rest, &["json"], &[])?;
            Ok(Command::WorktreeList {
                output: parsed.output(),
                repo: parsed.positionals.first().cloned(),
            })
        }
        "create" => {
            let parsed = Flags::parse(rest, &["json"], &["branch", "base"])?;
            Ok(Command::WorktreeCreate {
                output: parsed.output(),
                repo: parsed.positionals.first().cloned(),
                branch: parsed.option("branch"),
                base: parsed.option("base"),
            })
        }
        "remove" => {
            let parsed = Flags::parse(rest, &["json", "force"], &[])?;
            if parsed.positionals.len() < 2 {
                return Err(CliError::Usage(
                    "worktree remove needs a repo and a path".into(),
                ));
            }
            Ok(Command::WorktreeRemove {
                output: parsed.output(),
                repo: parsed.positionals[0].clone(),
                path: parsed.positionals[1].clone(),
                force: parsed.has("force"),
            })
        }
        other => Err(CliError::Usage(format!("unknown worktree action: {other}"))),
    }
}

fn parse_events(argv: &[String]) -> Result<Command, CliError> {
    let (action, rest) = split_action(argv, "subscribe");
    match action {
        "subscribe" => {
            let parsed = Flags::parse(rest, &["json"], &["session", "kind", "since-seq", "count"])?;
            let kind = parsed.options("kind");
            validate_kinds(&kind)?;
            Ok(Command::EventsSubscribe {
                output: parsed.output(),
                session: parsed.options("session"),
                kind,
                since_seq: parsed
                    .option("since-seq")
                    .map(|value| parse_u64(&value, "--since-seq"))
                    .transpose()?,
                count: parsed
                    .option("count")
                    .map(|value| parse_i64(&value, "--count"))
                    .transpose()?,
            })
        }
        "wait" => {
            let parsed = Flags::parse(rest, &["json"], &["session", "until", "kind", "timeout"])?;
            let until = parsed.options("until");
            let kind = parsed.options("kind");
            if until.is_empty() && kind.is_empty() {
                return Err(CliError::Usage(
                    "pass at least one --until <status> or --kind <event>".into(),
                ));
            }
            if !until.is_empty() && parsed.option("session").is_none() {
                return Err(CliError::Usage(
                    "--until needs --session (a status is a property of one session)".into(),
                ));
            }
            validate_kinds(&kind)?;
            Ok(Command::EventsWait {
                output: parsed.output(),
                session: parsed.option("session"),
                until,
                kind,
                timeout: parsed
                    .option("timeout")
                    .map(|value| parse_f64(&value, "--timeout"))
                    .transpose()?
                    .unwrap_or(600.0),
            })
        }
        other => Err(CliError::Usage(format!("unknown events action: {other}"))),
    }
}

fn split_action<'a>(argv: &'a [String], default: &'a str) -> (&'a str, &'a [String]) {
    match argv.first().map(String::as_str) {
        None => (default, argv),
        Some(first) if first.starts_with('-') => (default, argv),
        Some(first) => (first, &argv[1..]),
    }
}

fn require_one(positionals: &[String], name: &str) -> Result<String, CliError> {
    positionals
        .first()
        .cloned()
        .ok_or_else(|| CliError::Usage(format!("{name} needs a target")))
}

fn validate_kinds(kinds: &[String]) -> Result<(), CliError> {
    for name in kinds {
        if !ALL_EVENT_KINDS.contains(&name.as_str()) {
            return Err(CliError::Usage(format!(
                "unknown event kind \"{name}\"; expected one of: {}",
                ALL_EVENT_KINDS.join(", ")
            )));
        }
    }
    Ok(())
}

fn parse_i64(value: &str, flag: &str) -> Result<i64, CliError> {
    value
        .parse()
        .map_err(|_| CliError::Usage(format!("{flag} must be an integer")))
}

fn parse_u64(value: &str, flag: &str) -> Result<u64, CliError> {
    value
        .parse()
        .map_err(|_| CliError::Usage(format!("{flag} must be an integer")))
}

fn parse_f64(value: &str, flag: &str) -> Result<f64, CliError> {
    value
        .parse()
        .map_err(|_| CliError::Usage(format!("{flag} must be a number")))
}

pub fn wait_targets() -> &'static [&'static str] {
    WAIT_TARGETS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> Command {
        Command::parse(
            &argv
                .iter()
                .map(|item| (*item).to_string())
                .collect::<Vec<_>>(),
        )
        .expect("parse")
    }

    fn parse_err(argv: &[&str]) -> bool {
        Command::parse(
            &argv
                .iter()
                .map(|item| (*item).to_string())
                .collect::<Vec<_>>(),
        )
        .is_err()
    }

    #[test]
    fn root_exposes_resource_groups_and_legacy_entry_points() {
        assert!(matches!(
            parse(&["session", "list"]),
            Command::SessionList { .. }
        ));
        assert!(matches!(
            parse(&["worktree", "list"]),
            Command::WorktreeList { .. }
        ));
        assert!(matches!(
            parse(&["artifacts", "s_a"]),
            Command::Artifacts { .. }
        ));
        assert!(matches!(
            parse(&["events", "subscribe"]),
            Command::EventsSubscribe { .. }
        ));
        assert!(matches!(
            parse(&["status"]),
            Command::SessionList { all: true, .. }
        ));
        assert!(matches!(parse(&["hook", "Stop"]), Command::Hook { .. }));
        assert!(matches!(parse(&["notify", "{}"]), Command::Notify { .. }));
        assert!(matches!(parse(&["doctor"]), Command::Doctor));
        assert!(matches!(parse(&["mcp-stdio"]), Command::McpStdio));
        assert!(matches!(parse(&["mcp-tools"]), Command::McpTools));
        assert!(matches!(
            parse(&["mcp-call", "--tool", "list_agents"]),
            Command::McpCall { .. }
        ));
    }

    #[test]
    fn bare_resource_falls_back_to_list() {
        assert!(matches!(
            parse(&["session"]),
            Command::SessionList { all: false, .. }
        ));
        assert!(matches!(parse(&["worktree"]), Command::WorktreeList { .. }));
        assert!(matches!(
            parse(&["events"]),
            Command::EventsSubscribe { .. }
        ));
    }

    #[test]
    fn session_list_takes_filters_and_json() {
        let Command::SessionList {
            output,
            status,
            all,
        } = parse(&["session", "list", "--json", "--status", "working"])
        else {
            panic!("expected list");
        };
        assert!(output.json);
        assert_eq!(status.as_deref(), Some("working"));
        assert!(!all);
    }

    #[test]
    fn session_read_defaults_to_the_live_screen() {
        let Command::SessionRead {
            session,
            source,
            lines,
            output,
        } = parse(&["session", "read", "s_ab"])
        else {
            panic!("expected read");
        };
        assert_eq!(session, "s_ab");
        assert_eq!(source, "screen");
        assert_eq!(lines, None);
        assert!(!output.json);

        let Command::SessionRead {
            source,
            lines,
            output,
            ..
        } = parse(&[
            "session",
            "read",
            "s_ab",
            "--source",
            "scrollback",
            "--lines",
            "50",
            "--json",
        ])
        else {
            panic!("expected read");
        };
        assert_eq!(source, "scrollback");
        assert_eq!(lines, Some(50));
        assert!(output.json);
    }

    #[test]
    fn session_send_collects_remaining_argv() {
        let Command::SessionSend {
            session,
            text,
            no_submit,
            ..
        } = parse(&["session", "send", "s_ab", "run", "the", "tests"])
        else {
            panic!("expected send");
        };
        assert_eq!(session, "s_ab");
        assert_eq!(text, ["run", "the", "tests"]);
        assert!(!no_submit);

        let Command::SessionSend {
            no_submit, text, ..
        } = parse(&["session", "send", "s_ab", "--no-submit", "2"])
        else {
            panic!("expected send");
        };
        assert!(no_submit);
        assert_eq!(text, ["2"]);
    }

    #[test]
    fn session_wait_defaults_to_done() {
        let Command::SessionWait { until, timeout, .. } = parse(&["session", "wait", "s_ab"])
        else {
            panic!("expected wait");
        };
        assert_eq!(until, ["done"]);
        assert_eq!(timeout, 600.0);

        let Command::SessionWait { until, timeout, .. } = parse(&[
            "session",
            "wait",
            "s_ab",
            "--until",
            "done",
            "--until",
            "needs-input",
            "--timeout",
            "30",
        ]) else {
            panic!("expected wait");
        };
        assert_eq!(until, ["done", "needs-input"]);
        assert_eq!(timeout, 30.0);
        for target in &until {
            assert!(WAIT_TARGETS.contains(&target.as_str()));
        }
    }

    #[test]
    fn session_spawn_maps_agent_names() {
        let command = parse(&[
            "session",
            "spawn",
            "claude",
            "--cwd",
            "/tmp/x",
            "--worktree",
            "--prompt",
            "go",
        ]);
        let Command::SessionSpawn {
            cwd,
            worktree,
            prompt,
            kind,
            ..
        } = command
        else {
            panic!("expected spawn");
        };
        assert_eq!(parse_kind(&kind), AgentKind::CLAUDE_CODE);
        assert_eq!(cwd.as_deref(), Some("/tmp/x"));
        assert!(worktree);
        assert_eq!(prompt.as_deref(), Some("go"));
        assert_eq!(parse_kind("codex"), AgentKind::CODEX);
        assert_eq!(parse_kind("shell"), AgentKind::SHELL);
        assert_eq!(parse_kind("htop").command(), Some("htop"));
    }

    #[test]
    fn session_release_and_archive_flags() {
        let Command::SessionRelease { remove, .. } = parse(&["session", "release", "s_ab"]) else {
            panic!("expected release");
        };
        assert!(!remove);
        let Command::SessionRelease { remove, .. } =
            parse(&["session", "release", "s_ab", "--remove"])
        else {
            panic!("expected release");
        };
        assert!(remove);
        let Command::SessionArchive { undo, .. } = parse(&["session", "archive", "s_ab", "--undo"])
        else {
            panic!("expected archive");
        };
        assert!(undo);
    }

    #[test]
    fn worktree_and_artifacts_parse() {
        let Command::WorktreeCreate {
            repo, branch, base, ..
        } = parse(&[
            "worktree", "create", "/repo", "--branch", "feat/x", "--base", "main",
        ])
        else {
            panic!("expected create");
        };
        assert_eq!(repo.as_deref(), Some("/repo"));
        assert_eq!(branch.as_deref(), Some("feat/x"));
        assert_eq!(base.as_deref(), Some("main"));

        let Command::WorktreeRemove {
            repo, path, force, ..
        } = parse(&["worktree", "remove", "/repo", "/repo/../wt", "--force"])
        else {
            panic!("expected remove");
        };
        assert_eq!(repo, "/repo");
        assert_eq!(path, "/repo/../wt");
        assert!(force);

        let Command::WorktreeList { repo, .. } = parse(&["worktree", "list"]) else {
            panic!("expected list");
        };
        assert_eq!(repo, None);

        let Command::Artifacts {
            session, output, ..
        } = parse(&["artifacts", "s_ab", "--json"])
        else {
            panic!("expected artifacts");
        };
        assert_eq!(session, "s_ab");
        assert!(output.json);
    }

    #[test]
    fn events_subscribe_and_wait_validate() {
        let Command::EventsSubscribe {
            session,
            kind,
            since_seq,
            output,
            ..
        } = parse(&[
            "events",
            "subscribe",
            "--session",
            "s_a",
            "--session",
            "s_b",
            "--kind",
            "session.status",
            "--kind",
            "session.needs_input",
            "--since-seq",
            "42",
            "--json",
        ])
        else {
            panic!("expected subscribe");
        };
        assert_eq!(session, ["s_a", "s_b"]);
        assert_eq!(kind, ["session.status", "session.needs_input"]);
        assert_eq!(since_seq, Some(42));
        assert!(output.json);
        assert!(parse_err(&[
            "events",
            "subscribe",
            "--kind",
            "session.nope"
        ]));
        assert!(parse_err(&["events", "wait"]));
        assert!(parse_err(&["events", "wait", "--until", "done"]));
        let Command::EventsWait { session, .. } =
            parse(&["events", "wait", "--session", "s_a", "--until", "done"])
        else {
            panic!("expected wait");
        };
        assert_eq!(session.as_deref(), Some("s_a"));
        let Command::EventsWait {
            timeout, session, ..
        } = parse(&[
            "events",
            "wait",
            "--kind",
            "session.artifact",
            "--timeout",
            "45",
        ])
        else {
            panic!("expected wait");
        };
        assert_eq!(timeout, 45.0);
        assert_eq!(session, None);
        for name in ALL_EVENT_KINDS {
            assert!(!parse_err(&["events", "subscribe", "--kind", name]));
        }
    }

    #[test]
    fn exit_codes_are_distinct() {
        use crate::error::{EXIT_FAILURE, EXIT_NOT_FOUND, EXIT_TIMEOUT, EXIT_UNREACHABLE};
        let codes = [EXIT_FAILURE, EXIT_TIMEOUT, EXIT_NOT_FOUND, EXIT_UNREACHABLE];
        assert_eq!(codes.len(), 4);
        assert_eq!(EXIT_TIMEOUT, 2);
        assert!(codes.iter().all(|code| *code != 0));
    }
}
