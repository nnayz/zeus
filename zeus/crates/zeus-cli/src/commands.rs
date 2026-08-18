use std::io::{self, Write};
use std::time::Duration;

use serde_json::{Value, json};
use zeus_proto::{
    EventsSubscribeParams, EventsWaitParams, EventsWaitResult, HelloParams, Method,
    ReadScreenResult, ReadScrollbackResult, SendTextParams, SessionIdParams, SessionRecord,
    SessionSpawnParams, WorktreeCreateParams, WorktreeInfo, WorktreeListParams,
    WorktreeRemoveParams,
};

use crate::args::{Command, Output};
use crate::catalog::{self, parse_kind};
use crate::conn::{self, DaemonConn};
use crate::error::CliError;
use crate::mcp;
use crate::render;
use crate::support::{
    encode_compact, parse_payload, read_stdin, read_stdin_to_string, resolve_session, session_id,
    which,
};
use zeus_proto::paths::ZeusPaths;

impl Command {
    pub fn run(self) -> Result<i32, CliError> {
        match self {
            Self::Help(text) => {
                print!("{text}");
                Ok(0)
            }
            Self::SessionList {
                output,
                status,
                all,
            } => session_list(output, status, all),
            Self::SessionGet { output, session } => session_get(output, &session),
            Self::SessionRead {
                output,
                session,
                source,
                lines,
            } => session_read(output, &session, &source, lines),
            Self::SessionSend {
                output,
                session,
                text,
                no_submit,
            } => session_send(output, &session, text, no_submit),
            Self::SessionWait {
                output,
                session,
                until,
                timeout,
            } => session_wait(output, &session, until, timeout),
            spawn @ Self::SessionSpawn { .. } => session_spawn(spawn),
            Self::SessionRelease {
                output,
                session,
                remove,
            } => session_release(output, &session, remove),
            Self::SessionArchive {
                output,
                session,
                undo,
            } => session_archive(output, &session, undo),
            Self::Artifacts { output, session } => artifacts(output, &session),
            Self::WorktreeList { output, repo } => worktree_list(output, repo),
            Self::WorktreeCreate {
                output,
                repo,
                branch,
                base,
            } => worktree_create(output, repo, branch, base),
            Self::WorktreeRemove {
                output,
                repo,
                path,
                force,
            } => worktree_remove(output, &repo, &path, force),
            Self::EventsSubscribe {
                output,
                session,
                kind,
                since_seq,
                count,
            } => events_subscribe(output, session, kind, since_seq, count),
            Self::EventsWait {
                output,
                session,
                until,
                kind,
                timeout,
            } => events_wait(output, session, until, kind, timeout),
            Self::Ports { output } => ports(output),
            Self::Forward => Err(CliError::Failure(
                "port forwarding is not supported by this engine".into(),
            )),
            Self::Hook { event } => hook(&event),
            Self::Notify { args } => notify(args),
            Self::McpStdio => mcp::exec_stdio(),
            Self::McpTools => {
                mcp::print_tools();
                Ok(0)
            }
            Self::McpCall { tool } => {
                mcp::print_call(&tool);
                Ok(0)
            }
            Self::Doctor => doctor(),
        }
    }
}

fn emit(output: &Output, value: Value) {
    if output.json {
        println!("{}", encode_compact(&value));
    }
}

fn session_list(output: Output, status: Option<String>, all: bool) -> Result<i32, CliError> {
    let mut sessions = conn::sessions()?.sessions;
    sessions.sort_by(|left, right| left.created_at.0.total_cmp(&right.created_at.0));
    if !all {
        sessions.retain(|session| !session.is_archived());
    }
    if let Some(prefix) = status {
        sessions.retain(|session| render::status_label(&session.status).starts_with(&prefix));
    }
    if output.json {
        emit(
            &output,
            json!({"sessions": sessions.iter().filter_map(|session| serde_json::to_value(session).ok()).collect::<Vec<_>>()}),
        );
    } else {
        render::print_session_table(&sessions);
    }
    Ok(0)
}

fn session_get(output: Output, needle: &str) -> Result<i32, CliError> {
    let record = conn::resolve(needle)?;
    if output.json {
        println!("{}", encode_compact(&serde_json::to_value(&record)?));
    } else {
        render::print_session_detail(&record);
    }
    Ok(0)
}

