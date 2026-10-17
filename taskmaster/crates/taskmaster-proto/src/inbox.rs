//! The needs-you inbox: one triage queue across every live session.
//!
//! The sidebar stays the map of the fleet; this module is the work. It folds
//! `SessionRecord`s — local and remote alike, there is no second queue for
//! hosts — plus optional per-session change probes into ordered [`InboxItem`]s,
//! and owns only the user's dismissal/snooze decisions.
//!
//! Two rules are load-bearing:
//!
//! 1. **Never invent a blocker.** An item exists only when the reducer (or the
//!    exit facts) already named what needs the human. A `needsInput` status
//!    without a captured [`NeedsInputDetail`], or with an empty summary, is not
//!    an item the inbox may fabricate copy for.
//! 2. **A working session is never a fake "needs you."** Classification keys
//!    off the canonical status, so stale `needs_input` payloads on a session
//!    that has moved on cannot resurrect an entry.
//!
//! Pure and deterministic: no clock, no IO. The caller passes `now`, which is
//! what makes snooze expiry testable without sleeping.

use std::collections::{HashMap, HashSet};

use crate::model::{
    AgentKind, DateMillis, ExitInfo, ExitReason, NeedsInputDetail, NeedsInputKind, ProjectId,
    RiskHint, SessionId, SessionRecord, SessionStatus,
};

/// What a client learned about a finished session's workspace.
///
/// `git.workspace`-style probes are the only source of this signal, so the
/// queue itself never guesses that a session "produced a change" it cannot
/// see. [`SessionChange::Unknown`] (not yet probed) and
/// [`SessionChange::Unchanged`] both mean "no done-with-diff item".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionChange {
    /// Not probed (yet).
    #[default]
    Unknown,
    /// The workspace has no changes, or the session has no repository.
    Unchanged,
    /// The workspace holds uncommitted work from the session.
    Changed {
        /// Changed, staged, untracked, and conflicted files together.
        files: u32,
    },
}

impl SessionChange {
    /// A change signal can only come from evidence with at least one file.
    pub const fn changed_files(&self) -> Option<u32> {
        match self {
            Self::Changed { files } if *files > 0 => Some(*files),
            Self::Unknown | Self::Unchanged | Self::Changed { .. } => None,
        }
    }
}

/// Triage order of an inbox item: blockers first, then failures, then results
/// waiting on review. The rank is total and stable so sorting is deterministic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum InboxPriority {
    Permission,
    Question,
    Failure,
    DoneWithDiff,
}

impl InboxPriority {
    pub const fn rank(self) -> u8 {
        self as u8
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Permission => "Permission",
            Self::Question => "Question",
            Self::Failure => "Failure",
            Self::DoneWithDiff => "Done",
        }
    }
}

/// Why a session is in the inbox, with the facts the item was built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InboxItemKind {
    /// A captured permission prompt, carrying the reducer's risk label.
    Permission { risk: RiskHint },
    /// A question or confirmation the agent is waiting on.
    Question { risk: RiskHint },
    /// A failed or unexpected exit; `detail` names it ("exited with code 1").
    Failure { detail: String },
    /// The session finished a turn and its workspace changed.
    DoneWithDiff { files: u32 },
}

impl InboxItemKind {
    pub const fn priority(&self) -> InboxPriority {
        match self {
            Self::Permission { .. } => InboxPriority::Permission,
            Self::Question { .. } => InboxPriority::Question,
            Self::Failure { .. } => InboxPriority::Failure,
            Self::DoneWithDiff { .. } => InboxPriority::DoneWithDiff,
        }
    }

    pub const fn risk(&self) -> Option<RiskHint> {
        match self {
            Self::Permission { risk } | Self::Question { risk } => Some(*risk),
            Self::Failure { .. } | Self::DoneWithDiff { .. } => None,
        }
    }
}

/// Stable identity of one inbox item, used by dismiss and snooze.
///
/// The fingerprint is per *occurrence*, not per session: dismissing a
/// permission with one prompt must not silence a different one that arrives
/// later. The fields mirror what the reducer already treats as one blocker.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InboxItemKey {
    pub session_id: SessionId,
    pub fingerprint: String,
}

