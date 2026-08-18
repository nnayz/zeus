use std::env;
use std::io::{self, BufRead, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process;
use std::time::Duration;

use serde_json::{Map, Value, json};
use zeus_proto::{
    EventsSubscribeParams, EventsWaitParams, Method, SendTextParams, SessionId, SessionIdParams,
    SessionRecord, SessionSpawnParams, TestRunParams, WorktreeCreateParams, WorktreeListParams,
    WorktreeRemoveParams,
};

use crate::catalog::{self, AgentCatalog};
use crate::conn::{self, DaemonConn};
use crate::error::CliError;
use crate::lineage::{self, ChildReport, Relation, SessionLineage};
use crate::support::{encode_compact, parse_payload, read_stdin, session_id};

pub fn tools_json() -> Value {
    json!({ "tools": tool_definitions() })
}

pub fn call(tool: &str, args: &Value) -> Result<Value, CliError> {
    match tool {
        "spawn_agent" => spawn_agent(args),
        "list_agents" => list_agents(),
        "get_status" => get_status(args),
        "send_prompt" => send_prompt(args),
        "wait_for_agent" => wait_for_agent(args),
        "read_output" => read_output(args),
        "get_artifacts" => get_artifacts(args),
        "create_worktree" => create_worktree(args),
        "list_worktrees" => list_worktrees(args),
        "remove_worktree" => remove_worktree(args),
        "release_agent" => release_agent(args),
        "test_run" => test_run(args),
        "whoami" => whoami(),
        "list_children" => list_children(args),
        "wait_for_children" => wait_for_children(args),
        "summarize_children" => summarize_children(args),
        "report_to_parent" => report_to_parent(args),
        "browser" => browser(args),
        other => Err(CliError::Failure(format!("unknown tool: {other}"))),
    }
}

pub fn exec_stdio() -> Result<i32, CliError> {
    if let Some(proxy) = standalone_proxy_path() {
        let error = process::Command::new(&proxy).arg0(&proxy).exec();
        eprintln!("zeus: exec {} failed: {error}", proxy.display());
    }
    run_stdio()
}

pub fn standalone_proxy_path() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let proxy = exe.parent()?.join("zeus-mcp");
    crate::support::is_executable(&proxy).then_some(proxy)
}

fn run_stdio() -> Result<i32, CliError> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle_message(message),
            Err(_) => Some(
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}),
            ),
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut stdout, &response)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Ok(0)
}

fn handle_message(message: Value) -> Option<Value> {
    let object = message.as_object()?;
    let method = object.get("method")?.as_str()?;
    let id = object.get("id").cloned();
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "initialize" => id.map(|id| success(id, initialize(&params))),
        "ping" => id.map(|id| success(id, json!({}))),
        "tools/list" => id.map(|id| success(id, tools_json())),
        "tools/call" => id.map(|id| {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return success(id, tool_content(Err("tools/call missing 'name'".into())));
            };
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            success(
                id,
                tool_content(call(name, &arguments).map_err(|error| error.to_string())),
            )
        }),
        _ if id.is_none() => None,
        _ => Some(json!({
            "jsonrpc":"2.0",
            "id": id.unwrap_or(Value::Null),
            "error":{"code":-32601,"message":format!("Method not found: {method}")}
        })),
    }
}

fn initialize(params: &Value) -> Value {
    let version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("2025-06-18");
    let browser = if env::var_os("ZEUS_TEST_RUN_AVAILABLE").is_some() {
        " To test a web feature, use test_run with a preview URL from get_artifacts."
    } else {
        ""
    };
    json!({
        "protocolVersion": version,
        "capabilities": {"tools":{}},
        "serverInfo": {"name":"zeus","version":env!("CARGO_PKG_VERSION")},
        "instructions": format!(
            "This session is running INSIDE Zeus, a macOS orchestrator for coding agents. \
             These tools control it. Use them proactively whenever the user asks to \
             open/start/spawn/close another agent, session, tab, or terminal (Claude Code, \
             Codex, Cursor, Gemini, or a shell), to check what other sessions are doing, to \
             talk to another session, or to parallelize work across git worktrees — no \
             extra confirmation of intent needed.\n\nTypical orchestration flow: spawn_agent \
             (optionally worktree:true and an initial prompt) → wait_for_agent(until:\"done\") \
             → read_output → send_prompt for follow-ups → release_agent when finished. \
             get_artifacts returns PR/Linear/preview URLs and listening ports a session has \
             produced; PR entries include live GitHub status (state, review decision, checks, \
             comment counts, +/- lines).{browser}"
        )
    })
}

fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

fn tool_content(result: Result<Value, String>) -> Value {
    let (value, is_error) = match result {
        Ok(value) => (value, false),
        Err(message) => (Value::String(message), true),
    };
    let text = value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string(&value).unwrap_or_else(|_| "null".into()));
    json!({"content":[{"type":"text","text":text}],"isError":is_error})
}

fn tool_definitions() -> Vec<Value> {
    let kinds = catalog::spawnable_kind_labels();
    let names = catalog::spawnable_display_names();
    let mut tools = vec![
        tool(
            "spawn_agent",
            &format!(
                "Open a NEW session (tab) in Zeus running {names} — locally or on a configured remote host. USE THIS whenever the user asks to open/start/spawn/launch another agent, session, or terminal."
            ),
            json!({
                "type":"object",
                "properties":{
                    "kind":{"type":"string","enum":kinds,"description":"Which agent to run."},
                    "cwd":{"type":"string","description":"Working directory."},
                    "host":{"type":"string","description":"Host id from hosts.json."},
                    "worktree":{"type":"boolean","description":"Create a fresh git worktree off cwd and run there. Local only."},
                    "prompt":{"type":"string","description":"Initial prompt to send once the agent is ready."},
                    "name":{"type":"string","description":"Session title."}
                },
                "required":["kind","cwd"]
            }),
        ),
        tool(
            "list_agents",
            "List all active agent sessions with id, kind, title, status, parent, and cwd.",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "get_status",
            "Current status of one session.",
            json!({"type":"object","properties":{"session_id":{"type":"string"}},"required":["session_id"]}),
        ),
        tool(
            "send_prompt",
            "Type into another session's terminal and press Enter.",
            json!({"type":"object","properties":{"session_id":{"type":"string"},"text":{"type":"string"},"submit":{"type":"boolean"}},"required":["session_id","text"]}),
        ),
        tool(
            "wait_for_agent",
            "Block until an agent session reaches a target condition.",
            json!({"type":"object","properties":{"session_id":{"type":"string"},"until":{"type":"string","enum":["done","needs_me","idle","exited"]},"timeout_s":{"type":"number","default":600}},"required":["session_id"]}),
        ),
        tool(
            "read_output",
            "Read an agent session's output.",
            json!({"type":"object","properties":{"session_id":{"type":"string"},"mode":{"type":"string","enum":["screen","tail"]},"lines":{"type":"number","default":50}},"required":["session_id"]}),
        ),
        tool(
            "create_worktree",
            "Create a new git worktree in a repository.",
            json!({"type":"object","properties":{"repo":{"type":"string"},"branch":{"type":"string"}},"required":["repo"]}),
        ),
        tool(
            "list_worktrees",
            "List the git worktrees of a repository.",
            json!({"type":"object","properties":{"repo":{"type":"string"}},"required":["repo"]}),
        ),
        tool(
            "remove_worktree",
            "Remove a git worktree from a repository.",
            json!({"type":"object","properties":{"repo":{"type":"string"},"path":{"type":"string"},"force":{"type":"boolean"}},"required":["repo","path"]}),
        ),
        tool(
            "get_artifacts",
            "PR links, Linear issues, preview URLs and ports captured from the session's output.",
            json!({"type":"object","properties":{"session_id":{"type":"string"}},"required":["session_id"]}),
        ),
        tool(
            "release_agent",
            "Terminate an agent session.",
            json!({"type":"object","properties":{"session_id":{"type":"string"}},"required":["session_id"]}),
        ),
        tool(
            "browser",
            "Drive a real browser isolated to THIS session.",
            json!({
                "type":"object",
                "properties":{
                    "action":{"type":"string","enum":["open","snapshot","click","fill","type","press","hover","select","check","scroll","get","wait","screenshot","console","back","close","list"]},
                    "url":{"type":"string"},
                    "ref":{"type":"string"},
                    "selector":{"type":"string"},
                    "text":{"type":"string"},
                    "key":{"type":"string"},
                    "value":{"type":"string"},
                    "what":{"type":"string","enum":["url","title","text","html","value","count"]},
                    "ms":{"type":"number"},
                    "state":{"type":"string"},
                    "direction":{"type":"string","enum":["up","down","left","right"]},
                    "amount":{"type":"number"},
                    "button":{"type":"string","enum":["left","right","middle"]},
                    "double":{"type":"boolean"},
                    "full":{"type":"boolean"},
                    "annotate":{"type":"boolean"},
                    "engine":{"type":"string","enum":["chromium","webkit","firefox"]},
                    "profile":{"type":"string"}
                },
                "required":["action"]
            }),
        ),
        tool(
            "whoami",
            "Identity and lineage of THIS session.",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "list_children",
            "List the sessions YOU spawned.",
            json!({"type":"object","properties":{"recursive":{"type":"boolean"},"include_exited":{"type":"boolean"}}}),
        ),
        tool(
            "wait_for_children",
            "Block until the sessions you spawned settle.",
            json!({"type":"object","properties":{"session_ids":{"type":"array","items":{"type":"string"}},"until":{"type":"string","enum":["settled","done","exited"]},"timeout_s":{"type":"number","default":600}}}),
        ),
        tool(
            "summarize_children",
            "Compact screen tails plus status and artifacts for the sessions you spawned.",
            json!({"type":"object","properties":{"session_ids":{"type":"array","items":{"type":"string"}},"rows":{"type":"number","default":14}}}),
        ),
        tool(
            "report_to_parent",
            "Hand a structured result back to the session that spawned you.",
            json!({"type":"object","properties":{"summary":{"type":"string"},"status":{"type":"string","enum":["update","done","blocked","failed"]},"details":{"type":"string"},"blockers":{"type":"array","items":{"type":"string"}},"questions":{"type":"array","items":{"type":"string"}},"next_steps":{"type":"array","items":{"type":"string"}},"changed_paths":{"type":"array","items":{"type":"string"}},"artifacts":{"type":"array","items":{"type":"string"}},"proof":{"type":"array","items":{"type":"string"}},"submit":{"type":"boolean"}},"required":["summary"]}),
        ),
        tool(
            "test_run",
            "Run a web feature test flow across real browser engines.",
            json!({"type":"object","properties":{"url":{"type":"string"},"engines":{"type":"array","items":{"type":"string","enum":["chromium","webkit","firefox"]}},"steps":{"type":"array","items":{"type":"object"}},"observe":{"type":"string","enum":["a11y","screenshot"]},"profile":{"type":"string"},"auth":{"type":"object"}},"required":["url","steps"]}),
        ),
    ];
    if env::var("ZEUS_TEST_RUN_AVAILABLE").ok().as_deref() != Some("1") {
        tools.retain(|tool| tool["name"] != "test_run");
    }
    let _ = AgentCatalog::shared();
    tools
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({"name":name,"description":description,"inputSchema":schema})
}

