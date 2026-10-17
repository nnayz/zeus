//! One independent Engine-owned shell per inspector terminal tab.
//!
//! Closing or hiding a tab drops its attachment, preserving the shell. Ordinary
//! sidebar lifecycle actions remain the authority for ending that session.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{
    AnyElement, Context, Entity, EventEmitter, Render, Role, Task, Window, canvas, div, prelude::*,
    px,
};
use taskmaster_proto::{
    AgentKind, DateMillis, ProjectId, Resumability, SessionId, SessionRecord, SessionSpawnParams,
    SessionStatus, TitleSource,
};
use taskmaster_ui::SemanticColors;

use crate::store::{SessionStore, StoreRuntime};
use crate::terminal_pane::{TerminalPane, TerminalViewport};

static PREVIEW_TERMINAL_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Session events can precede the spawn response. Keep automatic center
/// selection suspended until the panel id and authoritative record are saved.
struct PanelLaunchGuard {
    runtime: Arc<StoreRuntime>,
}

impl PanelLaunchGuard {
    fn begin(runtime: Arc<StoreRuntime>) -> Self {
        runtime
            .store
            .write()
            .expect("session store lock poisoned")
            .begin_panel_launch();
        Self { runtime }
    }
}

impl Drop for PanelLaunchGuard {
    fn drop(&mut self) {
        if let Ok(mut store) = self.runtime.store.write() {
            store.end_panel_launch();
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PanelTerminalEvent {
    /// Persist this id with the panel tab so reopening the app reattaches it.
    Spawned(SessionId),
    /// Parent should select this session and focus the main terminal.
    OpenInMain(SessionId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LaunchContext {
    parent: Option<SessionId>,
    cwd: String,
    host: Option<String>,
    project_id: ProjectId,
}

impl LaunchContext {
    fn from_store(store: &SessionStore) -> Result<Self, String> {
        if let Some(session) = store.selected_session() {
            return Ok(Self {
                parent: Some(session.id.clone()),
                cwd: session.cwd.clone(),
                host: session.host.clone(),
                project_id: session.project_id.clone(),
            });
        }
        let project = store
            .active_workspace()
            .ok_or_else(|| "Choose a workspace before opening a terminal.".to_owned())?;
        // Location belongs to the project/session model; the local default
        // host must never turn a known remote project into a local directory.
        let host = store
            .sessions()
            .values()
            .find(|session| session.project_id == project.id)
            .and_then(|session| session.host.clone());
        Ok(Self {
            parent: None,
            cwd: project.root.clone(),
            host,
            project_id: project.id.clone(),
        })
    }

    fn spawn_params(&self) -> SessionSpawnParams {
        SessionSpawnParams {
            kind: AgentKind::SHELL,
            cwd: self.cwd.clone(),
            new_worktree: None,
            worktree_branch: None,
            title: Some("Terminal".to_owned()),
            initial_prompt: None,
            parent: self.parent.clone(),
            initial_cols: None,
            initial_rows: None,
            host: self.host.clone(),
            same_repo_as: None,
            // These are first-class shell sessions. They must not be mistaken
            // for the single ⌘J workbench terminal associated with a parent.
            workbench: Some(false),
        }
    }

    fn preview_record(&self, id: SessionId) -> SessionRecord {
        SessionRecord {
            id,
            kind: AgentKind::SHELL,
            cwd: self.cwd.clone(),
            project_id: self.project_id.clone(),
            worktree_path: None,
            git_branch: None,
            title: "Terminal".to_owned(),
            title_source: TitleSource::TaskmasterAssigned,
            agent_session_id: None,
            transcript_path: None,
            status: SessionStatus::Idle,
            needs_input: None,
            resumability: Resumability::Live,
            parent: self.parent.clone(),
            created_at: DateMillis(0.0),
            updated_at: DateMillis(0.0),
            last_turn_completed_at: None,
            last_seen_at: None,
            pinned: false,
            archived_at: None,
            host: self.host.clone(),
            remote_persistence: None,
            hibernation: None,
            memory_bytes: None,
            artifacts: None,
            pull_requests: None,
            listening_ports: None,
            foreground_agent: None,
            workbench: Some(false),
        }
    }
}

pub(crate) struct PanelTerminal {
    runtime: Arc<StoreRuntime>,
    tokio_owner: Arc<tokio::runtime::Runtime>,
    preview: bool,
    visible: bool,
    started: bool,
    loading: bool,
    error: Option<String>,
    session_id: Option<SessionId>,
    expected_project_id: Option<ProjectId>,
    record_seen: bool,
    pane: Option<Entity<TerminalPane>>,
    focus_pending: bool,
    bounds: Rc<Cell<Option<TerminalViewport>>>,
    launch: Option<Task<()>>,
    record_lookup: Option<Task<()>>,
    launch_guard: Option<PanelLaunchGuard>,
    _store_changes: Task<()>,
}

impl EventEmitter<PanelTerminalEvent> for PanelTerminal {}

impl PanelTerminal {
    #[cfg(test)]
    pub(crate) fn new(
        runtime: Arc<StoreRuntime>,
        tokio_owner: Arc<tokio::runtime::Runtime>,
        preview: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_existing(runtime, tokio_owner, preview, None, cx)
    }

    pub(crate) fn new_existing(
        runtime: Arc<StoreRuntime>,
        tokio_owner: Arc<tokio::runtime::Runtime>,
        preview: bool,
        session_id: Option<SessionId>,
        cx: &mut Context<Self>,
    ) -> Self {
        let expected_project_id = {
            let store = runtime.store.read().expect("session store lock poisoned");
            store
                .active_workspace_id()
                .cloned()
                .or_else(|| store.preferences().last_active_workspace.clone())
        };
        let mut changes = runtime.changes();
        let store_changes = cx.spawn(async move |this, cx| {
            loop {
                match changes.recv().await {
                    Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if this
                            .update(cx, |this, cx| {
                                this.reconcile_ownership(cx);
                                cx.notify();
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
        Self {
            runtime,
            tokio_owner,
            preview,
            visible: false,
            started: session_id.is_some(),
            loading: false,
            error: None,
            session_id,
            expected_project_id,
            record_seen: false,
            pane: None,
            focus_pending: false,
            bounds: Rc::new(Cell::new(None)),
            launch: None,
            record_lookup: None,
            launch_guard: None,
            _store_changes: store_changes,
        }
    }

    #[cfg(test)]
    pub(crate) fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    pub(crate) fn is_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.pane
            .as_ref()
            .is_some_and(|pane| pane.read(cx).is_focused(window))
    }

    /// Called only for the active inspector tab. Its initial execution context
    /// is captured once; changing the selected agent never repurposes its PTY.
    pub(crate) fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible && self.started {
            return;
        }
        if !self.visible {
            self.visible = true;
            self.focus_pending = true;
        }
        if !self.started {
            self.start(window, cx);
        } else if self.session_id.is_some() && !self.record_seen && self.error.is_none() {
            self.resolve_existing(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn deactivate(&mut self, cx: &mut Context<Self>) {
        if !self.visible && self.pane.is_none() {
            return;
        }
        self.visible = false;
        self.focus_pending = false;
        self.bounds.set(None);
        // Each hidden terminal preserves its Engine session while releasing
        // its desktop controller. No observer or duplicate attach is created.
        self.release_pane(cx);
        cx.notify();
    }

    pub(crate) fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = &self.pane {
            pane.update(cx, |pane, cx| pane.focus(window, cx));
        } else {
            self.focus_pending = true;
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let context = {
            let store = self
                .runtime
                .store
                .read()
                .expect("session store lock poisoned");
            LaunchContext::from_store(&store)
        };
        let context = match context {
            Ok(context) => context,
            Err(error) => {
                self.started = true;
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.started = true;
        self.loading = true;
        self.error = None;
        self.expected_project_id = Some(context.project_id.clone());
        let guard = PanelLaunchGuard::begin(Arc::clone(&self.runtime));
        if self.preview {
            let sequence = PREVIEW_TERMINAL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let id = SessionId::new(format!("preview-panel-terminal-{sequence}"));
            self.runtime
                .store
                .write()
                .expect("session store lock poisoned")
                .upsert_session(context.preview_record(id.clone()));
            self.session_id = Some(id.clone());
            self.loading = false;
            cx.emit(PanelTerminalEvent::Spawned(id));
            // Emit is processed before Deferred, so the parent's persistence
            // callback has recorded the tab id before automatic selection resumes.
            cx.defer(move |_| drop(guard));
            cx.notify();
            return;
        }
        let client = Arc::clone(self.runtime.client());
        let request = self.tokio_owner.spawn(async move {
            let result = client.spawn(context.spawn_params()).await;
            (result, guard)
        });
        self.launch = Some(cx.spawn_in(window, async move |this, cx| {
            let (result, guard) = match request.await {
                Ok((result, guard)) => (result.map_err(|error| error.to_string()), Some(guard)),
                Err(error) => (Err(format!("Terminal request stopped: {error}")), None),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.loading = false;
                this.launch_guard = guard;
                match result {
                    Ok(id) => {
                        this.session_id = Some(id.clone());
                        cx.emit(PanelTerminalEvent::Spawned(id));
                        this.resolve_existing(window, cx);
                    }
                    Err(error) => {
                        this.error = Some(error);
                        this.finish_launch_guard(cx);
                    }
                }
                cx.notify();
            });
        }));
    }

    /// A restored id, or a spawn response received before its event, needs an
    /// authoritative record. One bounded existing session.list request avoids
    /// guessing ids or selecting an unrelated shell while events arrive.
    fn resolve_existing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.session_id.clone() else {
            return;
        };
        if self.loading {
            return;
        }
        let existing = {
            let store = self
                .runtime
                .store
                .read()
                .expect("session store lock poisoned");
            store
                .sessions()
                .get(&id)
                .map(|record| self.record_error(record))
        };
        if let Some(error) = existing {
            self.error = error.map(str::to_owned);
            self.finish_launch_guard(cx);
            return;
        }
        if self.preview {
            self.error = Some("This terminal is not present in the preview.".to_owned());
            return;
        }
        self.error = None;
        self.loading = true;
        let client = Arc::clone(self.runtime.client());
        let request = self.tokio_owner.spawn(async move {
            client.sessions().await.map_err(|error| error.to_string())?
                .sessions.into_iter().find(|session| session.id == id)
                .ok_or_else(|| "This terminal is no longer available. Open another terminal from the panel menu.".to_owned())
        });
        self.record_lookup = Some(cx.spawn_in(window, async move |this, cx| {
            let result = request
                .await
                .map_err(|error| format!("Terminal lookup stopped: {error}"))
                .and_then(|result| result);
            let _ = this.update_in(cx, |this, _, cx| {
                this.loading = false;
                match result {
                    Ok(session) => {
                        if let Some(error) = this.record_error(&session) {
                            this.error = Some(error.to_owned());
                        } else {
                            let mut store = this
                                .runtime
                                .store
                                .write()
                                .expect("session store lock poisoned");
                            // An event may already have installed a newer record.
                            if !store.sessions().contains_key(&session.id) {
                                store.upsert_session(session);
                            }
                        }
                    }
                    Err(error) => this.error = Some(error),
                }
                this.finish_launch_guard(cx);
                cx.notify();
            });
        }));
    }

    fn main_owns_session(&self) -> bool {
        self.session_id.as_ref().is_some_and(|id| {
            let store = self
                .runtime
                .store
                .read()
                .expect("session store lock poisoned");
            store
                .sessions()
                .get(id)
                .is_some_and(|record| self.record_error(record).is_none())
                && store.terminal_residency().contains(id)
        })
    }

    fn record_error(&self, record: &SessionRecord) -> Option<&'static str> {
        if self.expected_project_id.as_ref() != Some(&record.project_id) {
            Some(
                "Saved terminal does not belong to this workspace. Open another terminal from the panel menu.",
            )
        } else if record.kind != AgentKind::SHELL || record.is_workbench_terminal() {
            Some(
                "Saved session is not an independent panel shell. Open another terminal from the panel menu.",
            )
        } else {
            None
        }
    }

    fn saved_identity_error(&self) -> Option<&'static str> {
        let id = self.session_id.as_ref()?;
        let store = self
            .runtime
            .store
            .read()
            .expect("session store lock poisoned");
        store
            .sessions()
            .get(id)
            .and_then(|record| self.record_error(record))
    }

    fn finish_launch_guard(&mut self, cx: &mut Context<Self>) {
        if let Some(guard) = self.launch_guard.take() {
            cx.defer(move |_| drop(guard));
        }
    }

    fn release_pane(&mut self, cx: &mut Context<Self>) {
        if let Some(pane) = self.pane.take() {
            pane.update(cx, |pane, _| pane.release_attachments());
        }
    }

    fn reconcile_ownership(&mut self, cx: &mut Context<Self>) {
        if self.main_owns_session() || self.saved_identity_error().is_some() {
            self.release_pane(cx);
        }
    }

    fn open_in_main(&mut self, cx: &mut Context<Self>) {
        if self.saved_identity_error().is_some() {
            self.release_pane(cx);
            return;
        }
        if let Some(id) = self.session_id.clone() {
            self.release_pane(cx);
            self.focus_pending = false;
            cx.emit(PanelTerminalEvent::OpenInMain(id));
            cx.notify();
        }
    }

    fn message(
        &self,
        title: &'static str,
        detail: impl Into<gpui::SharedString>,
        action: Option<&'static str>,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p(px(16.0))
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(colors.primary)
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(360.0))
                    .text_size(px(12.0))
                    .text_color(colors.secondary)
                    .child(detail.into()),
            )
            .when_some(action, |content, label| {
                content.child(
                    div()
                        .id("panel-terminal-action")
                        .debug_selector(|| "PANEL_TERMINAL_ACTION".to_owned())
                        .focusable()
                        .tab_stop(true)
                        .role(Role::Button)
                        .aria_label(label)
                        .h(px(32.0))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .rounded(px(8.0))
                        .bg(colors.primary.alpha(0.06))
                        .text_color(colors.primary)
                        .border_2()
                        .border_color(gpui::transparent_black())
                        .focus_visible(|style| style.border_color(colors.primary))
                        .hover(|style| style.bg(colors.primary.alpha(0.10)))
                        .cursor_pointer()
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if label == "Open in main terminal" {
                                this.open_in_main(cx);
                            } else if label == "Try again" && this.session_id.is_some() {
                                this.resolve_existing(window, cx);
                                cx.notify();
                            } else {
                                this.session_id = None;
                                this.record_seen = false;
                                this.release_pane(cx);
                                this.focus_pending = true;
                                this.start(window, cx);
                            }
                        })),
                )
            })
            .into_any_element()
    }
}

impl Render for PanelTerminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = {
            let store = self
                .runtime
                .store
                .read()
                .expect("session store lock poisoned");
            crate::app_theme::colors(&store.preferences().terminal_theme)
        };
        let root = div()
            .id("panel-terminal")
            .debug_selector(|| "PANEL_TERMINAL".to_owned())
            .relative()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(colors.background);
        if !self.visible {
            return root.into_any_element();
        }
        let bounds_slot = Rc::clone(&self.bounds);
        let this = cx.entity().downgrade();
        let root = root.child(
            canvas(
                move |bounds, _, cx| {
                    let viewport = TerminalViewport {
                        x: f32::from(bounds.origin.x),
                        y: f32::from(bounds.origin.y),
                        width: f32::from(bounds.size.width),
                        height: f32::from(bounds.size.height),
                    };
                    if bounds_slot.replace(Some(viewport)) != Some(viewport) {
                        let this = this.clone();
                        cx.defer(move |cx| {
                            let _ = this.update(cx, |_, cx| cx.notify());
                        });
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        );
        self.reconcile_ownership(cx);
        if let Some(error) = self.saved_identity_error() {
            return root
                .child(self.message("Terminal unavailable", error, None, colors, cx))
                .into_any_element();
        }
        if self.main_owns_session() {
            return root
                .child(self.message(
                    "Terminal is open in the main workspace",
                    "This shell continues in the main terminal.",
                    Some("Open in main terminal"),
                    colors,
                    cx,
                ))
                .into_any_element();
        }
        if self.loading {
            return root
                .child(self.message("Opening terminal…", "", None, colors, cx))
                .into_any_element();
        }
        if let Some(error) = &self.error {
            return root
                .child(self.message(
                    "Could not open terminal",
                    error.clone(),
                    Some("Try again"),
                    colors,
                    cx,
                ))
                .into_any_element();
        }
        let Some(id) = self.session_id.clone() else {
            return root
                .child(self.message(
                    "Terminal",
                    "Open a shell for this workspace.",
                    Some("Open terminal"),
                    colors,
                    cx,
                ))
                .into_any_element();
        };
        let record_exists = self
            .runtime
            .store
            .read()
            .expect("session store lock poisoned")
            .sessions()
            .contains_key(&id);
        if !record_exists {
            self.release_pane(cx);
            return root
                .child(self.message(
                    if self.record_seen {
                        "Terminal ended"
                    } else {
                        "Waiting for terminal…"
                    },
                    "",
                    self.record_seen.then_some("Open terminal"),
                    colors,
                    cx,
                ))
                .into_any_element();
        }
        self.record_seen = true;
        // Measure before constructing the child: its first resize must use
        // the panel allocation rather than the entire desktop window.
        let Some(bounds) = self.bounds.get() else {
            return root
                .child(self.message("Opening terminal…", "", None, colors, cx))
                .into_any_element();
        };
        if self.pane.is_none() {
            let runtime = Arc::clone(&self.runtime);
            let tokio_owner = Arc::clone(&self.tokio_owner);
            let preview = self.preview;
            self.pane = Some(cx.new(|cx| {
                if preview {
                    TerminalPane::new_fixed_preview(runtime, tokio_owner, id, window, cx)
                } else {
                    TerminalPane::new_fixed(runtime, tokio_owner, id, window, cx)
                }
            }));
        }
        let pane = self.pane.as_ref().expect("pane was initialized").clone();
        pane.update(cx, |pane, cx| pane.set_viewport(bounds, cx));
        if self.focus_pending {
            self.focus_pending = false;
            cx.defer_in(window, |this, window, cx| {
                if this.visible
                    && let Some(pane) = &this.pane
                {
                    pane.update(cx, |pane, cx| pane.focus(window, cx));
                }
            });
        }
        root.child(pane).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, size};
    use taskmaster_proto::{Project, SessionListResult};

    use crate::panel_tabs::{PanelTabKind, PanelTabs};
    use crate::store::Prefs;

    use super::*;

    fn fixture() -> SessionStore {
        let mut store =
            crate::sidebar::SidebarPreviewFixture::make(crate::sidebar::PreviewScenario::Typical)
                .into_store();
        store.select(SessionId::new("preview-codex"));
        store
    }

    #[test]
    fn panel_shell_inherits_remote_parent_without_becoming_the_workbench_split() {
        let mut store = fixture();
        let mut parent = store.selected_session().unwrap().clone();
        parent.cwd = "/srv/project/worktree with 'quotes' and $chars".to_owned();
        parent.host = Some("remote-fixture".to_owned());
        store.upsert_session(parent.clone());
        let params = LaunchContext::from_store(&store).unwrap().spawn_params();
        assert_eq!(params.kind, AgentKind::SHELL);
        assert_eq!(params.cwd, parent.cwd);
        assert_eq!(params.host, parent.host);
        assert_eq!(params.parent, Some(parent.id));
        assert_eq!(params.workbench, Some(false));
        assert!(params.new_worktree.is_none());
        assert!(params.initial_prompt.is_none());
    }

    #[gpui::test]
    fn restored_terminal_from_another_workspace_never_attaches_or_opens_in_main(
        cx: &mut TestAppContext,
    ) {
        let mut store = fixture();
        let expected_project = store.active_workspace_id().unwrap().clone();
        let mut foreign_context = LaunchContext::from_store(&store).unwrap();
        foreign_context.parent = None;
        foreign_context.project_id = ProjectId::new("another-workspace");
        foreign_context.cwd = "/work/another-workspace".to_owned();
        let id = SessionId::new("foreign-panel-terminal");
        store.upsert_session(foreign_context.preview_record(id.clone()));
        let runtime = Arc::new(StoreRuntime::preview_from(store));
        let view_runtime = Arc::clone(&runtime);
        let tokio_owner = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let mut panel =
                PanelTerminal::new_existing(view_runtime, tokio_owner, true, Some(id), cx);
            panel.activate(window, cx);
            panel
        });
        cx.simulate_resize(size(px(420.0), px(580.0)));
        assert_eq!(
            panel.read_with(cx, |panel, _| panel.expected_project_id.clone()),
            Some(expected_project)
        );
        assert!(panel.read_with(cx, |panel, _| panel.pane.is_none()));
        assert!(panel.read_with(cx, |panel, _| panel.saved_identity_error().is_some()));
        assert!(cx.debug_bounds("TERMINAL_SURFACE").is_none());
        assert!(cx.debug_bounds("PANEL_TERMINAL_ACTION").is_none());
        let opens = Arc::new(AtomicU64::new(0));
        let observed_opens = Arc::clone(&opens);
        let _events = cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event, _| {
                if matches!(event, PanelTerminalEvent::OpenInMain(_)) {
                    observed_opens.fetch_add(1, Ordering::Relaxed);
                }
            })
        });
        panel.update(cx, |panel, cx| panel.open_in_main(cx));
        assert_eq!(opens.load(Ordering::Relaxed), 0);
        assert_eq!(
            runtime.store.read().unwrap().selected_session_id(),
            Some(&SessionId::new("preview-codex"))
        );
        assert!(
            runtime
                .store
                .read()
                .unwrap()
                .sessions()
                .contains_key(&SessionId::new("foreign-panel-terminal"))
        );
    }

    #[gpui::test]
    fn empty_workspace_panel_shell_stays_out_of_center_until_explicitly_selected(
        cx: &mut TestAppContext,
    ) {
        let project = Project {
            id: ProjectId::new("empty"),
            root: "/work/empty".to_owned(),
            name: "Empty workspace".to_owned(),
            pinned_order: None,
        };
        let (mut store, _) = SessionStore::headless(Prefs::default());
        store.hydrate(SessionListResult {
            sessions: Vec::new(),
            projects: vec![project.clone()],
        });
        store.set_active_workspace(project.id.clone());
        let runtime = Arc::new(StoreRuntime::preview_from(store));
        let tokio_owner = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let view_runtime = Arc::clone(&runtime);
        let (panel, cx) = cx
            .add_window_view(move |_, cx| PanelTerminal::new(view_runtime, tokio_owner, true, cx));
        let persistence_runtime = Arc::clone(&runtime);
        let workspace = project.id.clone();
        let _persistence = cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event, _| {
                if let PanelTerminalEvent::Spawned(id) = event {
                    let mut tabs = PanelTabs::new(PanelTabKind::Terminal);
                    tabs.tabs[0].session_id = Some(id.clone());
                    persistence_runtime
                        .store
                        .write()
                        .unwrap()
                        .update_preferences(|prefs| {
                            prefs.workspace_panel_tabs.insert(workspace.clone(), tabs);
                        })
                        .unwrap();
                }
            })
        });
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.activate(window, cx)));
        cx.simulate_resize(size(px(420.0), px(580.0)));
        let id = panel.read_with(cx, |panel, _| panel.session_id().unwrap().clone());
        let mut updated = {
            let store = runtime.store.read().unwrap();
            assert!(store.selected_session_id().is_none());
            assert!(!store.terminal_residency().contains(&id));
            assert_eq!(
                store.preferences().workspace_panel_tabs[&project.id].tabs[0]
                    .session_id
                    .as_ref(),
                Some(&id)
            );
            store.sessions()[&id].as_ref().clone()
        };
        assert!(panel.read_with(cx, |panel, _| panel.pane.is_some()));
        assert!(cx.update(|window, cx| panel.read(cx).is_focused(window, cx)));

        updated.updated_at = DateMillis(1.0);
        runtime
            .store
            .write()
            .unwrap()
            .upsert_session(updated.clone());
        assert!(
            runtime
                .store
                .read()
                .unwrap()
                .selected_session_id()
                .is_none()
        );
        // Full Engine resynchronization must also respect the saved panel id
        // after the pending-launch guard has already been released.
        runtime.store.write().unwrap().hydrate(SessionListResult {
            sessions: vec![updated],
            projects: vec![project],
        });
        assert!(
            runtime
                .store
                .read()
                .unwrap()
                .selected_session_id()
                .is_none()
        );

        runtime.store.write().unwrap().select(id);
        panel.update(cx, |_, cx| cx.notify());
        assert!(cx.debug_bounds("TERMINAL_SURFACE").is_none());
        assert!(panel.read_with(cx, |panel, _| panel.main_owns_session()));
        assert!(panel.read_with(cx, |panel, _| panel.pane.is_none()));
        assert!(!cx.update(|window, cx| panel.read(cx).is_focused(window, cx)));
    }

    #[gpui::test]
    fn panel_terminals_have_independent_sessions_and_release_duplicate_ownership(
        cx: &mut TestAppContext,
    ) {
        let runtime = Arc::new(StoreRuntime::preview_from(fixture()));
        let tokio_owner = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let view_runtime = Arc::clone(&runtime);
        let view_tokio = Arc::clone(&tokio_owner);
        let (panel, cx) = cx.add_window_view(move |window, cx| {
            let mut panel = PanelTerminal::new(view_runtime, view_tokio, true, cx);
            panel.activate(window, cx);
            panel
        });
        cx.simulate_resize(size(px(420.0), px(580.0)));
        let id = panel.read_with(cx, |panel, _| panel.session_id().unwrap().clone());
        {
            let store = runtime.store.read().unwrap();
            assert_eq!(
                store.selected_session_id(),
                Some(&SessionId::new("preview-codex"))
            );
            let session = store.sessions().get(&id).unwrap();
            assert_eq!(session.kind, AgentKind::SHELL);
            assert_eq!(session.parent, Some(SessionId::new("preview-codex")));
            assert!(!session.is_workbench_terminal());
            assert!(!store.terminal_residency().contains(&id));
        }
        let surface = cx.debug_bounds("PANEL_TERMINAL").unwrap();
        let measured = panel.read_with(cx, |panel, _| panel.bounds.get().unwrap());
        assert_eq!(measured.width, f32::from(surface.size.width));
        assert_eq!(measured.height, f32::from(surface.size.height));
        assert_eq!(measured.x, f32::from(surface.origin.x));
        assert_eq!(measured.y, f32::from(surface.origin.y));
        assert!(panel.read_with(cx, |panel, _| panel.pane.is_some()));

        let second = cx.update(|window, cx| {
            cx.new(|cx| {
                let mut panel =
                    PanelTerminal::new(Arc::clone(&runtime), Arc::clone(&tokio_owner), true, cx);
                panel.activate(window, cx);
                panel
            })
        });
        let second_id = second.read_with(cx, |panel, _| panel.session_id().unwrap().clone());
        assert_ne!(id, second_id);
        assert!(
            runtime
                .store
                .read()
                .unwrap()
                .auxiliary_terminal_for(&SessionId::new("preview-codex"))
                .is_none()
        );
        drop(second);
        assert!(
            runtime
                .store
                .read()
                .unwrap()
                .sessions()
                .contains_key(&second_id)
        );

        let retained_pane = panel.read_with(cx, |panel, _| panel.pane.as_ref().unwrap().clone());
        panel.update(cx, |panel, cx| panel.deactivate(cx));
        assert!(panel.read_with(cx, |panel, _| panel.pane.is_none()));
        retained_pane.update(cx, |pane, _| {
            assert!(
                pane.resident_buffers().is_empty(),
                "a prior frame retaining the discarded pane must not reattach it"
            );
        });
        assert!(runtime.store.read().unwrap().sessions().contains_key(&id));
        runtime.store.write().unwrap().select(id.clone());
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.activate(window, cx)));
        assert!(panel.read_with(cx, |panel, _| panel.main_owns_session()));
        assert!(panel.read_with(cx, |panel, _| panel.pane.is_none()));
        assert!(cx.debug_bounds("TERMINAL_SURFACE").is_none());
        assert!(cx.debug_bounds("PANEL_TERMINAL_ACTION").is_some());
    }
}