fn session_read(
    output: Output,
    needle: &str,
    source: &str,
    lines: Option<i64>,
) -> Result<i32, CliError> {
    let record = conn::resolve(needle)?;
    let params = SessionIdParams {
        session_id: record.id.clone(),
    };
    let (mut text, cols, rows) = match source {
        "screen" => {
            let screen: ReadScreenResult = conn::with_conn(Duration::from_secs(3), |conn| {
                conn.request(Method::SESSION_READ_SCREEN, &params, Duration::from_secs(3))
            })?;
            (
                screen
                    .text
                    .split('\n')
                    .map(str::to_string)
                    .collect::<Vec<_>>(),
                screen.cols,
                screen.rows,
            )
        }
        _ => {
            let scrollback: ReadScrollbackResult =
                conn::with_conn(Duration::from_secs(3), |conn| {
                    conn.request(
                        Method::SESSION_READ_SCROLLBACK,
                        &params,
                        Duration::from_secs(3),
                    )
                })?;
            (scrollback.lines, scrollback.cols, scrollback.rows)
        }
    };
    if let Some(limit) = lines
        && limit > 0
        && text.len() > limit as usize
    {
        text = text.split_off(text.len() - limit as usize);
    }
    if output.json {
        println!(
            "{}",
            encode_compact(&json!({
                "id": record.id.0,
                "source": source,
                "cols": cols,
                "rows": rows,
                "lines": text,
            }))
        );
    } else {
        for line in text {
            println!("{line}");
        }
    }
    Ok(0)
}

fn session_send(
    output: Output,
    needle: &str,
    text: Vec<String>,
    no_submit: bool,
) -> Result<i32, CliError> {
    let record = conn::resolve(needle)?;
    let mut body = text.join(" ");
    if body.is_empty() {
        body = read_stdin_to_string();
    }
    if body.is_empty() {
        return Err(CliError::Usage("nothing to send".into()));
    }
    conn::with_conn(Duration::from_secs(3), |conn| {
        conn.request_value(
            Method::SESSION_SEND_TEXT,
            &SendTextParams {
                session_id: record.id.clone(),
                text: body.clone(),
                submit: !no_submit,
            },
            Duration::from_secs(3),
        )
    })?;
    if output.json {
        println!(
            "{}",
            encode_compact(&json!({"ok": true, "id": record.id.0}))
        );
    } else {
        println!("sent {} chars to {}", body.chars().count(), record.id.0);
    }
    Ok(0)
}

fn session_wait(
    output: Output,
    needle: &str,
    until: Vec<String>,
    timeout: f64,
) -> Result<i32, CliError> {
    let record = conn::resolve(needle)?;
    let params = EventsWaitParams {
        session_id: record.id.clone(),
        until,
        timeout_ms: (timeout * 1000.0) as i64,
    };
    let budget = Duration::from_secs_f64(timeout + 5.0);
    let result: EventsWaitResult = conn::with_conn(budget, |conn| {
        conn.request(Method::EVENTS_WAIT, &params, budget)
    })?;
    if output.json {
        println!("{}", encode_compact(&serde_json::to_value(&result)?));
    } else {
        println!(
            "{}  {}  {}",
            result.session.id.0,
            render::status_label(&result.session.status),
            result.session.title
        );
    }
    if result.timed_out {
        Err(CliError::Timeout)
    } else {
        Ok(0)
    }
}

fn session_spawn(command: Command) -> Result<i32, CliError> {
    let Command::SessionSpawn {
        output,
        kind,
        cwd,
        title,
        prompt,
        worktree,
        branch,
        host,
    } = command
    else {
        return Err(CliError::Failure("internal spawn dispatch".into()));
    };
    let params = SessionSpawnParams {
        kind: parse_kind(&kind),
        cwd: cwd.unwrap_or_else(|| crate::support::cwd().display().to_string()),
        new_worktree: worktree.then_some(true),
        worktree_branch: branch,
        title,
        initial_prompt: prompt,
        parent: session_id(),
        initial_cols: None,
        initial_rows: None,
        host,
        same_repo_as: None,
    };
    let record: SessionRecord = conn::with_conn(Duration::from_secs(60), |conn| {
        conn.request(Method::SESSION_SPAWN, &params, Duration::from_secs(60))
    })?;
    if output.json {
        println!("{}", encode_compact(&serde_json::to_value(&record)?));
    } else {
        println!(
            "{}  {}  {}",
            record.id.0,
            catalog::short_label(&record.kind),
            record.title
        );
    }
    Ok(0)
}

