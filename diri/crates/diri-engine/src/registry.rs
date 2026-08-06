//! The set of live sessions, and their persisted records.
//!
//! The registry is what a control channel talks to: spawn, list, write, kill.
//! It also owns persistence, in the same `state.json` shape the Swift daemon
//! writes — `{ version, projects, sessions }` — so the two engines can read
//! each other's state file and a switch does not lose anybody's session list.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use diri_proto::{DateMillis, SessionRecord, SessionStatus, TitleSource};
use serde::{Deserialize, Serialize};

use crate::detect::ManifestEngine;
use crate::holder::{HolderClient, HolderManagerPaths, HolderPaths};
use crate::session::{HolderConfig, Session, SessionSpec, SessionView};

/// The on-disk snapshot. Field names and the version match the Swift
/// `PersistedState` exactly.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct PersistedState {
    pub version: i64,
    #[serde(default)]
    pub projects: Vec<serde_json::Value>,
    #[serde(default)]
    pub sessions: Vec<SessionRecord>,
}

impl PersistedState {
    fn current(sessions: Vec<SessionRecord>, projects: Vec<serde_json::Value>) -> Self {
        Self {
            version: 1,
            projects,
            sessions,
        }
    }
}

pub struct Registry {
    engine: Arc<ManifestEngine>,
    sessions: HashMap<String, Session>,
    /// Records for sessions that are no longer live but still listed.
    records: HashMap<String, SessionRecord>,
    /// Carried through untouched: this engine has no project model yet, and
    /// dropping the key would erase the Swift daemon's projects on first write.
    projects: Vec<serde_json::Value>,
    state_file: PathBuf,
}

impl Registry {
    pub fn new(engine: Arc<ManifestEngine>, state_file: impl Into<PathBuf>) -> Self {
        Self {
            engine,
            sessions: HashMap::new(),
            records: HashMap::new(),
            projects: Vec::new(),
            state_file: state_file.into(),
        }
    }