fn require_string(args: &Value, key: &str) -> Result<String, CliError> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CliError::Failure(format!("missing required argument: {key}")))
}

fn opt_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}

fn opt_number(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(Value::as_f64)
}

fn opt_strings(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn fetch_sessions() -> Result<Vec<SessionRecord>, CliError> {
    Ok(conn::sessions()?.sessions)
}

fn spawn_agent(args: &Value) -> Result<Value, CliError> {
    let kind_str = require_string(args, "kind")?;
    let descriptor = AgentCatalog::shared().resolve(&kind_str).ok_or_else(|| {
        let known = AgentCatalog::shared()
            .ordered
            .iter()
            .map(|item| item.short_label.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        CliError::Failure(format!("invalid kind: {kind_str} (known: {known})"))
    })?;
    let params = SessionSpawnParams {
        kind: zeus_proto::AgentKind::new(descriptor.id.clone()),
        cwd: require_string(args, "cwd")?,
        new_worktree: opt_bool(args, "worktree"),
        worktree_branch: None,
        title: args.get("name").and_then(Value::as_str).map(str::to_string),
        initial_prompt: args
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::to_string),
        parent: session_id(),
        initial_cols: None,
        initial_rows: None,
        host: args.get("host").and_then(Value::as_str).map(str::to_string),
        same_repo_as: None,
    };
    conn::with_conn(Duration::from_secs(60), |conn| {
        conn.request_value(Method::SESSION_SPAWN, &params, Duration::from_secs(60))
    })
}

fn list_agents() -> Result<Value, CliError> {
    let sessions = fetch_sessions()?;
    Ok(json!({"agents": sessions.iter().map(lineage::compact).collect::<Vec<_>>()}))
}

fn get_status(args: &Value) -> Result<Value, CliError> {
    let id = SessionId::new(require_string(args, "session_id")?);
    let sessions = fetch_sessions()?;
    let record = sessions
        .iter()
        .find(|record| record.id.0 == id.0)
        .ok_or_else(|| CliError::Failure(format!("no such session: {}", id.0)))?;
    Ok(lineage::compact(record))
}

fn send_prompt(args: &Value) -> Result<Value, CliError> {
    let id = SessionId::new(require_string(args, "session_id")?);
    let text = require_string(args, "text")?;
    let submit = opt_bool(args, "submit").unwrap_or(true);
    let lineage = SessionLineage::current(fetch_sessions()?);
    let relation = lineage.relation(&id);
    if relation == Relation::Caller {
        return Err(CliError::Failure(format!(
            "send_prompt cannot target the calling session ({}) — that types into your own terminal and would feed your output back to yourself. Just answer normally.",
            id.0
        )));
    }
    let delivered = lineage.frame(&text, relation);
    conn::with_conn(Duration::from_secs(3), |conn| {
        conn.request_value(
            Method::SESSION_SEND_TEXT,
            &SendTextParams {
                session_id: id,
                text: delivered.clone(),
                submit,
            },
            Duration::from_secs(3),
        )
    })?;
    Ok(json!({
        "ok": true,
        "relation": relation.as_str(),
        "attributed": delivered != text,
    }))
}

fn wait_for_agent(args: &Value) -> Result<Value, CliError> {
    let id = SessionId::new(require_string(args, "session_id")?);
    let until = vec![
        args.get("until")
            .and_then(Value::as_str)
            .unwrap_or("done")
            .to_string(),
    ];
    let timeout_s = opt_number(args, "timeout_s").unwrap_or(600.0);
    let params = EventsWaitParams {
        session_id: id,
        until,
        timeout_ms: (timeout_s * 1000.0) as i64,
    };
    let read_timeout = Duration::from_secs_f64(timeout_s + 5.0);
    conn::with_conn(read_timeout, |conn| {
        conn.request_value(Method::EVENTS_WAIT, &params, read_timeout)
    })
}

fn read_output(args: &Value) -> Result<Value, CliError> {
    let id = SessionId::new(require_string(args, "session_id")?);
    let mode = args.get("mode").and_then(Value::as_str).unwrap_or("screen");
    let mut result = conn::with_conn(Duration::from_secs(3), |conn| {
        conn.request_value(
            Method::SESSION_READ_SCREEN,
            &SessionIdParams { session_id: id },
            Duration::from_secs(3),
        )
    })?;
    if mode == "tail"
        && let Some(object) = result.as_object_mut()
    {
        object.insert(
            "note".into(),
            json!("tail mode returns the current screen in v1"),
        );
    }
    Ok(result)
}

fn get_artifacts(args: &Value) -> Result<Value, CliError> {
    let id = SessionId::new(require_string(args, "session_id")?);
    let sessions = fetch_sessions()?;
    let record = sessions
        .iter()
        .find(|record| record.id.0 == id.0)
        .ok_or_else(|| CliError::Failure(format!("no such session: {}", id.0)))?;
    let mut pr_by_url = Map::new();
    for status in record.pull_requests.iter().flatten() {
        pr_by_url
            .entry(status.url.clone())
            .or_insert_with(|| pr_json(status));
    }
    let artifacts: Vec<Value> = record
        .artifacts
        .iter()
        .flatten()
        .map(|artifact| {
            let mut obj = json!({
                "kind": crate::render::artifact_kind(&artifact.kind),
                "url": artifact.url,
            });
            if matches!(artifact.kind, zeus_proto::ArtifactKind::PullRequest)
                && let Some(pr) = pr_by_url.get(&artifact.url)
            {
                obj["pr"] = pr.clone();
            }
            obj
        })
        .collect();
    let ports: Vec<Value> = record
        .listening_ports
        .iter()
        .flatten()
        .map(|port| json!({"port": port.port, "process": port.process_name}))
        .collect();
    Ok(json!({"artifacts": artifacts, "listeningPorts": ports}))
}

fn pr_json(pr: &zeus_proto::PullRequestStatus) -> Value {
    let runs: Vec<Value> = pr
        .checks
        .iter()
        .flatten()
        .map(|check| {
            let mut run = json!({"name": check.name, "result": check.result});
            if let Some(detail) = &check.detail {
                run["detail"] = json!(detail);
            }
            run
        })
        .collect();
    let mut obj = json!({
        "number": pr.number,
        "state": pr.state,
        "overall": crate::render::pr_overall(pr),
        "draft": pr.is_draft,
        "additions": pr.additions,
        "deletions": pr.deletions,
        "changed_files": pr.changed_files,
        "comments": pr.comment_count,
        "reviews": pr.review_count,
        "checks": {
            "passed": pr.checks_passed,
            "failed": pr.checks_failed,
            "pending": pr.checks_pending,
            "runs": runs
        },
        "fetched_at": crate::render::iso8601(pr.fetched_at.0),
    });
    if let Some(total) = pr.total_threads {
        obj["review_threads"] = json!({
            "resolved": pr.resolved_threads.unwrap_or(0),
            "total": total,
        });
    }
    if let Some(title) = &pr.title {
        obj["title"] = json!(title);
    }
    if let Some(decision) = &pr.review_decision {
        obj["review_decision"] = json!(decision);
    }
    if let Some(mergeable) = &pr.mergeable {
        obj["mergeable"] = json!(mergeable);
    }
    if let Some(merge_state) = &pr.merge_state_status {
        obj["merge_state"] = json!(merge_state);
    }
    obj
}

fn create_worktree(args: &Value) -> Result<Value, CliError> {
    let params = WorktreeCreateParams {
        repo_path: require_string(args, "repo")?,
        branch: args
            .get("branch")
            .and_then(Value::as_str)
            .map(str::to_string),
        base: None,
    };
    conn::with_conn(Duration::from_secs(120), |conn| {
        conn.request_value(Method::WORKTREE_CREATE, &params, Duration::from_secs(120))
    })
}

fn list_worktrees(args: &Value) -> Result<Value, CliError> {
    let params = WorktreeListParams {
        repo_path: require_string(args, "repo")?,
    };
    conn::with_conn(Duration::from_secs(3), |conn| {
        conn.request_value(Method::WORKTREE_LIST, &params, Duration::from_secs(3))
    })
}

fn remove_worktree(args: &Value) -> Result<Value, CliError> {
    let params = WorktreeRemoveParams {
        repo_path: require_string(args, "repo")?,
        worktree_path: require_string(args, "path")?,
        force: opt_bool(args, "force").unwrap_or(false),
    };
    conn::with_conn(Duration::from_secs(60), |conn| {
        conn.request_value(Method::WORKTREE_REMOVE, &params, Duration::from_secs(60))
    })?;
    Ok(json!({"ok": true}))
}

fn release_agent(args: &Value) -> Result<Value, CliError> {
    let id = SessionId::new(require_string(args, "session_id")?);
    let lineage = SessionLineage::current(fetch_sessions()?);
    match lineage.relation(&id) {
        Relation::Caller => {
            return Err(CliError::Failure(format!(
                "release_agent cannot terminate the calling session ({}) — you would be killing the process running this tool.",
                id.0
            )));
        }
        Relation::Parent | Relation::Ancestor => {
            return Err(CliError::Failure(format!(
                "{} is the session that spawned you; releasing it would kill the conversation waiting on your result. Use report_to_parent to hand your work back instead.",
                id.0
            )));
        }
        _ => {}
    }
    conn::with_conn(Duration::from_secs(3), |conn| {
        conn.request_value(
            Method::SESSION_KILL,
            &SessionIdParams { session_id: id },
            Duration::from_secs(3),
        )
    })?;
    Ok(json!({"ok": true}))
}

fn browser(args: &Value) -> Result<Value, CliError> {
    let session_id = session_id().ok_or_else(|| {
        CliError::Failure(
            "The browser is scoped to a Zeus session and ZEUS_SESSION_ID is unset — run this from a session hosted by Zeus.".into(),
        )
    })?;
    let action = require_string(args, "action")?;
    let mut params = json!({
        "sessionID": session_id.0,
        "action": action,
    });
    for key in [
        "url",
        "ref",
        "selector",
        "text",
        "key",
        "value",
        "what",
        "state",
        "direction",
        "button",
        "engine",
        "profile",
    ] {
        if let Some(value) = args.get(key).and_then(Value::as_str) {
            params[key] = json!(value);
        }
    }
    for key in ["ms", "amount"] {
        if let Some(value) = opt_number(args, key) {
            params[key] = json!(value);
        }
    }
    for key in ["double", "full", "annotate"] {
        if let Some(value) = opt_bool(args, key) {
            params[key] = json!(value);
        }
    }
    conn::with_conn(Duration::from_secs(60), |conn| {
        conn.request_value("browser.act", &params, Duration::from_secs(60))
    })
}

fn test_run(args: &Value) -> Result<Value, CliError> {
    let url = require_string(args, "url")?;
    let steps = args
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| {
            CliError::Failure("missing required argument: steps (array of step objects)".into())
        })?;
    let engines = args.get("engines").and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    });
    let params = TestRunParams {
        url,
        engines,
        steps,
        observe: args
            .get("observe")
            .and_then(Value::as_str)
            .map(str::to_string),
        baseline: args
            .get("baseline")
            .and_then(Value::as_str)
            .map(str::to_string),
        profile: args
            .get("profile")
            .and_then(Value::as_str)
            .map(str::to_string),
        auth: args.get("auth").cloned(),
    };
    conn::with_conn(Duration::from_secs(180), |conn| {
        conn.request_value(Method::TEST_RUN, &params, Duration::from_secs(180))
    })
}

