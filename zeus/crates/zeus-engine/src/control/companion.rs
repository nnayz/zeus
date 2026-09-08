//! Bounded projections and mutation fencing owned by the authoritative Engine.
//! Internal IPC entry points, never a generic external method forwarder.
use super::*;
use std::collections::HashMap;
use zeus_companion_api as api;

pub(super) struct Fence {
    pub epoch: String,
    // Never evict a consumed key: overflow fails closed until a new Engine epoch.
    used: HashMap<(String, String), Vec<u8>>,
}
impl Fence {
    pub fn new() -> Self {
        let mut nonce = [0_u8; 32];
        getrandom::fill(&mut nonce).expect("secure Engine epoch");
        Self {
            epoch: nonce.iter().map(|b| format!("{b:02x}")).collect(),
            used: HashMap::new(),
        }
    }
}
fn error(code: &str) -> ControlError {
    ControlError::new(code, code)
}

pub(crate) fn project(
    record: &zeus_proto::SessionRecord,
    live: Option<crate::session::SessionView>,
) -> api::Session {
    let status = live.as_ref().map(|v| &v.status).unwrap_or(&record.status);
    let status = match status {
        zeus_proto::SessionStatus::Starting => "starting",
        zeus_proto::SessionStatus::Working => "working",
        zeus_proto::SessionStatus::Idle => "idle",
        zeus_proto::SessionStatus::NeedsInput(_) => "needs_input",
        zeus_proto::SessionStatus::Exited(_) => "exited",
        zeus_proto::SessionStatus::Unknown => "unknown",
    };
    let clip = |s: &str| api::bounded_string(s, api::MAX_STRING);
    let mut projection = api::Session {
        id: clip(&record.id.0),
        project_id: clip(&record.project_id.0),
        kind: clip(record.effective_kind().id()),
        title: clip(&record.title),
        cwd: clip(&record.cwd),
        host: record.host.as_deref().map(clip),
        status: status.into(),
        created_at_ms: record.created_at.0,
        updated_at_ms: record.updated_at.0,
        archived: record.is_archived(),
        hibernated: record.hibernation.is_some(),
        revision: String::new(),
    };
    projection.revision = Sha256::digest(serde_json::to_vec(&projection).unwrap_or_default())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    projection
}