    /// Loads a persisted state file.
    ///
    /// A file that exists but will not parse is quarantined rather than
    /// ignored: treating it as a fresh install would make the next write
    /// overwrite every session record the user had.
    pub fn load(&mut self) -> std::io::Result<usize> {
        let bytes = match std::fs::read(&self.state_file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error),
        };
        match serde_json::from_slice::<PersistedState>(&bytes) {
            Ok(state) => {
                self.projects = state.projects;
                for record in state.sessions {
                    self.records.insert(record.id.0.clone(), record);
                }
                Ok(self.records.len())
            }
            Err(error) => {
                let quarantine = self.state_file.with_extension("json.corrupt");
                let _ = std::fs::rename(&self.state_file, &quarantine);
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "state file did not parse ({error}); quarantined at {}",
                        quarantine.display()
                    ),
                ))
            }
        }
    }

    /// Writes the current state atomically.
    pub fn persist(&self) -> std::io::Result<()> {
        let state = PersistedState::current(self.records(), self.projects.clone());
        let bytes = serde_json::to_vec_pretty(&state)?;
        let temp = self.state_file.with_extension("json.tmp");
        if let Some(parent) = self.state_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&temp, &bytes)?;
        // Rename is atomic, so a crash mid-write cannot truncate the real file.
        std::fs::rename(&temp, &self.state_file)?;
        Ok(())
    }

    /// Adds (or replaces) a record without a live session — restores,
    /// imports, and tests use this; live sessions come from [`spawn`].
    ///
    /// [`spawn`]: Registry::spawn
    pub fn insert_record(&mut self, record: SessionRecord) {
        self.records.insert(record.id.0.clone(), record);
    }

    /// Starts a session and takes ownership of it.
    pub fn spawn(&mut self, spec: SessionSpec, record: SessionRecord) -> std::io::Result<String> {
        let id = spec.id.clone();
        let session = Session::spawn(spec, Arc::clone(&self.engine))?;
        self.records.insert(id.clone(), record);
        self.sessions.insert(id.clone(), session);
        Ok(id)
    }

    /// Adopts every still-live holder-owned session found under
    /// `holder.holders_dir` that has a persisted record. Call after [`load`]:
    /// this is what makes sessions survive a daemon restart — or the switch
    /// from the Swift daemon to this one.
    ///
    /// Returns the ids adopted. Sessions whose holder is gone are left as
    /// records only, exactly as [`load`] left them.
    ///
    /// [`load`]: Registry::load
    pub fn restore(&mut self, holder: &HolderConfig, logs_dir: &Path) -> Vec<String> {
        let holders_dir = HolderPaths::new(&holder.holders_dir, "probe").directory;
        let Ok(entries) = std::fs::read_dir(&holders_dir) else {
            return Vec::new();
        };
        let holder_session_ids: Vec<String> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().is_some_and(|extension| extension == "sock")
                    && !HolderManagerPaths::is_manager_socket(path)
            })
            .filter_map(|path| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_string)
            })
            .collect();

        let mut adopted = Vec::new();
        for session_id in holder_session_ids {
            let Some(record) = self.records.get(&session_id) else {
                continue; // a holder without a record is not ours to run
            };
            if self.sessions.contains_key(&session_id) {
                continue;
            }
            let paths = HolderPaths::new(&holder.holders_dir, &session_id);
            let client = HolderClient::new(paths.socket());
            let Ok(stat) = client.stat() else { continue };
            if !stat.alive {
                continue;
            }
            let manifest_id = record.kind.id().to_string();
            let spec = SessionSpec {
                id: session_id.clone(),
                // The holder owns the real spec; this one only shapes the
                // emulator until stat's dimensions overwrite it in `adopt`.
                pty: crate::pty::PtySpec::new(Vec::new(), record.cwd.clone()),
                manifest_id: manifest_id.clone(),
                authority: crate::session::authority_for(&manifest_id, &self.engine),
                logs_dir: logs_dir.to_path_buf(),
                holder: Some(holder.clone()),
            };
            match Session::adopt(spec, holder, &stat, Arc::clone(&self.engine)) {
                Ok(session) => {
                    self.sessions.insert(session_id.clone(), session);
                    adopted.push(session_id);
                }
                Err(_) => continue,
            }
        }
        adopted
    }

    /// The manifest engine these sessions were started with.
    pub fn engine(&self) -> Arc<ManifestEngine> {
        Arc::clone(&self.engine)
    }

    pub fn get(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }

    pub fn views(&self) -> Vec<SessionView> {
        let mut views: Vec<_> = self.sessions.values().map(Session::view).collect();
        views.sort_by(|a, b| a.id.cmp(&b.id));
        views
    }

    /// Session records with live status folded in.
    pub fn records(&self) -> Vec<SessionRecord> {
        let mut records: Vec<SessionRecord> = self.records.values().cloned().collect();
        for record in &mut records {
            if let Some(session) = self.sessions.get(&record.id.0) {
                let view = session.view();
                record.status = view.status;
                record.needs_input = view.needs_input;
            }
        }
        records.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        records
    }

    /// Ends a session but keeps its record, which is what archiving means here.
    pub fn terminate(
        &mut self,
        id: &str,
        grace: std::time::Duration,
    ) -> std::io::Result<Option<crate::pty::Exit>> {
        let Some(mut session) = self.sessions.remove(id) else {
            return Ok(None);
        };
        let exit = session.terminate(grace)?;
        if let Some(record) = self.records.get_mut(id) {
            record.status = SessionStatus::Exited(diri_proto::ExitInfo {
                reason: match exit {
                    crate::pty::Exit::Signal(_) => diri_proto::ExitReason::Signaled,
                    crate::pty::Exit::Code(_) => diri_proto::ExitReason::Exited,
                },
                code: match exit {
                    crate::pty::Exit::Code(code) => Some(code),
                    crate::pty::Exit::Signal(_) => None,
                },
                signal: match exit {
                    crate::pty::Exit::Signal(signal) => Some(signal),
                    crate::pty::Exit::Code(_) => None,
                },
            });
        }
        Ok(Some(exit))
    }

    /// Drops a record entirely — the session is gone and not coming back.
    pub fn forget(&mut self, id: &str) {
        self.sessions.remove(id);
        self.records.remove(id);
    }

    /// Ends the session (if live), deletes its record AND its output log.
    /// This is the user closing a tab for good, not archiving.
    pub fn remove(&mut self, id: &str, logs_dir: &Path) -> std::io::Result<()> {
        if self.sessions.contains_key(id) {
            let _ = self.terminate(id, std::time::Duration::from_millis(500));
        }
        if self.records.remove(id).is_none() {
            return Err(not_found(id));
        }
        self.sessions.remove(id);
        let _ = std::fs::remove_file(logs_dir.join(format!("{id}.bin")));
        Ok(())
    }

    pub fn rename(&mut self, id: &str, title: &str) -> std::io::Result<()> {
        let record = self.records.get_mut(id).ok_or_else(|| not_found(id))?;
        record.title = title.to_string();
        record.title_source = TitleSource::UserRename;
        record.updated_at = DateMillis::from(std::time::SystemTime::now());
        Ok(())
    }

    pub fn mark_seen(&mut self, id: &str) -> std::io::Result<()> {
        let record = self.records.get_mut(id).ok_or_else(|| not_found(id))?;
        record.last_seen_at = Some(DateMillis::from(std::time::SystemTime::now()));
        Ok(())
    }

    /// Ends the session but keeps its record on the shelf: kill-tree,
    /// keep-record, stamp `archivedAt`.
    pub fn archive(&mut self, id: &str) -> std::io::Result<()> {
        if !self.records.contains_key(id) {
            return Err(not_found(id));
        }
        if self.sessions.contains_key(id) {
            let _ = self.terminate(id, std::time::Duration::from_millis(500));
        }
        let record = self.records.get_mut(id).expect("checked above");
        record.archived_at = Some(DateMillis::from(std::time::SystemTime::now()));
        if !matches!(record.status, SessionStatus::Exited(_)) {
            record.status = SessionStatus::Exited(diri_proto::ExitInfo {
                reason: diri_proto::ExitReason::Archived,
                code: None,
                signal: None,
            });
        }
        record.needs_input = None;
        Ok(())
    }

    pub fn unarchive(&mut self, id: &str) -> std::io::Result<()> {
        let record = self.records.get_mut(id).ok_or_else(|| not_found(id))?;
        if record.archived_at.is_none() {
            return Ok(());
        }
        record.archived_at = None;
        record.updated_at = DateMillis::from(std::time::SystemTime::now());
        Ok(())
    }

    /// Agent-side conversation ids already represented here, so a history
    /// scan can exclude conversations that are live sessions.
    pub fn tracked_agent_session_ids(&self) -> Vec<String> {
        self.records
            .values()
            .filter_map(|record| record.agent_session_id.clone())
            .collect()
    }

    /// The project list, verbatim as loaded — this engine does not model
    /// projects yet, but the list response carries them.
    pub fn projects_raw(&self) -> &[serde_json::Value] {
        &self.projects
    }

    pub fn live_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    pub fn state_file(&self) -> &Path {
        &self.state_file
    }
}