fn whoami() -> Result<Value, CliError> {
    let lineage = SessionLineage::current(fetch_sessions()?);
    let Some(caller) = lineage.caller() else {
        return Ok(json!({
            "hosted": false,
            "note": "Not running inside a Zeus session (ZEUS_SESSION_ID is unset). Lineage tools are unavailable and writes to other sessions are unrestricted and unattributed."
        }));
    };
    let record = lineage.record(caller).ok_or_else(|| {
        CliError::Failure(format!(
            "ZEUS_SESSION_ID is {} but the daemon has no such session — the record may have been removed.",
            caller.0
        ))
    })?;
    let mut obj = json!({
        "hosted": true,
        "session": lineage::detailed(record, Some(Relation::Caller)),
        "children": lineage.children(caller).into_iter().map(|child| lineage::detailed(child, Some(Relation::Child))).collect::<Vec<_>>(),
        "descendant_count": lineage.descendants(caller).len(),
        "write_policy": SessionLineage::write_policy(),
    });
    if let Some(parent_id) = &record.parent
        && let Some(parent) = lineage.record(parent_id)
    {
        obj["parent"] = lineage::detailed(parent, Some(Relation::Parent));
    }
    let ancestors = lineage.ancestors(caller);
    if !ancestors.is_empty() {
        obj["ancestors"] = json!(
            ancestors
                .into_iter()
                .map(|item| lineage::detailed(item, Some(Relation::Ancestor)))
                .collect::<Vec<_>>()
        );
    }
    Ok(obj)
}