impl InboxItemKey {
    pub fn new(session_id: SessionId, fingerprint: impl Into<String>) -> Self {
        Self {
            session_id,
            fingerprint: fingerprint.into(),
        }
    }
}

/// One thing that actually needs a human, named as precisely as the reducer
/// could name it.
#[derive(Clone, Debug, PartialEq)]
pub struct InboxItem {
    pub key: InboxItemKey,
    pub session_id: SessionId,
    pub project_id: ProjectId,
    /// The session title, for context. The headline is the ask itself.
    pub title: String,
    /// The agent the human would be answering (foreground agent when set).
    pub kind: AgentKind,
    /// `HostEntry.id` when the session runs on a remote host; `None` is local.
    /// Both kinds share this one queue; the field exists for display.
    pub host: Option<String>,
    /// The human-readable ask: captured prompt text, the failure, or the
    /// change summary.
    pub headline: String,
    /// Longer captured evidence (a prompt excerpt) when one exists.
    pub excerpt: Option<String>,
    pub reason: InboxItemKind,
    /// When the item's own event happened: the prompt, the exit, the turn
    /// completion.
    pub occurred_at: DateMillis,
}

impl InboxItem {
    pub const fn priority(&self) -> InboxPriority {
        self.reason.priority()
    }

    pub fn is_remote(&self) -> bool {
        self.host.is_some()
    }
}

/// Dismiss/snooze decisions over the derived items, plus the pure fold that
/// turns sessions into items.
///
/// The queue is rebuilt from source-of-truth records on every read; only
/// human decisions are state. That is what keeps a session from lingering in
/// the inbox after it moved on: the next fold simply does not produce an item.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InboxQueue {
    dismissed: HashSet<InboxItemKey>,
    snoozed: HashMap<InboxItemKey, DateMillis>,
}

impl InboxQueue {
    /// Hide one item until the session produces something new.
    pub fn dismiss(&mut self, key: &InboxItemKey) {
        self.snoozed.remove(key);
        self.dismissed.insert(key.clone());
    }

    /// Hide one item until `until`; afterwards it returns unless resolved.
    pub fn snooze(&mut self, key: &InboxItemKey, until: DateMillis) {
        self.snoozed.insert(key.clone(), until);
    }

    pub fn is_hidden(&self, key: &InboxItemKey, now: DateMillis) -> bool {
        if self.dismissed.contains(key) {
            return true;
        }
        self.snoozed.get(key).is_some_and(|until| now.0 < until.0)
    }

    /// Number of decisions currently suppressing an item.
    pub fn hidden(&self) -> usize {
        self.dismissed.len() + self.snoozed.len()
    }

    /// Bring every dismissed and snoozed item back. Returns how many decisions
    /// were cleared.
    pub fn restore_all(&mut self) -> usize {
        let restored = self.dismissed.len() + self.snoozed.len();
        self.dismissed.clear();
        self.snoozed.clear();
        restored
    }

    /// Drop decisions for sessions that no longer exist; they can never be
    /// matched again, and the daemon's session cap is the only bound on how
    /// many there could be.
    pub fn retain_sessions(&mut self, live: &HashSet<SessionId>) {
        self.dismissed.retain(|key| live.contains(&key.session_id));
        self.snoozed.retain(|key, _| live.contains(&key.session_id));
    }

    /// Fold every live session into the ordered triage queue.
    ///
    /// `changes` carries whatever workspace probes have answered; sessions
    /// without an answer are treated as not-yet-known, never as changed.
    pub fn items<'a>(
        &self,
        sessions: impl IntoIterator<Item = &'a SessionRecord>,
        changes: &HashMap<SessionId, SessionChange>,
        now: DateMillis,
    ) -> Vec<InboxItem> {
        let mut items: Vec<_> = sessions
            .into_iter()
            .filter_map(|session| {
                let change = changes
                    .get(&session.id)
                    .copied()
                    .unwrap_or(SessionChange::Unknown);
                classify(session, change)
            })
            .filter(|item| !self.is_hidden(&item.key, now))
            .collect();
        // Blocker first, then failure, then finished work; newest first inside
        // a rank, with the session id as the final stable tie-break.
        items.sort_by(|left, right| {
            left.priority()
                .rank()
                .cmp(&right.priority().rank())
                .then_with(|| {
                    right
                        .occurred_at
                        .0
                        .partial_cmp(&left.occurred_at.0)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| left.session_id.0.cmp(&right.session_id.0))
        });
        items
    }
}