fn not_found(id: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::NotFound, format!("no session {id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use diri_proto::{AgentKind, DateMillis, ProjectId, Resumability, SessionId, TitleSource};

    fn record(id: &str) -> SessionRecord {
        SessionRecord {
            id: SessionId(id.into()),
            kind: AgentKind::SHELL,
            cwd: "/tmp".into(),
            project_id: ProjectId("p".into()),
            worktree_path: None,
            git_branch: None,
            title: "test".into(),
            title_source: TitleSource::Placeholder,
            agent_session_id: None,
            transcript_path: None,
            status: SessionStatus::Starting,
            needs_input: None,
            resumability: Resumability::NotResumable,
            parent: None,
            created_at: DateMillis(0.0),
            updated_at: DateMillis(0.0),
            last_turn_completed_at: None,
            last_seen_at: None,
            pinned: false,
            archived_at: None,
            remote_active: false,
            host: None,
            hibernation: None,
            memory_bytes: None,
            artifacts: None,
            pull_requests: None,
            listening_ports: None,
            foreground_agent: None,
        }
    }

    fn engine() -> Arc<ManifestEngine> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../Sources/DirijorCore/Resources/manifests")
            .canonicalize()
            .expect("manifests");
        let (engine, _) = ManifestEngine::load_dir(&dir).expect("load");
        Arc::new(engine)
    }

    #[test]
    fn state_round_trips_through_the_swift_file_shape() {
        let temp = tempfile::tempdir().expect("temp");
        let state_file = temp.path().join("state.json");

        let mut registry = Registry::new(engine(), &state_file);
        registry.records.insert("s_1".into(), record("s_1"));
        registry.persist().expect("persist");

        // The shape on disk is what the Swift daemon expects.
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&state_file).expect("read")).expect("parse");
        assert_eq!(raw["version"], 1);
        assert!(raw["sessions"].is_array());
        assert!(raw["projects"].is_array());
        assert_eq!(raw["sessions"][0]["id"], "s_1");

        let mut reloaded = Registry::new(engine(), &state_file);
        assert_eq!(reloaded.load().expect("load"), 1);
        assert_eq!(reloaded.records()[0].id.0, "s_1");
    }

    /// Interop against the state file the Swift daemon actually maintains.
    ///
    /// Ignored by default because it needs a real one. Point
    /// `DIRI_INTEROP_STATE` at a **copy** — never at the live file, which the
    /// running daemon rewrites:
    ///
    /// ```sh
    /// cp "~/Library/Application Support/Dirijor/state.json" /tmp/state.json
    /// DIRI_INTEROP_STATE=/tmp/state.json cargo test -p diri-engine -- --ignored
    /// ```
    #[test]
    #[ignore = "needs DIRI_INTEROP_STATE pointing at a copy of a Swift-written state.json"]
    fn reads_the_state_file_the_swift_daemon_wrote() {
        let Ok(raw) = std::env::var("DIRI_INTEROP_STATE") else {
            eprintln!("skipped: DIRI_INTEROP_STATE is not set");
            return;
        };
        let path = PathBuf::from(raw);
        let original: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("parse");
        let session_count = original["sessions"].as_array().map_or(0, Vec::len);
        let project_count = original["projects"].as_array().map_or(0, Vec::len);
        assert!(session_count > 0, "pick a state file with sessions in it");

        let temp = tempfile::tempdir().expect("temp");
        let working = temp.path().join("state.json");
        std::fs::copy(&path, &working).expect("copy");

        let mut registry = Registry::new(engine(), &working);
        assert_eq!(
            registry.load().expect("the real state file must parse"),
            session_count,
            "every session record should survive the round trip"
        );

        // Writing it back must not lose anything the Swift daemon owns.
        registry.persist().expect("persist");
        let rewritten: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&working).expect("read")).expect("parse");
        assert_eq!(rewritten["version"], 1);
        assert_eq!(
            rewritten["projects"].as_array().map_or(0, Vec::len),
            project_count,
            "projects this engine does not model must be carried through"
        );
        assert_eq!(
            rewritten["sessions"].as_array().map_or(0, Vec::len),
            session_count
        );
    }

    #[test]
    fn a_missing_state_file_is_a_fresh_start_not_an_error() {
        let temp = tempfile::tempdir().expect("temp");
        let mut registry = Registry::new(engine(), temp.path().join("absent.json"));
        assert_eq!(registry.load().expect("load"), 0);
    }

    #[test]
    fn an_unparseable_state_file_is_quarantined_rather_than_overwritten() {
        // Treating a corrupt file as a fresh install would erase every session
        // record on the next write.
        let temp = tempfile::tempdir().expect("temp");
        let state_file = temp.path().join("state.json");
        std::fs::write(&state_file, b"{ not json").expect("write");

        let mut registry = Registry::new(engine(), &state_file);
        let error = registry.load().expect_err("corrupt state must be an error");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);

        assert!(
            temp.path().join("state.json.corrupt").exists(),
            "the unreadable file should still be recoverable by hand"
        );
    }

    #[test]
    fn unknown_projects_survive_a_write() {
        // This engine has no project model yet. Dropping the key would erase
        // the Swift daemon's projects the first time the Rust one persisted.
        let temp = tempfile::tempdir().expect("temp");
        let state_file = temp.path().join("state.json");
        std::fs::write(
            &state_file,
            br#"{"version":1,"projects":[{"id":"p1","name":"keep me"}],"sessions":[]}"#,
        )
        .expect("write");

        let mut registry = Registry::new(engine(), &state_file);
        registry.load().expect("load");
        registry.persist().expect("persist");

        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&state_file).expect("read")).expect("parse");
        assert_eq!(raw["projects"][0]["name"], "keep me");
    }
}