fn session_release(output: Output, needle: &str, remove: bool) -> Result<i32, CliError> {
    let record = conn::resolve(needle)?;
    let method = if remove {
        Method::SESSION_REMOVE
    } else {
        Method::SESSION_KILL
    };
    conn::with_conn(Duration::from_secs(3), |conn| {
        conn.request_value(
            method,
            &SessionIdParams {
                session_id: record.id.clone(),
            },
            Duration::from_secs(3),
        )
    })?;
    if output.json {
        println!(
            "{}",
            encode_compact(&json!({"ok": true, "id": record.id.0}))
        );
    } else {
        println!("released {}", record.id.0);
    }
    Ok(0)
}

fn session_archive(output: Output, needle: &str, undo: bool) -> Result<i32, CliError> {
    let record = conn::resolve(needle)?;
    let method = if undo {
        Method::SESSION_UNARCHIVE
    } else {
        Method::SESSION_ARCHIVE
    };
    conn::with_conn(Duration::from_secs(3), |conn| {
        conn.request_value(
            method,
            &SessionIdParams {
                session_id: record.id.clone(),
            },
            Duration::from_secs(3),
        )
    })?;
    if output.json {
        println!(
            "{}",
            encode_compact(&json!({"ok": true, "id": record.id.0}))
        );
    } else {
        println!(
            "{} {}",
            if undo { "unarchived" } else { "archived" },
            record.id.0
        );
    }
    Ok(0)
}

fn artifacts(output: Output, needle: &str) -> Result<i32, CliError> {
    let record = conn::resolve(needle)?;
    let artifacts = record.artifacts.clone().unwrap_or_default();
    let ports = record.listening_ports.clone().unwrap_or_default();
    if output.json {
        println!(
            "{}",
            encode_compact(&json!({
                "id": record.id.0,
                "artifacts": artifacts,
                "listeningPorts": ports,
                "pullRequests": record.pull_requests.clone().unwrap_or_default(),
            }))
        );
        return Ok(0);
    }
    if artifacts.is_empty() && ports.is_empty() {
        println!("No artifacts for {}.", record.id.0);
        return Ok(0);
    }
    let mut pr_by_url = std::collections::BTreeMap::new();
    for pr in record.pull_requests.iter().flatten() {
        pr_by_url.entry(pr.url.clone()).or_insert(pr);
    }
    for artifact in &artifacts {
        if let Some(pr) = pr_by_url.get(&artifact.url) {
            println!(
                "{}  {}  [{}]",
                render::pad_column(render::artifact_kind(&artifact.kind), 12),
                artifact.url,
                render::pr_overall(pr)
            );
        } else {
            println!(
                "{}  {}",
                render::pad_column(render::artifact_kind(&artifact.kind), 12),
                artifact.url
            );
        }
    }
    for port in &ports {
        println!(
            "{}  localhost:{}  ({})",
            render::pad_column("port", 12),
            port.port,
            port.process_name
        );
    }
    Ok(0)
}

fn worktree_list(output: Output, repo: Option<String>) -> Result<i32, CliError> {
    let repo_path = repo.unwrap_or_else(|| crate::support::cwd().display().to_string());
    let worktrees: Vec<WorktreeInfo> = conn::with_conn(Duration::from_secs(3), |conn| {
        conn.request(
            Method::WORKTREE_LIST,
            &WorktreeListParams {
                repo_path: repo_path.clone(),
            },
            Duration::from_secs(3),
        )
    })?;
    if output.json {
        println!(
            "{}",
            encode_compact(&json!({"repo": repo_path, "worktrees": worktrees}))
        );
        return Ok(0);
    }
    if worktrees.is_empty() {
        println!("No worktrees for {repo_path}.");
        return Ok(0);
    }
    let branch_width = worktrees
        .iter()
        .map(|item| item.branch.as_deref().unwrap_or("-").len())
        .max()
        .unwrap_or(6)
        .max(6);
    for worktree in worktrees {
        let mut flags = Vec::new();
        if worktree.is_bare {
            flags.push("bare");
        }
        if worktree.is_detached {
            flags.push("detached");
        }
        if worktree.is_prunable {
            flags.push("prunable");
        }
        let suffix = if flags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", flags.join(","))
        };
        println!(
            "{}  {}{suffix}",
            render::pad_column(worktree.branch.as_deref().unwrap_or("-"), branch_width),
            worktree.path
        );
    }
    Ok(0)
}