/// Classify one session, or return `None` when nothing nameable needs a human.
pub fn classify(session: &SessionRecord, change: SessionChange) -> Option<InboxItem> {
    if session.is_archived() {
        return None;
    }
    match &session.status {
        // Only the canonical status decides. A stale `needs_input` payload on a
        // session that has moved back to working must not fabricate an item.
        SessionStatus::NeedsInput(_) => blocker_item(session),
        SessionStatus::Exited(info) => failure_item(session, info),
        SessionStatus::Idle => done_with_diff_item(session, change),
        // A working session is never a fake "needs you"; starting and unknown
        // carry nothing the inbox can name either.
        SessionStatus::Working | SessionStatus::Starting | SessionStatus::Unknown => None,
    }
}

/// A prompt the reducer captured. Without the capture there is nothing to
/// name, and the inbox does not invent copy: no detail, no empty summary, no
/// item.
fn blocker_item(session: &SessionRecord) -> Option<InboxItem> {
    let detail = session.needs_input.as_ref()?;
    let headline = detail.summary.trim();
    if headline.is_empty() {
        return None;
    }
    let reason = match detail.kind {
        NeedsInputKind::Permission => InboxItemKind::Permission {
            risk: detail.risk_hint,
        },
        NeedsInputKind::Question => InboxItemKind::Question {
            risk: detail.risk_hint,
        },
        // A kind this build cannot interpret is not a blocker the inbox may
        // turn into either a permission or a question.
        NeedsInputKind::Unknown => return None,
    };
    let fingerprint = blocker_fingerprint(detail);
    Some(item(
        session,
        InboxItemKey::new(session.id.clone(), fingerprint),
        reason,
        headline.to_owned(),
        non_empty(detail.prompt_excerpt.as_deref()),
        detail.occurred_at,
    ))
}

/// A failed or unexpected exit, named from the exit facts alone. Clean exits
/// (code 0), reason `Exited` without a code, daemon restarts, imported
/// sessions, and archived sessions are not the inbox's business.
fn failure_item(session: &SessionRecord, info: &ExitInfo) -> Option<InboxItem> {
    let detail = failure_detail(info)?;
    Some(item(
        session,
        InboxItemKey::new(session.id.clone(), format!("exit-{detail}")),
        InboxItemKind::Failure {
            detail: detail.clone(),
        },
        detail,
        None,
        session.updated_at,
    ))
}

fn failure_detail(info: &ExitInfo) -> Option<String> {
    match info.reason {
        ExitReason::Exited => match info.code {
            Some(0) | None => None,
            Some(code) => Some(format!("exited with code {code}")),
        },
        ExitReason::Signaled => Some(match info.signal {
            Some(signal) => format!("terminated by signal {signal}"),
            None => "terminated by a signal".to_owned(),
        }),
        ExitReason::DaemonRestart
        | ExitReason::External
        | ExitReason::Archived
        | ExitReason::Unknown => None,
    }
}

/// Done, with a change to review. Requires all three: a completed turn, a
/// result the human has not seen yet, and a probe that actually found a
/// change.
fn done_with_diff_item(session: &SessionRecord, change: SessionChange) -> Option<InboxItem> {
    let completed = session.last_turn_completed_at?;
    let seen = session
        .last_seen_at
        .map_or(f64::NEG_INFINITY, |seen| seen.0);
    if completed.0 <= seen {
        return None;
    }
    let files = change.changed_files()?;
    Some(item(
        session,
        InboxItemKey::new(session.id.clone(), format!("done-{}", completed.0 as i64)),
        InboxItemKind::DoneWithDiff { files },
        format!("{files} file{} changed", if files == 1 { "" } else { "s" }),
        None,
        completed,
    ))
}

