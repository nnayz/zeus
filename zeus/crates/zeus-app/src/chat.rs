//! Bounded, read-only transcript projection for the native chat surface.
//!
//! Agent transcript formats remain owned by their respective CLIs. Zeus only
//! extracts user/assistant text and never writes, logs, or forwards transcript
//! payloads.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

const MAX_TRANSCRIPT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_MESSAGES: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
}

pub fn read_transcript(path: &Path) -> io::Result<Vec<ChatMessage>> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(MAX_TRANSCRIPT_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(file);
    if start > 0 {
        let mut partial = String::new();
        reader.read_line(&mut partial)?;
    }

    let mut messages = Vec::new();
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(message) = parse_message(&value) {
            if messages
                .last()
                .is_some_and(|last: &ChatMessage| last == &message)
            {
                continue;
            }
            messages.push(message);
            if messages.len() > MAX_MESSAGES {
                messages.remove(0);
            }
        }
    }
    Ok(messages)
}

fn parse_message(value: &Value) -> Option<ChatMessage> {
    match value.get("type").and_then(Value::as_str) {
        Some("user") => {
            text_content(value.get("message")?.get("content")?).map(|text| ChatMessage {
                role: ChatRole::User,
                text,
            })
        }
        Some("assistant") => {
            text_content(value.get("message")?.get("content")?).map(|text| ChatMessage {
                role: ChatRole::Assistant,
                text,
            })
        }
        Some("event_msg") => {
            let payload = value.get("payload")?;
            let role = match payload.get("type").and_then(Value::as_str)? {
                "user_message" => ChatRole::User,
                "agent_message" => ChatRole::Assistant,
                _ => return None,
            };
            payload
                .get("message")
                .and_then(Value::as_str)
                .map(|text| ChatMessage {
                    role,
                    text: text.to_owned(),
                })
        }
        Some("response_item") => {
            let payload = value.get("payload")?;
            if payload.get("role").and_then(Value::as_str) != Some("assistant") {
                return None;
            }
            text_content(payload.get("content")?).map(|text| ChatMessage {
                role: ChatRole::Assistant,
                text,
            })
        }
        _ => None,
    }
}

fn text_content(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return (!text.trim().is_empty()).then(|| text.to_owned());
    }
    let text = content
        .as_array()?
        .iter()
        .filter_map(|item| {
            let kind = item.get("type").and_then(Value::as_str);
            matches!(kind, Some("text" | "output_text"))
                .then(|| item.get("text").and_then(Value::as_str))
                .flatten()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.trim().is_empty()).then_some(text)
}

pub fn preview_messages() -> Vec<ChatMessage> {
    vec![
        ChatMessage {
            role: ChatRole::User,
            text: "Bring the workspace explorer and editor into the main flow, then verify the new glass material.".into(),
        },
        ChatMessage {
            role: ChatRole::Assistant,
            text: "I’ve mapped the work into three layers:\n\n- a full-height **Explorer**\n- a native editor with language detection\n- a transcript-backed chat surface\n\nI’m keeping the terminal one click away for raw agent control.".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_and_codex_text_messages() {
        let claude: Value = serde_json::from_str(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]}}"#,
        )
        .unwrap();
        let codex: Value = serde_json::from_str(
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"Fix it"}}"#,
        )
        .unwrap();
        assert_eq!(parse_message(&claude).unwrap().text, "Done.");
        assert_eq!(parse_message(&codex).unwrap().role, ChatRole::User);
    }
}