fn list_children(args: &Value) -> Result<Value, CliError> {
    let lineage = SessionLineage::current(fetch_sessions()?);
    let caller = lineage.require_caller()?;
    let recursive = opt_bool(args, "recursive").unwrap_or(false);
    let include_exited = opt_bool(args, "include_exited").unwrap_or(true);
    let mut rows = if recursive {
        lineage.descendants(caller)
    } else {
        lineage.children(caller)
    };
    if !include_exited {
        rows.retain(|record| lineage::is_running(&record.status));
    }
    let items: Vec<Value> = rows
        .into_iter()
        .map(|record| {
            let relation = if record
                .parent
                .as_ref()
                .is_some_and(|parent| parent.0 == caller.0)
            {
                Relation::Child
            } else {
                Relation::Descendant
            };
            lineage::detailed(record, Some(relation))
        })
        .collect();
    let count = items.len();
    Ok(json!({"children": items, "count": count}))
}

fn resolve_child_subset<'a>(
    args: &Value,
    lineage: &'a SessionLineage,
    caller: &SessionId,
) -> Result<Vec<&'a SessionRecord>, CliError> {
    let all = lineage.children(caller);
    let requested = opt_strings(args, "session_ids");
    if requested.is_empty() {
        return Ok(all);
    }
    let mut out = Vec::new();
    for raw in requested {
        let match_child = all.iter().copied().find(|record| record.id.0 == raw);
        match match_child {
            Some(record) => out.push(record),
            None => {
                return Err(CliError::Failure(format!(
                    "{raw} is not one of your children — you can only coordinate sessions you spawned. Call list_children to see them."
                )));
            }
        }
    }
    Ok(out)
}

