use std::env;
use std::io::{self, Read};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::Value;
use zeus_proto::paths::ZeusEnv;
use zeus_proto::{SessionId, SessionRecord};

use crate::error::CliError;

pub const WAIT_TARGETS: &[&str] = &[
    "done",
    "idle",
    "working",
    "starting",
    "needs-input",
    "exited",
];

pub fn session_id() -> Option<SessionId> {
    env::var(ZeusEnv::SESSION_ID)
        .ok()
        .filter(|value| !value.is_empty())
        .map(SessionId::new)
}

pub fn encode_compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
}

pub fn parse_payload(bytes: &[u8]) -> Value {
    if !bytes.is_empty()
        && let Ok(value) = serde_json::from_slice::<Value>(bytes)
    {
        return value;
    }
    serde_json::json!({ "raw": String::from_utf8_lossy(bytes) })
}

pub fn read_stdin(cap: usize, timeout: Duration) -> Vec<u8> {
    #[cfg(unix)]
    {
        read_stdin_timeout(cap, timeout)
    }
    #[cfg(not(unix))]
    {
        let _ = timeout;
        let mut data = Vec::new();
        let _ = io::stdin().take(cap as u64).read_to_end(&mut data);
        data
    }
}

#[cfg(unix)]
fn read_stdin_timeout(cap: usize, timeout: Duration) -> Vec<u8> {
    let mut data = Vec::new();
    let mut buffer = [0_u8; 65536];
    while data.len() < cap {
        let mut pollfd = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        let ready = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if ready <= 0 {
            break;
        }
        let want = (cap - data.len()).min(buffer.len());
        let read = unsafe { libc::read(0, buffer.as_mut_ptr().cast(), want) };
        if read <= 0 {
            break;
        }
        data.extend_from_slice(&buffer[..read as usize]);
    }
    data
}

pub fn read_stdin_to_string() -> String {
    let mut data = String::new();
    let _ = io::stdin().read_to_string(&mut data);
    data.trim_end_matches(['\n', '\r']).to_string()
}

pub fn which(name: &str) -> Option<String> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        is_executable(&candidate).then(|| candidate.display().to_string())
    })
}

pub fn is_executable(path: &std::path::Path) -> bool {
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

pub fn resolve_session<'a>(
    needle: &str,
    sessions: &'a [SessionRecord],
) -> Result<&'a SessionRecord, CliError> {
    if let Some(exact) = sessions.iter().find(|session| session.id.0 == needle) {
        return Ok(exact);
    }
    let by_prefix: Vec<_> = sessions
        .iter()
        .filter(|session| session.id.0.starts_with(needle))
        .collect();
    if by_prefix.len() == 1 {
        return Ok(by_prefix[0]);
    }
    let lowered = needle.to_ascii_lowercase();
    let by_title: Vec<_> = sessions
        .iter()
        .filter(|session| session.title.to_ascii_lowercase().contains(&lowered))
        .collect();
    if by_title.len() == 1 {
        return Ok(by_title[0]);
    }
    let candidates = if by_prefix.is_empty() {
        by_title
    } else {
        by_prefix
    };
    if candidates.len() > 1 {
        let ids = candidates
            .iter()
            .map(|session| session.id.0.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(CliError::Failure(format!(
            "\"{needle}\" matches {} sessions: {ids}",
            candidates.len()
        )));
    }
    Err(CliError::NotFound(format!("no such session: {needle}")))
}

pub fn cwd() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn deadline_from_secs(seconds: f64) -> Instant {
    Instant::now() + Duration::from_secs_f64(seconds.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeus_proto::{AgentKind, DateMillis, ProjectId, Resumability, SessionStatus, TitleSource};

    fn record(id: &str, title: &str) -> SessionRecord {
        SessionRecord {
            id: SessionId::new(id),
            kind: AgentKind::CLAUDE_CODE,
            cwd: "/tmp".into(),
            project_id: ProjectId::new("p"),
            worktree_path: None,
            git_branch: None,
            title: title.into(),
            title_source: TitleSource::Placeholder,
            agent_session_id: None,
            transcript_path: None,
            status: SessionStatus::Idle,
            needs_input: None,
            resumability: Resumability::Live,
            parent: None,
            created_at: DateMillis(0.0),
            updated_at: DateMillis(0.0),
            last_turn_completed_at: None,
            last_seen_at: None,
            pinned: false,
            archived_at: None,
            host: None,
            remote_persistence: None,
            hibernation: None,
            memory_bytes: None,
            artifacts: None,
            pull_requests: None,
            listening_ports: None,
            foreground_agent: None,
        }
    }

    #[test]
    fn session_targets_resolve_by_id_prefix_and_title() {
        let sessions = vec![
            record("s_alpha1", "Refactor the parser"),
            record("s_beta22", "Ship the release"),
        ];
        assert_eq!(
            resolve_session("s_alpha1", &sessions).unwrap().id.0,
            "s_alpha1"
        );
        assert_eq!(resolve_session("s_al", &sessions).unwrap().id.0, "s_alpha1");
        assert_eq!(
            resolve_session("release", &sessions).unwrap().id.0,
            "s_beta22"
        );
        assert_eq!(
            resolve_session("RELEASE", &sessions).unwrap().id.0,
            "s_beta22"
        );
        assert!(matches!(
            resolve_session("s_", &sessions),
            Err(CliError::Failure(_))
        ));
        assert!(matches!(
            resolve_session("nothing", &sessions),
            Err(CliError::NotFound(_))
        ));
    }
}