impl ControlServer {
    pub(super) fn companion_hello(&self) -> Result<JsonValue, ControlError> {
        encode(&api::Hello {
            server_id: String::new(),
            api_major: api::API_MAJOR,
            api_minor: api::API_MINOR,
            capabilities: api::CAPABILITIES.iter().map(|s| s.to_string()).collect(),
            engine_epoch: self.companion.lock().map_err(poisoned)?.epoch.clone(),
            max_body_bytes: api::MAX_BODY,
            max_response_bytes: api::MAX_RESPONSE,
        })
    }
    pub(super) fn companion_sessions(&self, params: Option<JsonValue>) -> Result<JsonValue, ControlError> {
        let p: api::PageRequest = decode(params)?;
        let limit = p.limit.unwrap_or(api::MAX_PAGE).clamp(1, api::MAX_PAGE);
        let epoch = self.companion.lock().map_err(poisoned)?.epoch.clone();
        let registry = self.registry.lock().map_err(poisoned)?;
        if p.offset > 8192 || registry.record_count() > 8192 {
            return Err(error("projection_limit"));
        }
        let items = registry.companion_page(p.offset, limit);
        let next = (p.offset.saturating_add(items.len()) < registry.record_count())
            .then_some(p.offset + items.len());
        encode(&api::Page {
            items,
            next_offset: next,
            engine_epoch: epoch,
        })
    }
    pub(super) fn companion_projects(&self, params: Option<JsonValue>) -> Result<JsonValue, ControlError> {
        let p: api::PageRequest = decode(params)?;
        let limit = p.limit.unwrap_or(api::MAX_PAGE).clamp(1, api::MAX_PAGE);
        let epoch = self.companion.lock().map_err(poisoned)?.epoch.clone();
        let registry = self.registry.lock().map_err(poisoned)?;
        let projects = registry.projects_raw();
        if p.offset > 8192 || projects.len() > 8192 {
            return Err(error("projection_limit"));
        }
        let items: Vec<_> = projects
            .iter()
            .skip(p.offset)
            .take(limit)
            .map(|p| {
                let field = |key| {
                    api::bounded_string(
                        p.get(key).and_then(Value::as_str).unwrap_or_default(),
                        api::MAX_STRING,
                    )
                };
                api::Project {
                    id: field("id"),
                    name: field("name"),
                    root: field("root"),
                    host: p
                        .get("host")
                        .and_then(Value::as_str)
                        .map(|s| api::bounded_string(s, api::MAX_STRING)),
                }
            })
            .collect();
        let next = (p.offset.saturating_add(items.len()) < projects.len())
            .then_some(p.offset + items.len());
        encode(&api::Page {
            items,
            next_offset: next,
            engine_epoch: epoch,
        })
    }
    pub(super) fn companion_session(&self, params: Option<JsonValue>) -> Result<JsonValue, ControlError> {
        let p: zeus_proto::SessionIdParams = decode(params)?;
        if !api::valid_id(&p.session_id.0) {
            return Err(error("invalid_request"));
        }
        let engine_epoch = self.companion.lock().map_err(poisoned)?.epoch.clone();
        let session = self
            .registry
            .lock()
            .map_err(poisoned)?
            .companion_record(&p.session_id.0)
            .ok_or_else(|| error("not_found"))?;
        encode(&api::SessionDetail {
            session,
            engine_epoch,
        })
    }
    pub(super) fn companion_mutate(&self, params: Option<JsonValue>) -> Result<JsonValue, ControlError> {
        #[derive(serde::Deserialize, serde::Serialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            session_id: String,
            device_id: String,
            mutation: api::Mutation,
        }
        let p: Params = decode(params)?;
        if !api::valid_id(&p.session_id)
            || !api::valid_id(&p.device_id)
            || !api::valid_id(&p.mutation.mutation_id)
            || p.mutation.mutation_id.len() < 16
        {
            return Err(error("invalid_request"));
        }
        match &p.mutation.action {
            api::Action::Rename { title }
                if title.is_empty()
                    || title.len() > api::MAX_STRING
                    || title.chars().any(char::is_control) =>
            {
                return Err(error("invalid_request"));
            }
            api::Action::Archive { confirmed }
            | api::Action::Wake { confirmed }
            | api::Action::Hibernate { confirmed }
            | api::Action::Terminate { confirmed }
                if !confirmed =>
            {
                return Err(error("confirmation_required"));
            }
            _ => {}
        }
        let mut fence = self.companion.try_lock().map_err(|_| error("busy"))?;
        if p.mutation.engine_epoch != fence.epoch {
            return Err(error("stale_engine"));
        }
        let digest =
            Sha256::digest(serde_json::to_vec(&p).map_err(|_| error("invalid_request"))?).to_vec();
        let key = (p.device_id.clone(), p.mutation.mutation_id.clone());
        if let Some(previous) = fence.used.get(&key) {
            return Err(error(if previous == &digest {
                "replayed_mutation"
            } else {
                "mutation_conflict"
            }));
        }
        if fence.used.len() >= 4096 {
            return Err(error("mutation_window_full"));
        }
        let mut registry = self.registry.lock().map_err(poisoned)?;
        let current = registry
            .companion_record(&p.session_id)
            .ok_or_else(|| error("not_found"))?;
        if current.revision != p.mutation.expected_revision {
            return Err(error("stale_revision"));
        }
        // Lifecycle is unavailable until integrated with the terminal controller fence.
        if !matches!(p.mutation.action, api::Action::Rename { .. }) {
            return Err(error("capability_unavailable"));
        }
        fence.used.insert(key, digest);
        if let api::Action::Rename { title } = &p.mutation.action {
            registry
                .rename(&p.session_id, title)
                .map_err(|_| error("mutation_failed"))?;
        }
        registry.persist().map_err(|_| error("outcome_unknown"))?;
        self.publish_updated(&registry, &p.session_id);
        encode(&api::MutationResult {
            mutation_id: p.mutation.mutation_id,
            applied: true,
        })
    }
}