fn wait_for_children(args: &Value) -> Result<Value, CliError> {
    let initial = SessionLineage::current(fetch_sessions()?);
    let caller = initial.require_caller()?.clone();
    let targets = resolve_child_subset(args, &initial, &caller)?;
    if targets.is_empty() {
        return Ok(json!({
            "settled": true,
            "children": [],
            "note": "You have no child sessions to wait for."
        }));
    }
    let mode = args
        .get("until")
        .and_then(Value::as_str)
        .unwrap_or("settled")
        .to_string();
    let timeout_s = opt_number(args, "timeout_s").unwrap_or(600.0);
    let wanted: Vec<String> = targets.iter().map(|record| record.id.0.clone()).collect();
    let mut latest: Vec<SessionRecord> = targets.into_iter().cloned().collect();
    let mut all_settled = false;
    let mut reassess = || -> Result<bool, CliError> {
        latest = fetch_sessions()?
            .into_iter()
            .filter(|record| wanted.contains(&record.id.0))
            .collect();
        all_settled = latest.len() != wanted.len()
            || latest
                .iter()
                .all(|record| lineage::has_reached(&mode, &record.status));
        Ok(all_settled)
    };
    if !reassess()? {
        let mut conn = DaemonConn::connect()?;
        let subscribe = EventsSubscribeParams {
            since_seq: None,
            sessions: Some(wanted.iter().cloned().map(SessionId::new).collect()),
            kinds: Some(vec!["session.updated".into(), "session.removed".into()]),
        };
        let result = conn.stream(
            Method::EVENTS_SUBSCRIBE,
            &subscribe,
            Some(Duration::from_secs_f64(timeout_s)),
            |_, _, _| Ok(!reassess()?),
        );
        if !matches!(result, Ok(()) | Err(CliError::Timeout)) {
            result?;
        }
    }
    let mut out = json!({
        "settled": all_settled,
        "timed_out": !all_settled,
        "children": latest.iter().map(|record| lineage::detailed(record, Some(Relation::Child))).collect::<Vec<_>>(),
        "waited_for": mode,
    });
    let shells: Vec<_> = latest
        .iter()
        .filter(|record| {
            record.effective_kind().id() == zeus_proto::AgentKind::SHELL_ID
                && !lineage::has_reached(&mode, &record.status)
        })
        .map(|record| record.id.0.clone())
        .collect();
    if !shells.is_empty() {
        out["note"] = json!(format!(
            "These children are plain shells, which never report idle: {}. Wait on agent sessions, or use until:\"exited\" for shells.",
            shells.join(", ")
        ));
    }
    Ok(out)
}

