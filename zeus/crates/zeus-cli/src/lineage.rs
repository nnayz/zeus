use zeus_proto::{SessionId, SessionRecord, SessionStatus};

use crate::error::CliError;
use crate::render::iso8601;
use crate::support::session_id;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    Caller,
    Parent,
    Child,
    Ancestor,
    Descendant,
    Sibling,
    Unrelated,
}

impl Relation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Caller => "self",
            Self::Parent => "parent",
            Self::Child => "child",
            Self::Ancestor => "ancestor",
            Self::Descendant => "descendant",
            Self::Sibling => "sibling",
            Self::Unrelated => "unrelated",
        }
    }

    pub fn delivers_verbatim(self) -> bool {
        matches!(self, Self::Parent | Self::Child)
    }
}

pub struct SessionLineage {
    records: Vec<SessionRecord>,
    caller: Option<SessionId>,
}

impl SessionLineage {
    pub fn current(records: Vec<SessionRecord>) -> Self {
        Self {
            caller: session_id(),
            records,
        }
    }

    pub fn caller(&self) -> Option<&SessionId> {
        self.caller.as_ref()
    }

    pub fn require_caller(&self) -> Result<&SessionId, CliError> {
        self.caller.as_ref().ok_or_else(|| {
            CliError::Failure(
                "This tool needs to know which session is calling it, and ZEUS_SESSION_ID is unset — run it from a session hosted by Zeus.".into(),
            )
        })
    }

    pub fn record(&self, id: &SessionId) -> Option<&SessionRecord> {
        self.records.iter().find(|record| record.id.0 == id.0)
    }

    pub fn caller_record(&self) -> Option<&SessionRecord> {
        self.caller.as_ref().and_then(|id| self.record(id))
    }

    pub fn children(&self, id: &SessionId) -> Vec<&SessionRecord> {
        let mut children: Vec<_> = self
            .records
            .iter()
            .filter(|record| {
                record
                    .parent
                    .as_ref()
                    .is_some_and(|parent| parent.0 == id.0)
            })
            .collect();
        children.sort_by(|left, right| left.created_at.0.total_cmp(&right.created_at.0));
        children
    }

    pub fn descendants(&self, id: &SessionId) -> Vec<&SessionRecord> {
        let mut seen = vec![id.0.clone()];
        let mut queue = self.children(id);
        let mut out = Vec::new();
        while !queue.is_empty() {
            let next = queue.remove(0);
            if seen.iter().any(|item| item == &next.id.0) {
                continue;
            }
            seen.push(next.id.0.clone());
            out.push(next);
            queue.extend(self.children(&next.id));
        }
        out
    }

    pub fn ancestors(&self, id: &SessionId) -> Vec<&SessionRecord> {
        let mut seen = vec![id.0.clone()];
        let mut out = Vec::new();
        let mut cursor = self.record(id).and_then(|record| record.parent.clone());
        while let Some(current) = cursor {
            if seen.iter().any(|item| item == &current.0) {
                break;
            }
            seen.push(current.0.clone());
            let Some(record) = self.record(&current) else {
                break;
            };
            out.push(record);
            cursor = record.parent.clone();
        }
        out
    }

    pub fn relation(&self, target: &SessionId) -> Relation {
        let Some(caller) = &self.caller else {
            return Relation::Unrelated;
        };
        if caller.0 == target.0 {
            return Relation::Caller;
        }
        if self
            .record(caller)
            .and_then(|record| record.parent.as_ref())
            .is_some_and(|parent| parent.0 == target.0)
        {
            return Relation::Parent;
        }
        if self
            .record(target)
            .and_then(|record| record.parent.as_ref())
            .is_some_and(|parent| parent.0 == caller.0)
        {
            return Relation::Child;
        }
        if self
            .ancestors(caller)
            .iter()
            .any(|record| record.id.0 == target.0)
        {
            return Relation::Ancestor;
        }
        if self
            .descendants(caller)
            .iter()
            .any(|record| record.id.0 == target.0)
        {
            return Relation::Descendant;
        }
        let mine = self
            .record(caller)
            .and_then(|record| record.parent.as_ref());
        let theirs = self
            .record(target)
            .and_then(|record| record.parent.as_ref());
        if mine.is_some() && mine == theirs {
            return Relation::Sibling;
        }
        Relation::Unrelated
    }

    pub fn frame(&self, text: &str, relation: Relation) -> String {
        if relation.delivers_verbatim() {
            return text.to_string();
        }
        let Some(caller) = &self.caller else {
            return text.to_string();
        };
        let who = match self.caller_record().map(|record| record.title.as_str()) {
            Some(title) => format!("id:{} ({title})", caller.0),
            None => format!("id:{}", caller.0),
        };
        format!("[message from {who}, channel: zeus — reply with send_prompt to that id]\n\n{text}")
    }

    pub fn write_policy() -> &'static str {
        "Reads are open across all sessions. Writes to your parent or your direct \
         children are delivered verbatim; writes to anyone else are prefixed with a \
         provenance header naming you, so the receiving agent knows an unrelated \
         session is talking to it. You cannot send_prompt to yourself, and \
         release_agent refuses to kill you or any of your ancestors."
    }
}