fn blocker_fingerprint(detail: &NeedsInputDetail) -> String {
    // FNV-1a, matching the deterministic identity the notification policy
    // already uses for "one prompt per distinct blocker". No hasher is used
    // because `DefaultHasher` is intentionally randomized between runs.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let identity = format!(
        "{:?}\0{}\0{}",
        detail.kind,
        detail.tool_name.as_deref().unwrap_or_default(),
        detail.summary.trim()
    );
    for byte in identity.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("blocker-{hash:016x}")
}

#[allow(clippy::too_many_arguments)]
fn item(
    session: &SessionRecord,
    key: InboxItemKey,
    reason: InboxItemKind,
    headline: String,
    excerpt: Option<String>,
    occurred_at: DateMillis,
) -> InboxItem {
    InboxItem {
        key,
        session_id: session.id.clone(),
        project_id: session.project_id.clone(),
        title: session.title.clone(),
        kind: session.effective_kind().clone(),
        host: session.host.clone(),
        headline,
        excerpt,
        reason,
        occurred_at,
    }
}

fn non_empty(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DateMillis, NeedsInputSource, Resumability, TitleSource};

    fn session(id: &str, status: SessionStatus) -> SessionRecord {
        SessionRecord {
            id: SessionId::new(id),
            kind: AgentKind::CODEX,
            cwd: "/work/repo".to_owned(),
            project_id: ProjectId::new("project-1"),
            worktree_path: None,
            git_branch: None,
            title: format!("Session {id}"),
            title_source: TitleSource::Placeholder,
            agent_session_id: None,
            transcript_path: None,
            status,
            needs_input: None,
            resumability: Resumability::NotResumable,
            parent: None,
            created_at: DateMillis(0.0),
            updated_at: DateMillis(100.0),
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

    fn detail(kind: NeedsInputKind, summary: &str, risk: RiskHint) -> NeedsInputDetail {
        NeedsInputDetail {
            kind,
            source: NeedsInputSource::ClaudePermissionHook,
            tool_name: Some("Bash".to_owned()),
            summary: summary.to_owned(),
            prompt_excerpt: Some(format!("prompt: {summary}")),
            options: None,
            risk_hint: risk,
            occurred_at: DateMillis(500.0),
        }
    }

    fn permission(summary: &str, risk: RiskHint) -> SessionRecord {
        let mut session = session(
            "session-1",
            SessionStatus::NeedsInput(NeedsInputKind::Permission),
        );
        session.needs_input = Some(detail(NeedsInputKind::Permission, summary, risk));
        session
    }

    #[test]
    fn permission_prompts_carry_captured_text_and_risk() {
        let session = permission("delete the build directory?", RiskHint::Destructive);
        let item = classify(&session, SessionChange::Unknown).expect("permission item");
        assert_eq!(
            item.reason,
            InboxItemKind::Permission {
                risk: RiskHint::Destructive
            }
        );
        assert_eq!(item.headline, "delete the build directory?");
        assert_eq!(
            item.excerpt.as_deref(),
            Some("prompt: delete the build directory?")
        );
        assert_eq!(item.session_id, SessionId::new("session-1"));
        assert_eq!(item.occurred_at, DateMillis(500.0));
        assert_eq!(item.priority(), InboxPriority::Permission);
    }

    #[test]
    fn questions_and_confirmations_are_queued_as_questions() {
        let mut session = session(
            "session-1",
            SessionStatus::NeedsInput(NeedsInputKind::Question),
        );
        session.needs_input = Some(detail(
            NeedsInputKind::Question,
            "run the migration?",
            RiskHint::Neutral,
        ));
        let item = classify(&session, SessionChange::Unknown).expect("question item");
        assert_eq!(
            item.reason,
            InboxItemKind::Question {
                risk: RiskHint::Neutral
            }
        );
        assert_eq!(item.headline, "run the migration?");
        assert_eq!(item.priority(), InboxPriority::Question);
    }

    #[test]
    fn failures_and_unexpected_exits_are_named_from_exit_facts() {
        let failed = session(
            "session-failed",
            SessionStatus::Exited(ExitInfo {
                reason: ExitReason::Exited,
                code: Some(1),
                signal: None,
            }),
        );
        let item = classify(&failed, SessionChange::Unknown).expect("failure item");
        assert_eq!(
            item.reason,
            InboxItemKind::Failure {
                detail: "exited with code 1".to_owned()
            }
        );
        assert_eq!(item.headline, "exited with code 1");

        let killed = session(
            "session-killed",
            SessionStatus::Exited(ExitInfo {
                reason: ExitReason::Signaled,
                code: None,
                signal: Some(9),
            }),
        );
        let item = classify(&killed, SessionChange::Unknown).expect("signal failure item");
        assert_eq!(item.headline, "terminated by signal 9");
    }

    #[test]
    fn expected_exits_are_not_failures() {
        for info in [
            ExitInfo {
                reason: ExitReason::Exited,
                code: Some(0),
                signal: None,
            },
            // No code to name a failure with.
            ExitInfo {
                reason: ExitReason::Exited,
                code: None,
                signal: None,
            },
            ExitInfo {
                reason: ExitReason::DaemonRestart,
                code: Some(1),
                signal: None,
            },
            ExitInfo {
                reason: ExitReason::External,
                code: None,
                signal: None,
            },
            ExitInfo {
                reason: ExitReason::Archived,
                code: None,
                signal: None,
            },
            ExitInfo {
                reason: ExitReason::Unknown,
                code: Some(2),
                signal: None,
            },
        ] {
            let session = session("session-1", SessionStatus::Exited(info.clone()));
            assert_eq!(
                classify(&session, SessionChange::Unknown),
                None,
                "expected no inbox item for {info:?}"
            );
        }
    }

    #[test]
    fn done_with_diff_needs_a_finished_unseen_turn_and_a_found_change() {
        let mut session = session("session-1", SessionStatus::Idle);
        session.last_turn_completed_at = Some(DateMillis(900.0));

        // The classification is only as good as the probe: no change, no item.
        assert_eq!(classify(&session, SessionChange::Unknown), None);
        assert_eq!(classify(&session, SessionChange::Unchanged), None);
        assert_eq!(
            classify(&session, SessionChange::Changed { files: 0 }),
            None
        );

        let item =
            classify(&session, SessionChange::Changed { files: 3 }).expect("done-with-diff item");
        assert_eq!(item.reason, InboxItemKind::DoneWithDiff { files: 3 });
        assert_eq!(item.headline, "3 files changed");
        assert_eq!(item.occurred_at, DateMillis(900.0));
        assert_eq!(item.priority(), InboxPriority::DoneWithDiff);

        // A result the human already looked at is not queue work.
        session.last_seen_at = Some(DateMillis(950.0));
        assert_eq!(
            classify(&session, SessionChange::Changed { files: 3 }),
            None
        );

        // Neither is a turn that never completed.
        session.last_seen_at = None;
        session.last_turn_completed_at = None;
        assert_eq!(
            classify(&session, SessionChange::Changed { files: 3 }),
            None
        );
    }

    #[test]
    fn done_with_diff_reappears_when_the_next_turn_completes_with_changes() {
        let mut session = session("session-1", SessionStatus::Idle);
        session.last_turn_completed_at = Some(DateMillis(900.0));
        session.last_seen_at = Some(DateMillis(950.0));
        let mut changes = HashMap::new();
        changes.insert(session.id.clone(), SessionChange::Changed { files: 2 });
        let queue = InboxQueue::default();
        assert!(
            queue
                .items([&session], &changes, DateMillis(1_000.0))
                .is_empty()
        );

        let previous_turn = InboxItemKey::new(session.id.clone(), "done-900");
        session.last_turn_completed_at = Some(DateMillis(1_100.0));
        let items = queue.items([&session], &changes, DateMillis(1_200.0));
        assert_eq!(items.len(), 1);
        assert_ne!(items[0].key, previous_turn);
    }

    #[test]
    fn unnamed_blockers_are_never_invented() {
        // A needs-input status with no captured detail cannot be named.
        let bare = session(
            "session-1",
            SessionStatus::NeedsInput(NeedsInputKind::Permission),
        );
        assert_eq!(classify(&bare, SessionChange::Unknown), None);

        // A captured detail with no usable text cannot be named either.
        let mut empty = session(
            "session-2",
            SessionStatus::NeedsInput(NeedsInputKind::Permission),
        );
        empty.needs_input = Some(detail(NeedsInputKind::Permission, "   ", RiskHint::Neutral));
        assert_eq!(classify(&empty, SessionChange::Unknown), None);
    }

    #[test]
    fn working_sessions_never_appear_as_needs_you() {
        // Even with a stale prompt payload still on the record.
        let mut working = permission("approve the deploy?", RiskHint::Destructive);
        working.status = SessionStatus::Working;
        assert_eq!(classify(&working, SessionChange::Unknown), None);

        let mut starting = permission("approve the deploy?", RiskHint::Destructive);
        starting.status = SessionStatus::Starting;
        assert_eq!(classify(&starting, SessionChange::Unknown), None);

        let mut unknown = permission("approve the deploy?", RiskHint::Destructive);
        unknown.status = SessionStatus::Unknown;
        assert_eq!(classify(&unknown, SessionChange::Unknown), None);
    }

    #[test]
    fn archived_sessions_are_not_queued() {
        let mut archived = permission("approve the deploy?", RiskHint::Neutral);
        archived.archived_at = Some(DateMillis(600.0));
        assert_eq!(classify(&archived, SessionChange::Unknown), None);
    }

    #[test]
    fn local_and_remote_sessions_share_one_queue() {
        // The acceptance case: a permission on a remote Codex and a question on
        // a local Claude in one list.
        let mut remote = permission("push to origin/main?", RiskHint::Network);
        remote.kind = AgentKind::CODEX;
        remote.host = Some("forge".to_owned());
        remote.updated_at = DateMillis(700.0);

        let mut local = session(
            "session-2",
            SessionStatus::NeedsInput(NeedsInputKind::Question),
        );
        local.kind = AgentKind::CLAUDE_CODE;
        local.needs_input = Some(detail(
            NeedsInputKind::Question,
            "which database should I use?",
            RiskHint::Neutral,
        ));

        let queue = InboxQueue::default();
        let items = queue.items([&remote, &local], &HashMap::new(), DateMillis(1_000.0));
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].session_id, SessionId::new("session-1"));
        assert!(items[0].is_remote());
        assert_eq!(items[0].host.as_deref(), Some("forge"));
        assert_eq!(items[1].session_id, SessionId::new("session-2"));
        assert!(!items[1].is_remote());
        assert_eq!(items[1].kind, AgentKind::CLAUDE_CODE);
    }

    #[test]
    fn items_sort_blockers_first_then_failures_then_finished_work() {
        let done = {
            let mut session = session("session-done", SessionStatus::Idle);
            session.last_turn_completed_at = Some(DateMillis(950.0));
            session
        };
        let failed = session(
            "session-failed",
            SessionStatus::Exited(ExitInfo {
                reason: ExitReason::Exited,
                code: Some(2),
                signal: None,
            }),
        );
        let question = {
            let mut session = session(
                "session-question",
                SessionStatus::NeedsInput(NeedsInputKind::Question),
            );
            session.needs_input = Some(detail(
                NeedsInputKind::Question,
                "continue?",
                RiskHint::Neutral,
            ));
            session
        };
        let mut older_permission = permission("run tests?", RiskHint::Neutral);
        older_permission.id = SessionId::new("session-permission-old");
        older_permission
            .needs_input
            .as_mut()
            .expect("detail")
            .occurred_at = DateMillis(100.0);
        let mut newer_permission = permission("deploy?", RiskHint::Destructive);
        newer_permission.id = SessionId::new("session-permission-new");

        let mut changes = HashMap::new();
        changes.insert(done.id.clone(), SessionChange::Changed { files: 1 });
        let queue = InboxQueue::default();
        let items = queue.items(
            [
                &done,
                &failed,
                &question,
                &older_permission,
                &newer_permission,
            ],
            &changes,
            DateMillis(2_000.0),
        );
        let ids: Vec<_> = items.iter().map(|item| item.session_id.0.clone()).collect();
        assert_eq!(
            ids,
            vec![
                "session-permission-new",
                "session-permission-old",
                "session-question",
                "session-failed",
                "session-done",
            ]
        );
    }

    #[test]
    fn dismissed_items_stay_hidden_until_their_blocker_changes() {
        let mut queue = InboxQueue::default();
        let first = permission("approve the deploy?", RiskHint::Destructive);
        let items = queue.items([&first], &HashMap::new(), DateMillis(1_000.0));
        assert_eq!(items.len(), 1);
        queue.dismiss(&items[0].key);
        assert!(
            queue
                .items([&first], &HashMap::new(), DateMillis(1_000.0))
                .is_empty()
        );

        // A different prompt names a different blocker and must resurface.
        let mut second = first.clone();
        second.needs_input = Some(detail(
            NeedsInputKind::Permission,
            "delete the database?",
            RiskHint::Destructive,
        ));
        assert_eq!(
            queue
                .items([&second], &HashMap::new(), DateMillis(1_100.0))
                .len(),
            1
        );
    }

    #[test]
    fn snoozed_items_return_when_the_snooze_expires() {
        let session = permission("approve the deploy?", RiskHint::Neutral);
        let mut queue = InboxQueue::default();
        let key = queue.items([&session], &HashMap::new(), DateMillis(1_000.0))[0]
            .key
            .clone();
        queue.snooze(&key, DateMillis(2_000.0));

        assert!(queue.is_hidden(&key, DateMillis(1_999.0)));
        assert!(!queue.is_hidden(&key, DateMillis(2_000.0)));
        assert!(
            queue
                .items([&session], &HashMap::new(), DateMillis(1_999.0))
                .is_empty()
        );
        assert_eq!(
            queue
                .items([&session], &HashMap::new(), DateMillis(2_000.0))
                .len(),
            1
        );
        // Dismissing afterwards hides it permanently, not until some time.
        queue.dismiss(&key);
        assert!(queue.is_hidden(&key, DateMillis(10_000.0)));
    }

    #[test]
    fn restore_all_brings_cleared_items_back() {
        let session = permission("approve the deploy?", RiskHint::Neutral);
        let mut queue = InboxQueue::default();
        let key = queue.items([&session], &HashMap::new(), DateMillis(1_000.0))[0]
            .key
            .clone();
        queue.dismiss(&key);
        queue.snooze(
            &InboxItemKey::new(SessionId::new("other"), "blocker-x"),
            DateMillis(2_000.0),
        );
        assert_eq!(queue.hidden(), 2);
        assert_eq!(queue.restore_all(), 2);
        assert_eq!(queue.hidden(), 0);
        assert_eq!(
            queue
                .items([&session], &HashMap::new(), DateMillis(1_000.0))
                .len(),
            1
        );
    }

    #[test]
    fn decisions_for_removed_sessions_are_pruned() {
        let session = permission("approve the deploy?", RiskHint::Neutral);
        let mut queue = InboxQueue::default();
        let key = queue.items([&session], &HashMap::new(), DateMillis(1_000.0))[0]
            .key
            .clone();
        queue.dismiss(&key);
        queue.retain_sessions(&HashSet::from([SessionId::new("someone-else")]));
        assert_eq!(queue.hidden(), 0);
    }

    #[test]
    fn change_probes_without_evidence_are_not_changed() {
        assert_eq!(SessionChange::Unknown.changed_files(), None);
        assert_eq!(SessionChange::Unchanged.changed_files(), None);
        assert_eq!(SessionChange::Changed { files: 0 }.changed_files(), None);
        assert_eq!(SessionChange::Changed { files: 4 }.changed_files(), Some(4));
    }
}
