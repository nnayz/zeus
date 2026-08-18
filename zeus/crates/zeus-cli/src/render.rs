use serde_json::Value;
use zeus_proto::{
    ArtifactKind, ExitReason, NeedsInputKind, PullRequestStatus, Resumability, SessionRecord,
    SessionStatus,
};

use crate::catalog;
use crate::support::encode_compact;

pub const ALL_EVENT_KINDS: &[&str] = &[
    "session.updated",
    "session.resources",
    "session.removed",
    "project.updated",
    "session.spawned",
    "session.status",
    "session.needs_input",
    "session.output",
    "session.artifact",
    "session.archived",
    "worktree.created",
    "worktree.removed",
    "events.dropped",
];

pub fn pad_column(text: &str, width: usize) -> String {
    if text.chars().count() >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - text.chars().count()))
    }
}

pub fn status_label(status: &SessionStatus) -> String {
    match status {
        SessionStatus::Starting => "starting".into(),
        SessionStatus::Idle => "idle".into(),
        SessionStatus::Working => "working".into(),
        SessionStatus::NeedsInput(kind) => format!("needsInput:{}", needs_input_kind(kind)),
        SessionStatus::Exited(info) => format!("exited:{}", exit_reason(&info.reason)),
        SessionStatus::Unknown => "unknown".into(),
    }
}

fn needs_input_kind(kind: &NeedsInputKind) -> &'static str {
    match kind {
        NeedsInputKind::Permission => "permission",
        NeedsInputKind::Question => "question",
        NeedsInputKind::Unknown => "unknown",
    }
}

fn exit_reason(reason: &ExitReason) -> &'static str {
    match reason {
        ExitReason::Exited => "exited",
        ExitReason::Signaled => "signaled",
        ExitReason::DaemonRestart => "daemonRestart",
        ExitReason::External => "external",
        ExitReason::Archived => "archived",
        ExitReason::Unknown => "unknown",
    }
}

pub fn resumability_label(value: &Resumability) -> &'static str {
    match value {
        Resumability::Live => "live",
        Resumability::Resumable => "resumable",
        Resumability::TranscriptMissing => "transcriptMissing",
        Resumability::NotResumable => "notResumable",
        Resumability::Unknown => "unknown",
    }
}

pub fn print_session_table(sessions: &[SessionRecord]) {
    if sessions.is_empty() {
        println!("No active sessions.");
        return;
    }
    let id_width = sessions
        .iter()
        .map(|session| session.id.0.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let status_width = sessions
        .iter()
        .map(|session| status_label(&session.status).len())
        .max()
        .unwrap_or(6)
        .max(6);
    let header = format!(
        "{}  K  {}  TITLE",
        pad_column("ID", id_width),
        pad_column("STATUS", status_width)
    );
    println!("{header}");
    println!("{}", "─".repeat(header.chars().count()));
    for session in sessions {
        println!(
            "{}  {}  {}  {}",
            pad_column(&session.id.0, id_width),
            catalog::glyph(session.effective_kind()),
            pad_column(&status_label(&session.status), status_width),
            session.title
        );
    }
}

pub fn print_session_detail(record: &SessionRecord) {
    println!("id        {}", record.id.0);
    println!("title     {}", record.title);
    println!(
        "kind      {}",
        catalog::short_label(record.effective_kind())
    );
    println!("status    {}", status_label(&record.status));
    println!("cwd       {}", record.cwd);
    if let Some(branch) = &record.git_branch {
        println!("branch    {branch}");
    }
    if let Some(worktree) = &record.worktree_path {
        println!("worktree  {worktree}");
    }
    if let Some(host) = &record.host {
        println!("host      {host}");
    }
    if let Some(parent) = &record.parent {
        println!("parent    {}", parent.0);
    }
    if let Some(needs_input) = &record.needs_input {
        println!(
            "blocked   {}: {}",
            needs_input_kind(&needs_input.kind),
            needs_input.summary
        );
        if let Some(tool) = &needs_input.tool_name {
            println!("  tool    {tool}");
        }
        println!(
            "  risk    {}",
            serde_json::to_value(needs_input.risk_hint)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "neutral".into())
        );
    }
    if let Some(memory) = record.memory_bytes {
        println!("memory    {} MB", memory / 1_048_576);
    }
    println!("resume    {}", resumability_label(&record.resumability));
}