fn worktree_create(
    output: Output,
    repo: Option<String>,
    branch: Option<String>,
    base: Option<String>,
) -> Result<i32, CliError> {
    let repo_path = repo.unwrap_or_else(|| crate::support::cwd().display().to_string());
    let info: WorktreeInfo = conn::with_conn(Duration::from_secs(120), |conn| {
        conn.request(
            Method::WORKTREE_CREATE,
            &WorktreeCreateParams {
                repo_path,
                branch,
                base,
            },
            Duration::from_secs(120),
        )
    })?;
    if output.json {
        println!("{}", encode_compact(&serde_json::to_value(&info)?));
    } else {
        println!("{}  {}", info.branch.as_deref().unwrap_or("-"), info.path);
    }
    Ok(0)
}

fn worktree_remove(output: Output, repo: &str, path: &str, force: bool) -> Result<i32, CliError> {
    conn::with_conn(Duration::from_secs(60), |conn| {
        conn.request_value(
            Method::WORKTREE_REMOVE,
            &WorktreeRemoveParams {
                repo_path: repo.to_string(),
                worktree_path: path.to_string(),
                force,
            },
            Duration::from_secs(60),
        )
    })?;
    if output.json {
        println!("{}", encode_compact(&json!({"ok": true, "path": path})));
    } else {
        println!("removed {path}");
    }
    Ok(0)
}

fn events_subscribe(
    output: Output,
    session: Vec<String>,
    kind: Vec<String>,
    since_seq: Option<u64>,
    count: Option<i64>,
) -> Result<i32, CliError> {
    let mut sessions = Vec::new();
    if !session.is_empty() {
        let known = conn::sessions()?.sessions;
        for needle in session {
            sessions.push(resolve_session(&needle, &known)?.id.clone());
        }
    }
    let params = EventsSubscribeParams {
        since_seq,
        sessions: (!sessions.is_empty()).then_some(sessions),
        kinds: (!kind.is_empty()).then_some(kind),
    };
    let mut conn = DaemonConn::connect()?;
    let mut seen = 0;
    conn.stream(
        Method::EVENTS_SUBSCRIBE,
        &params,
        None,
        |name, seq, params| {
            if output.json {
                println!(
                    "{}",
                    encode_compact(&json!({"event": name, "seq": seq, "params": params}))
                );
            } else {
                println!("{}", render::event_line(name, seq, params));
            }
            let _ = io::stdout().flush();
            seen += 1;
            Ok(count.is_none_or(|limit| seen < limit))
        },
    )?;
    Ok(0)
}

fn events_wait(
    output: Output,
    session: Option<String>,
    until: Vec<String>,
    kind: Vec<String>,
    timeout: f64,
) -> Result<i32, CliError> {
    if !until.is_empty() {
        let needle = session.ok_or_else(|| {
            CliError::Usage(
                "--until needs --session (a status is a property of one session)".into(),
            )
        })?;
        return session_wait(output, &needle, until, timeout);
    }
    let mut sessions = None;
    if let Some(needle) = session {
        sessions = Some(vec![conn::resolve(&needle)?.id]);
    }
    let params = EventsSubscribeParams {
        since_seq: None,
        sessions,
        kinds: (!kind.is_empty()).then_some(kind),
    };
    let mut conn = DaemonConn::connect()?;
    let mut matched = None;
    let result = conn.stream(
        Method::EVENTS_SUBSCRIBE,
        &params,
        Some(Duration::from_secs_f64(timeout)),
        |name, seq, params| {
            matched = Some((name.to_string(), seq, params.clone()));
            Ok(false)
        },
    );
    match (result, matched) {
        (_, Some((name, seq, params))) => {
            if output.json {
                println!(
                    "{}",
                    encode_compact(&json!({"event": name, "seq": seq, "params": params}))
                );
            } else {
                println!("{}", render::event_line(&name, seq, &params));
            }
            Ok(0)
        }
        (Err(CliError::Timeout), None) => {
            if output.json {
                println!("{}", encode_compact(&json!({"timedOut": true})));
            } else {
                println!("timed out");
            }
            Err(CliError::Timeout)
        }
        (Err(error), None) => Err(error),
        (Ok(()), None) => {
            println!("timed out");
            Err(CliError::Timeout)
        }
    }
}