pub fn compact(record: &SessionRecord) -> Value {
    let mut obj = json!({
        "id": record.id.0,
        "kind": crate::catalog::short_label(&record.kind),
        "title": record.title,
        "status": crate::render::status_label(&record.status),
        "cwd": record.cwd,
    });
    if let Some(parent) = &record.parent {
        obj["parent"] = json!(parent.0);
    }
    if let Some(host) = &record.host {
        obj["host"] = json!(host);
    }
    obj
}

pub fn detailed(record: &SessionRecord, relation: Option<Relation>) -> Value {
    let mut obj = compact(record);
    if let Some(relation) = relation {
        obj["relation"] = json!(relation.as_str());
    }
    if let Some(branch) = &record.git_branch {
        obj["branch"] = json!(branch);
    }
    if let Some(worktree) = &record.worktree_path {
        obj["worktree"] = json!(worktree);
    }
    obj["created_at"] = json!(iso8601(record.created_at.0));
    if record.is_archived() {
        obj["archived"] = json!(true);
    }
    obj
}

pub fn is_running(status: &SessionStatus) -> bool {
    !matches!(status, SessionStatus::Exited(_))
}

pub fn has_reached(mode: &str, status: &SessionStatus) -> bool {
    match mode {
        "exited" => matches!(status, SessionStatus::Exited(_)),
        "done" => matches!(status, SessionStatus::Idle | SessionStatus::Exited(_)),
        _ => matches!(
            status,
            SessionStatus::Idle | SessionStatus::NeedsInput(_) | SessionStatus::Exited(_)
        ),
    }
}

pub struct ChildReport {
    pub status: String,
    pub summary: String,
    pub details: Option<String>,
    pub blockers: Vec<String>,
    pub questions: Vec<String>,
    pub next_steps: Vec<String>,
    pub changed_paths: Vec<String>,
    pub artifacts: Vec<String>,
    pub proof: Vec<String>,
}

impl ChildReport {
    pub const STATUSES: &'static [&'static str] = &["update", "done", "blocked", "failed"];

    pub fn rendered(
        &self,
        sender: Option<&SessionRecord>,
        sender_id: Option<&SessionId>,
    ) -> String {
        let who = match (sender_id, sender.map(|record| record.title.as_str())) {
            (Some(id), Some(title)) => format!("id:{} ({title})", id.0),
            (Some(id), None) => format!("id:{}", id.0),
            _ => "an unidentified session".into(),
        };
        let mut lines = vec![
            format!("[report from {who} · status: {}]", self.status),
            String::new(),
            format!("Summary: {}", self.summary),
        ];
        if let Some(details) = &self.details
            && !details.is_empty()
        {
            lines.push(String::new());
            lines.push(details.clone());
        }
        append_section(&mut lines, "Blockers", &self.blockers);
        append_section(&mut lines, "Questions", &self.questions);
        append_section(&mut lines, "Next steps", &self.next_steps);
        append_section(&mut lines, "Changed", &self.changed_paths);
        append_section(&mut lines, "Artifacts", &self.artifacts);
        append_section(&mut lines, "Proof", &self.proof);
        lines.join("\n")
    }
}

fn append_section(lines: &mut Vec<String>, title: &str, items: &[String]) {
    let kept: Vec<_> = items
        .iter()
        .filter(|item| !item.is_empty())
        .cloned()
        .collect();
    if kept.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push(format!("{title}:"));
    lines.extend(kept.into_iter().map(|item| format!("- {item}")));
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeus_proto::{AgentKind, DateMillis, ProjectId, Resumability, TitleSource};

    fn record(id: &str, parent: Option<&str>) -> SessionRecord {
        SessionRecord {
            id: SessionId::new(id),
            kind: AgentKind::CLAUDE_CODE,
            cwd: "/tmp".into(),
            project_id: ProjectId::new("p"),
            worktree_path: None,
            git_branch: None,
            title: id.into(),
            title_source: TitleSource::Placeholder,
            agent_session_id: None,
            transcript_path: None,
            status: SessionStatus::Idle,
            needs_input: None,
            resumability: Resumability::Live,
            parent: parent.map(SessionId::new),
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
            workbench: None,
        }
    }

    #[test]
    fn relation_walks_the_spawn_graph() {
        let lineage = SessionLineage {
            caller: Some(SessionId::new("child")),
            records: vec![
                record("root", None),
                record("child", Some("root")),
                record("grand", Some("child")),
                record("sib", Some("root")),
                record("other", None),
            ],
        };
        assert_eq!(lineage.relation(&SessionId::new("child")), Relation::Caller);
        assert_eq!(lineage.relation(&SessionId::new("root")), Relation::Parent);
        assert_eq!(lineage.relation(&SessionId::new("grand")), Relation::Child);
        assert_eq!(lineage.relation(&SessionId::new("sib")), Relation::Sibling);
        assert_eq!(
            lineage.relation(&SessionId::new("other")),
            Relation::Unrelated
        );
    }

    #[test]
    fn report_drops_empty_sections() {
        let report = ChildReport {
            status: "done".into(),
            summary: "shipped".into(),
            details: None,
            blockers: vec![],
            questions: vec![],
            next_steps: vec![],
            changed_paths: vec![],
            artifacts: vec![],
            proof: vec![],
        };
        let rendered = report.rendered(None, Some(&SessionId::new("s_x")));
        assert!(rendered.contains("id:s_x"));
        assert!(rendered.contains("Summary: shipped"));
        assert!(!rendered.contains("Blockers:"));
    }
}