pub fn event_line(name: &str, seq: u64, params: &Value) -> String {
    let head = format!(
        "{}{}",
        pad_column(&seq.to_string(), 6),
        pad_column(name, 22)
    );
    match name {
        "session.status" => {
            let id = str_field(params, "id");
            let label = str_field(params, "label");
            let blocker = params
                .get("needsInput")
                .and_then(|value| value.get("summary"))
                .and_then(Value::as_str);
            match blocker {
                Some(summary) => format!("{head}{id}  {label}  — {summary}"),
                None => format!("{head}{id}  {label}"),
            }
        }
        "session.needs_input" => {
            let id = str_field(params, "id");
            let detail = params.get("needsInput");
            let summary = detail
                .and_then(|value| value.get("summary"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let risk = detail
                .and_then(|value| value.get("riskHint"))
                .and_then(Value::as_str)
                .unwrap_or("neutral");
            format!("{head}{id}  [{risk}] {summary}")
        }
        "session.artifact" => {
            let id = str_field(params, "id");
            let kind = str_field(params, "kind");
            if let Some(url) = params.get("url").and_then(Value::as_str) {
                format!("{head}{id}  {kind}  {url}")
            } else if let Some(port) = params.get("port").and_then(Value::as_i64) {
                format!("{head}{id}  port  localhost:{port}")
            } else {
                format!("{head}{id}  {kind}")
            }
        }
        "session.output" | "session.removed" => {
            format!("{head}{}", str_field(params, "id"))
        }
        "session.spawned" | "session.updated" | "session.archived" => {
            format!(
                "{head}{}  {}",
                str_field(params, "id"),
                params
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            )
        }
        "worktree.created" | "worktree.removed" => {
            format!("{head}{}", str_field(params, "path"))
        }
        "events.dropped" => {
            let dropped = params.get("dropped").and_then(Value::as_i64).unwrap_or(0);
            format!("{head}lost {dropped} events — re-read state")
        }
        _ => format!("{head}{}", encode_compact(params)),
    }
}

fn str_field<'a>(params: &'a Value, key: &str) -> &'a str {
    params.get(key).and_then(Value::as_str).unwrap_or("?")
}

pub fn artifact_kind(kind: &ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::PullRequest => "pullRequest",
        ArtifactKind::LinearIssue => "linearIssue",
        ArtifactKind::Preview => "preview",
        ArtifactKind::Link => "link",
        ArtifactKind::Unknown => "unknown",
    }
}

pub fn pr_overall(pr: &PullRequestStatus) -> &'static str {
    if pr.state == "MERGED" {
        "merged"
    } else if pr.state == "CLOSED" {
        "closed"
    } else if pr.is_draft {
        "draft"
    } else if pr.mergeable.as_deref() == Some("CONFLICTING") {
        "conflicts"
    } else if pr.checks_failed > 0 {
        "checks failing"
    } else if pr.review_decision.as_deref() == Some("CHANGES_REQUESTED") {
        "changes requested"
    } else if pr.checks_pending > 0 {
        "checks pending"
    } else if pr.review_decision.as_deref() == Some("REVIEW_REQUIRED") {
        "needs review"
    } else if pr.merge_state_status.as_deref() == Some("BLOCKED") {
        "blocked"
    } else {
        "ready"
    }
}

pub fn iso8601(millis: f64) -> String {
    let total = millis.max(0.0) as i64;
    let secs = total / 1000;
    let ms = total % 1000;
    let (year, month, day, hour, minute, second) = civil_from_unix(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z")
}

fn civil_from_unix(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400) as u32;
    let hour = tod / 3600;
    let minute = (tod % 3600) / 60;
    let second = tod % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = (yoe as i64 + era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day, hour, minute, second)
}