fn ports(output: Output) -> Result<i32, CliError> {
    let sessions = conn::sessions()?.sessions;
    let mut rows = Vec::new();
    for session in &sessions {
        for port in session.listening_ports.iter().flatten() {
            rows.push(json!({
                "session": session.id.0,
                "title": session.title,
                "port": port.port,
                "process": port.process_name,
            }));
        }
    }
    if output.json {
        println!("{}", encode_compact(&json!({"ports": rows})));
        return Ok(0);
    }
    if rows.is_empty() {
        println!("No listening ports.");
        return Ok(0);
    }
    for row in rows {
        println!(
            "{}  localhost:{}  {}  ({})",
            row["session"].as_str().unwrap_or("?"),
            row["port"],
            row["process"].as_str().unwrap_or("?"),
            row["title"].as_str().unwrap_or("")
        );
    }
    Ok(0)
}

fn hook(event: &str) -> Result<i32, CliError> {
    let output = perform_hook(event).unwrap_or_else(|_| "{}".into());
    println!("{output}");
    Ok(0)
}

fn perform_hook(event: &str) -> Result<String, CliError> {
    let payload = parse_payload(&read_stdin(1 << 20, Duration::from_millis(500)));
    let params = zeus_proto::HookReportParams {
        kind: "claude-hook".into(),
        zeus_session_id: session_id(),
        event: Some(event.to_string()),
        payload,
    };
    let response = conn::with_conn(Duration::from_secs(3), |conn| {
        conn.request_value(Method::HOOK_REPORT, &params, Duration::from_secs(3))
    })?;
    if event == "SessionStart"
        && let Some(title) = response.get("sessionTitle").and_then(Value::as_str)
    {
        return Ok(encode_compact(&json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "sessionTitle": title,
            }
        })));
    }
    Ok("{}".into())
}

fn notify(args: Vec<String>) -> Result<i32, CliError> {
    let _ = perform_notify(args);
    Ok(0)
}

fn perform_notify(args: Vec<String>) -> Result<(), CliError> {
    let Some(json_string) = args.last() else {
        return Ok(());
    };
    let params = zeus_proto::HookReportParams {
        kind: "codex-notify".into(),
        zeus_session_id: session_id(),
        event: None,
        payload: parse_payload(json_string.as_bytes()),
    };
    conn::with_conn(Duration::from_secs(3), |conn| {
        conn.request_value(Method::HOOK_REPORT, &params, Duration::from_secs(3))
    })?;
    Ok(())
}

fn doctor() -> Result<i32, CliError> {
    let mut daemon_ok = false;
    let socket = DaemonConn::socket_path();
    if socket.exists() {
        match DaemonConn::connect() {
            Ok(mut conn) => {
                match conn.request::<_, zeus_proto::HelloResult>(
                    Method::HELLO,
                    &HelloParams::new(format!("zeus-cli/{}", env!("CARGO_PKG_VERSION"))),
                    Duration::from_secs(3),
                ) {
                    Ok(hello) => {
                        println!(
                            "✓ daemon reachable (build {}, pid {}, proto {})",
                            hello.build, hello.pid, hello.proto
                        );
                        daemon_ok = true;
                    }
                    Err(error) => println!("✗ daemon unreachable ({error})"),
                }
            }
            Err(error) => println!("✗ daemon unreachable ({error})"),
        }
    } else {
        println!("✗ daemon socket missing at {}", socket.display());
    }
    for binary in ["claude", "codex"] {
        match which(binary) {
            Some(path) => println!("✓ {binary} found at {path}"),
            None => println!("✗ {binary} not found on PATH"),
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let state = ZeusPaths::state_file(home);
    if state.exists() {
        println!("✓ state file present at {}", state.display());
    } else {
        println!("✗ state file missing at {}", state.display());
    }
    if daemon_ok {
        Ok(0)
    } else {
        Err(CliError::Failure("daemon unreachable".into()))
    }
}