fn summarize_children(args: &Value) -> Result<Value, CliError> {
    let lineage = SessionLineage::current(fetch_sessions()?);
    let caller = lineage.require_caller()?.clone();
    let targets = resolve_child_subset(args, &lineage, &caller)?;
    let rows = opt_number(args, "rows").unwrap_or(14.0).clamp(1.0, 60.0) as usize;
    let items: Vec<Value> = targets
        .into_iter()
        .map(|record| {
            let mut obj = lineage::detailed(record, Some(Relation::Child));
            match screen_tail(&record.id, rows) {
                Ok(tail) => obj["screen_tail"] = json!(tail),
                Err(_) => {
                    obj["screen_tail"] = Value::Null;
                    obj["screen_note"] = json!("no readable screen (session may have exited)");
                }
            }
            if let Some(artifacts) = &record.artifacts
                && !artifacts.is_empty()
            {
                obj["artifacts"] =
                    json!(artifacts.iter().map(|item| &item.url).collect::<Vec<_>>());
            }
            obj
        })
        .collect();
    let count = items.len();
    Ok(json!({"children": items, "count": count}))
}

fn screen_tail(id: &SessionId, rows: usize) -> Result<String, CliError> {
    let result: zeus_proto::ReadScreenResult = conn::with_conn(Duration::from_secs(3), |conn| {
        conn.request(
            Method::SESSION_READ_SCREEN,
            &SessionIdParams {
                session_id: id.clone(),
            },
            Duration::from_secs(3),
        )
    })?;
    let lines: Vec<_> = result
        .text
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let start = lines.len().saturating_sub(rows);
    Ok(lines[start..].join("\n"))
}

