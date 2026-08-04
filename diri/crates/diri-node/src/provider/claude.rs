use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::process::Command;

use crate::accounts::profile_environment;
use crate::error::{NodeError, NodeResult};
use diri_proto::ProviderKind;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(45);

pub async fn claude_status(config_home: &Path) -> NodeResult<Value> {
    command_json(config_home, &["auth", "status", "--json"], None).await
}

pub async fn claude_call(config_home: &Path, method: &str, params: Value) -> NodeResult<Value> {
    match method {
        "account/read" => claude_status(config_home).await,
        "session/list" => command_json(config_home, &["agents", "--json", "--all"], None).await,
        "session/start" => {
            let cwd = required_string(&params, "cwd")?;
            let prompt = required_string(&params, "prompt")?;
            command_json(
                config_home,
                &["--bg", "--print", prompt],
                Some(Path::new(cwd)),
            )
            .await
        }
        "session/resume" => {
            let cwd = required_string(&params, "cwd")?;
            let session_id = required_string(&params, "sessionId")?;
            command_json(
                config_home,
                &["--bg", "--resume", session_id],
                Some(Path::new(cwd)),
            )
            .await
        }
        "session/fork" => {
            let cwd = required_string(&params, "cwd")?;
            let session_id = required_string(&params, "sessionId")?;
            command_json(
                config_home,
                &["--bg", "--fork-session", "--resume", session_id],
                Some(Path::new(cwd)),
            )
            .await
        }
        "usage/read" => Ok(json!({
            "source": "node-ledger",
            "note": "Claude usage is collected from OpenTelemetry or transcript fallback"
        })),
        _ => Err(NodeError::BadRequest(format!(
            "Claude method `{method}` is not exposed by the node"
        ))),
    }
}

async fn command_json(
    config_home: &Path,
    arguments: &[&str],
    cwd: Option<&Path>,
) -> NodeResult<Value> {
    let (variable, value) = profile_environment(ProviderKind::Claude, config_home);
    let mut command = Command::new("claude");
    command
        .args(arguments)
        .env(variable, value)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = tokio::time::timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| NodeError::Provider("Claude command timed out".into()))?
        .map_err(|error| NodeError::Provider(format!("could not start Claude: {error}")))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(NodeError::Provider(if error.is_empty() {
            format!("Claude exited with {}", output.status)
        } else {
            error
        }));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(json!({"ok": true}));
    }
    Ok(serde_json::from_str(trimmed).unwrap_or_else(|_| json!({"output": trimmed})))
}

fn required_string<'a>(params: &'a Value, key: &str) -> NodeResult<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| NodeError::BadRequest(format!("missing `{key}`")))
}