fn report_to_parent(args: &Value) -> Result<Value, CliError> {
    let lineage = SessionLineage::current(fetch_sessions()?);
    let caller = lineage.require_caller()?.clone();
    let record = lineage
        .record(&caller)
        .cloned()
        .ok_or_else(|| CliError::Failure(format!("no session record for {}", caller.0)))?;
    let parent_id = record.parent.clone().ok_or_else(|| {
        CliError::Failure(
            "This session has no parent — it was started by the user, not delegated by another agent, so there is nobody to report to. Answer in your own terminal instead.".into(),
        )
    })?;
    if lineage.record(&parent_id).is_none() {
        return Err(CliError::Failure(format!(
            "Your parent session ({}) is gone; the report has nowhere to land.",
            parent_id.0
        )));
    }
    let status = args
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("update")
        .to_string();
    if !ChildReport::STATUSES.contains(&status.as_str()) {
        return Err(CliError::Failure(format!(
            "invalid status: {status} — expected one of {}",
            ChildReport::STATUSES.join(", ")
        )));
    }
    let report = ChildReport {
        status: status.clone(),
        summary: require_string(args, "summary")?,
        details: args
            .get("details")
            .and_then(Value::as_str)
            .map(str::to_string),
        blockers: opt_strings(args, "blockers"),
        questions: opt_strings(args, "questions"),
        next_steps: opt_strings(args, "next_steps"),
        changed_paths: opt_strings(args, "changed_paths"),
        artifacts: opt_strings(args, "artifacts"),
        proof: opt_strings(args, "proof"),
    };
    let rendered = report.rendered(Some(&record), Some(&caller));
    let submit = opt_bool(args, "submit").unwrap_or(true);
    conn::with_conn(Duration::from_secs(3), |conn| {
        conn.request_value(
            Method::SESSION_SEND_TEXT,
            &SendTextParams {
                session_id: parent_id.clone(),
                text: rendered.clone(),
                submit,
            },
            Duration::from_secs(3),
        )
    })?;
    Ok(json!({
        "ok": true,
        "parent": parent_id.0,
        "status": status,
        "delivered": rendered,
    }))
}

pub fn print_tools() {
    println!("{}", encode_compact(&tools_json()));
}

pub fn print_call(tool: &str) {
    let input = read_stdin(4 << 20, Duration::from_secs(5));
    let arguments = parse_payload(&input);
    let result = match call(tool, &arguments) {
        Ok(value) => json!({"ok": value}),
        Err(error) => json!({"error": error.to_string()}),
    };
    println!("{}", encode_compact(&result));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_list_includes_lineage_and_artifacts() {
        let tools = tool_definitions();
        let names: Vec<_> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        for expected in [
            "spawn_agent",
            "get_artifacts",
            "browser",
            "whoami",
            "report_to_parent",
            "summarize_children",
        ] {
            assert!(
                names.contains(&expected),
                "{expected} missing from {names:?}"
            );
        }
        assert!(!names.contains(&"test_run"));
    }
}
