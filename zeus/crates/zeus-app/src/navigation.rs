use std::cmp::Ordering;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use crate::code_intelligence::{CodeIntelligence, SearchHit};
use crate::fuzzy::{FuzzyMatcher, FuzzyQuery};
use crate::macos::sf_symbols::{SymbolWeight, sf_symbol, sf_symbol_weighted};
use crate::palette::{self, PaletteAction, PaletteCommand, Ranked};
use crate::query_editor::{self, ClipboardEdit, Edit, QueryEditor};
use crate::quick_open::{
    self, DirectoryIndex, QuickOpenItem, QuickOpenSnapshot, RANK_DEBOUNCE, RESULT_LIMIT,
    RankedFolder,
};
use crate::store::{SessionStore, SpawnOptions, StoreRuntime};
use gpui::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle,
    KeyDownEvent, MouseButton, Pixels, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, StyledText, Task, Window, actions, div, prelude::*, px, rgba,
};
use zeus_proto::{AgentKind, AttentionLevel, SessionId, SessionRecord};
use zeus_ui::{FloatingSurface, HairlineDivider, Palette, Radius, SemanticColors};

actions!(zeus, [ToggleCommandPalette, ToggleQuickOpen]);

/// The command palette stays compact; Quick Open gets the roomier dialog
/// treatment used by file palettes in editors.
const SEARCH_HEIGHT: f32 = 34.0;
const ROW_HEIGHT: f32 = 24.0;
const QUICK_SEARCH_HEIGHT: f32 = 48.0;
const QUICK_FOOTER_HEIGHT: f32 = 34.0;
const QUICK_ROW_HEIGHT: f32 = 38.0;
/// Quick Open rows that show a parent path stack two lines.
const QUICK_ROW_HEIGHT_WITH_PATH: f32 = 42.0;
const SECTION_HEADER_HEIGHT: f32 = 24.0;
const LIST_PADDING_X: f32 = 6.0;
const LIST_PADDING_Y: f32 = 6.0;
const ROW_PADDING_X: f32 = 10.0;
const SURFACE_WIDTH: f32 = 720.0;
const MIN_LIST_HEIGHT: f32 = 96.0;
const MAX_LIST_HEIGHT: f32 = 520.0;
const MIN_TOP_INSET: f32 = 12.0;
const MAX_TOP_INSET: f32 = 104.0;
const BOTTOM_INSET: f32 = 16.0;
const FILE_RESULT_LIMIT: usize = 32;

/// Where the overlay sits and how tall its list may grow in this window.
#[derive(Clone, Copy, Debug, PartialEq)]
struct OverlayLayout {
    top_inset: Pixels,
    width: Pixels,
    list_height: Pixels,
}

impl OverlayLayout {
    fn for_viewport(viewport: gpui::Size<Pixels>) -> Self {
        let height = viewport.height.as_f32();
        // Reserve the larger Quick Open chrome for both overlays. That keeps
        // the layout safe when Cmd+K transitions into Quick Open in place.
        let chrome = QUICK_SEARCH_HEIGHT + QUICK_FOOTER_HEIGHT + 2.0 + BOTTOM_INSET;
        // Float the surface a twelfth of the way down, but give the inset back
        // to the list before the list is allowed to fall below its minimum.
        let top = (height / 12.0)
            .clamp(MIN_TOP_INSET, MAX_TOP_INSET)
            .min((height - chrome - MIN_LIST_HEIGHT).max(MIN_TOP_INSET));
        let list = (height - top - chrome).clamp(MIN_LIST_HEIGHT, MAX_LIST_HEIGHT);
        Self {
            top_inset: px(top),
            width: px((viewport.width.as_f32() - 2.0 * BOTTOM_INSET).clamp(280.0, SURFACE_WIDTH)),
            list_height: px(list),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Overlay {
    CommandPalette,
    QuickOpen,
}

#[derive(Clone)]
enum CommandSelection {
    Action(PaletteCommand),
    Session(SessionId),
}

#[derive(Clone)]
enum QuickSelection {
    File { cwd: PathBuf, reference: String },
    Session(SessionId),
    Folder(QuickOpenItem),
}

#[derive(Clone, Debug)]
pub enum NavigationEvent {
    ToggleSidebar,
    OpenOverview,
    OpenWorktrees,
    OpenBranches,
    GoToPullRequest,
    OpenSettings,
    CheckForUpdates,
    OpenFile { cwd: PathBuf, reference: String },
}

pub struct NavigationOverlay {
    focus_handle: FocusHandle,
    store: Arc<RwLock<SessionStore>>,
    _runtime: Arc<StoreRuntime>,
    overlay: Option<Overlay>,
    query: QueryEditor,
    highlight: usize,
    /// Ranked once per keystroke, then read by hit-testing, keyboard
    /// navigation, and rendering alike — they must agree on what row 3 is.
    ranked_actions: Vec<Ranked<PaletteAction>>,
    ranked_sessions: Vec<Ranked<SessionRecord>>,
    matcher: FuzzyMatcher,
    directory_index: DirectoryIndex,
    quick_snapshot: QuickOpenSnapshot,
    ranked_items: Vec<RankedFolder>,
    ranked_files: Vec<SearchHit>,
    file_workspace: Option<PathBuf>,
    file_root: Option<PathBuf>,
    file_intelligence: Option<Arc<CodeIntelligence>>,
    file_index_message: Option<SharedString>,
    file_rank_generation: u64,
    scroll_handle: ScrollHandle,
    /// Separate slots: the disk-cache load and the filesystem scan both start
    /// at launch, and neither may cancel the other by sharing a `Task` slot.
    cache_task: Option<Task<()>>,
    scan_task: Option<Task<()>>,
    rank_task: Option<Task<()>>,
    file_index_task: Option<Task<()>>,
    file_rank_task: Option<Task<()>>,
    /// This view is `.cached()` in RootView, so ambient window redraws no
    /// longer reach it: store changes must notify it directly, or an open
    /// palette's session rows go stale.
    _store_changes: Option<Task<()>>,
}

impl EventEmitter<NavigationEvent> for NavigationOverlay {}

impl NavigationOverlay {
    pub fn new(runtime: Arc<StoreRuntime>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let _ = window;
        let mut changes = runtime.changes();
        let store_changes = cx.spawn(async move |this, cx| {
            loop {
                match changes.recv().await {
                    Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if this
                            .update(cx, |this, cx| {
                                match this.overlay {
                                    Some(Overlay::CommandPalette) => this.refresh_command_items(),
                                    Some(Overlay::QuickOpen) => {
                                        this.refresh_quick_sessions();
                                        this.refresh_file_workspace(cx);
                                    }
                                    None => {}
                                }
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
        let mut overlay = Self {
            focus_handle,
            store: Arc::clone(&runtime.store),
            _runtime: runtime,
            overlay: None,
            query: QueryEditor::default(),
            highlight: 0,
            ranked_actions: Vec::new(),
            ranked_sessions: Vec::new(),
            matcher: FuzzyMatcher::text(),
            directory_index: DirectoryIndex::default(),
            quick_snapshot: QuickOpenSnapshot::default(),
            ranked_items: Vec::new(),
            ranked_files: Vec::new(),
            file_workspace: None,
            file_root: None,
            file_intelligence: None,
            file_index_message: None,
            file_rank_generation: 0,
            scroll_handle: ScrollHandle::new(),
            cache_task: None,
            scan_task: None,
            rank_task: None,
            file_index_task: None,
            file_rank_task: None,
            _store_changes: Some(store_changes),
        };
        // Warm at launch, the way Zed's worktree scan does: the cache makes the
        // index usable immediately and the scan refreshes it behind that, so the
        // first ⌘P of a session never waits on `read_dir`.
        overlay.load_cached_index(cx);
        overlay.refresh_directory_index(cx);
        overlay
    }

    #[cfg(test)]
    fn opened_for_test(runtime: Arc<StoreRuntime>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            store: Arc::clone(&runtime.store),
            _runtime: runtime,
            overlay: Some(Overlay::CommandPalette),
            query: QueryEditor::default(),
            highlight: 0,
            ranked_actions: Vec::new(),
            ranked_sessions: Vec::new(),
            matcher: FuzzyMatcher::text(),
            directory_index: DirectoryIndex::default(),
            quick_snapshot: QuickOpenSnapshot::default(),
            ranked_items: Vec::new(),
            ranked_files: Vec::new(),
            file_workspace: None,
            file_root: None,
            file_intelligence: None,
            file_index_message: None,
            file_rank_generation: 0,
            scroll_handle: ScrollHandle::new(),
            cache_task: None,
            scan_task: None,
            rank_task: None,
            file_index_task: None,
            file_rank_task: None,
            _store_changes: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.overlay.is_some()
    }

    pub(crate) fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay == Some(Overlay::CommandPalette) {
            self.close_overlay(cx);
        } else {
            self.open_overlay(Overlay::CommandPalette, window, cx);
        }
    }

    pub(crate) fn toggle_quick_open(
        &mut self,
        _: &ToggleQuickOpen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay == Some(Overlay::QuickOpen) {
            self.close_overlay(cx);
        } else {
            self.open_overlay(Overlay::QuickOpen, window, cx);
        }
    }

    fn open_overlay(&mut self, overlay: Overlay, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Some(overlay);
        self.query.clear();
        self.reset_selection();
        self.ranked_items.clear();
        match overlay {
            Overlay::CommandPalette => self.refresh_command_items(),
            Overlay::QuickOpen => {
                self.refresh_quick_sessions();
                self.refresh_file_workspace(cx);
                self.schedule_file_rank(cx);
                self.refresh_directory_index(cx);
            }
        }
        let _ = window;
        cx.notify();
    }

    fn close_overlay(&mut self, cx: &mut Context<Self>) {
        self.overlay = None;
        self.query.clear();
        self.highlight = 0;
        self.ranked_actions.clear();
        self.ranked_sessions.clear();
        self.ranked_files.clear();
        self.rank_task = None;
        self.file_rank_task = None;
        self.file_rank_generation = self.file_rank_generation.wrapping_add(1);
        cx.notify();
    }

    /// Back to the first row, scrolled back to the top of the list.
    fn reset_selection(&mut self) {
        self.highlight = 0;
        self.scroll_handle.set_offset(gpui::point(px(0.0), px(0.0)));
    }

    /// The roots to index, and where their cached index lives.
    fn index_roots(&mut self) -> (Vec<PathBuf>, Vec<PathBuf>, PathBuf) {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/nonexistent"));
        let projects = self.project_roots();
        let mut fallback = vec![PathBuf::from("~/fun")];
        fallback.extend(
            projects
                .iter()
                .filter_map(|(root, _)| root.parent().map(Path::to_path_buf)),
        );
        let quick_open_roots = self
            .store
            .read()
            .expect("session store lock poisoned")
            .preferences()
            .quick_open_roots
            .clone();
        let roots = quick_open::resolve_roots(&quick_open_roots, &fallback, &home);
        let cache = quick_open::cache_file(&home);
        (roots, vec![home], cache)
    }

    /// Populate the index from the previous run's scan. Costs one file read, so
    /// the first ⌘P of a launch has results to show instead of "Scanning…".
    fn load_cached_index(&mut self, cx: &mut Context<Self>) {
        let (roots, _, cache) = self.index_roots();
        let (projects, cwds) = self.snapshot_inputs();
        self.cache_task = Some(cx.spawn(async move |this, cx| {
            let built = cx
                .background_spawn(async move {
                    let entries = quick_open::load_cache(&cache, &roots)?;
                    let snapshot = quick_open::build_snapshot(&entries, &projects, &cwds);
                    Some((entries, snapshot))
                })
                .await;
            let Some((entries, snapshot)) = built else {
                return;
            };
            this.update(cx, |this, cx| {
                this.directory_index.adopt_cached(entries);
                this.quick_snapshot = snapshot;
                cx.notify();
            })
            .ok();
        }));
    }

    fn refresh_directory_index(&mut self, cx: &mut Context<Self>) {
        if !self.directory_index.needs_scan(Instant::now()) || !self.directory_index.begin_scan() {
            return;
        }
        let (roots, standalone, cache) = self.index_roots();
        let (projects, cwds) = self.snapshot_inputs();

        self.scan_task = Some(cx.spawn(async move |this, cx| {
            // Scan, persist, and prepare 20 000 ranking candidates all on the
            // background executor: preparing them on the main thread cost ~13 ms,
            // which is a dropped frame on any display and most of two at 120 Hz.
            let (entries, snapshot) = cx
                .background_spawn(async move {
                    let entries = quick_open::scan(&roots, &standalone);
                    quick_open::store_cache(&cache, &roots, &entries);
                    let snapshot = quick_open::build_snapshot(&entries, &projects, &cwds);
                    (entries, snapshot)
                })
                .await;
            this.update(cx, |this, cx| {
                this.directory_index.finish_scan(entries, Instant::now());
                this.quick_snapshot = snapshot;
                if !this.query.text().trim().is_empty() {
                    this.schedule_folder_rank(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The Recent section's contents: configured projects first, then session
    /// working directories in most-recently-updated order.
    fn snapshot_inputs(&mut self) -> (Vec<(PathBuf, String)>, Vec<PathBuf>) {
        let projects = self.project_roots();
        let store = self.store.read().expect("session store lock poisoned");
        let mut sessions: Vec<_> = store.sessions().values().collect();
        sessions.sort_by(|left, right| {
            right
                .updated_at
                .partial_cmp(&left.updated_at)
                .unwrap_or(Ordering::Equal)
        });
        let cwds = sessions
            .into_iter()
            .map(|session| PathBuf::from(&session.cwd))
            .collect();
        (projects, cwds)
    }

    fn project_roots(&mut self) -> Vec<(PathBuf, String)> {
        self.store
            .write()
            .expect("session store lock poisoned")
            .sidebar_projection()
            .projects
            .iter()
            .map(|entry| {
                (
                    PathBuf::from(&entry.project.root),
                    entry.project.name.clone(),
                )
            })
            .collect()
    }

    fn schedule_folder_rank(&mut self, cx: &mut Context<Self>) {
        self.rank_task = None;
        let query = self.query.text().trim().to_owned();
        if query.is_empty() {
            self.ranked_items.clear();
            cx.notify();
            return;
        }
        let pool = self.quick_snapshot.pool.clone();
        self.rank_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RANK_DEBOUNCE).await;
            let ranked = cx
                .background_spawn(async move { quick_open::rank(&query, &pool, RESULT_LIMIT) })
                .await;
            this.update(cx, |this, cx| {
                this.ranked_items = ranked;
                this.clamp_highlight();
                cx.notify();
            })
            .ok();
        }));
    }

    fn refresh_quick_sessions(&mut self) {
        let sessions = self
            .store
            .write()
            .expect("session store lock poisoned")
            .ordered_sessions();
        let query = FuzzyQuery::new(self.query.text());
        self.ranked_sessions = palette::rank_sessions(sessions, &query, &mut self.matcher);
    }

    /// Cmd+P searches files from the selected local session's workspace. The
    /// expensive index is retained across opens and query edits; changing the
    /// selected workspace invalidates it explicitly.
    fn refresh_file_workspace(&mut self, cx: &mut Context<Self>) {
        let (workspace, unavailable) = {
            let mut store = self.store.write().expect("session store lock poisoned");
            let selected = store
                .selected_session()
                .map(|session| (session.cwd.clone(), session.host.is_some()));
            match selected {
                Some((cwd, false)) => (Some(PathBuf::from(cwd)), None),
                Some(_) => (
                    None,
                    Some(SharedString::from(
                        "Project files are unavailable for remote sessions",
                    )),
                ),
                None => store
                    .sidebar_projection()
                    .projects
                    .first()
                    .map(|entry| (Some(PathBuf::from(&entry.project.root)), None))
                    .unwrap_or_else(|| {
                        (
                            None,
                            Some(SharedString::from(
                                "Open a project to search files from startup",
                            )),
                        )
                    }),
            }
        };

        if self.file_workspace == workspace {
            return;
        }

        self.file_workspace = workspace.clone();
        self.file_root = None;
        self.file_intelligence = None;
        self.ranked_files.clear();
        self.file_index_message = unavailable;
        self.file_index_task = None;
        self.file_rank_task = None;
        self.file_rank_generation = self.file_rank_generation.wrapping_add(1);

        let Some(workspace) = workspace else {
            self.clamp_highlight();
            cx.notify();
            return;
        };
        self.file_index_message = Some(SharedString::from("Indexing project files…"));
        let requested = workspace.clone();
        self.file_index_task = Some(cx.spawn(async move |this, cx| {
            let build_workspace = requested.clone();
            let built = cx
                .background_spawn(async move {
                    let intelligence = CodeIntelligence::for_session(&build_workspace)
                        .map_err(|error| error.to_string())?;
                    let initial = intelligence.search_files("", FILE_RESULT_LIMIT);
                    Ok::<_, String>((Arc::new(intelligence), initial))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.file_workspace.as_ref() != Some(&requested) {
                    return;
                }
                this.file_index_task = None;
                match built {
                    Ok((intelligence, initial)) => {
                        this.file_root = Some(intelligence.workspace_root().to_path_buf());
                        this.file_intelligence = Some(intelligence);
                        this.file_index_message = None;
                        if this.query.text().trim().is_empty() {
                            this.ranked_files = initial;
                            this.clamp_highlight();
                        } else {
                            this.schedule_file_rank(cx);
                        }
                    }
                    Err(_) => {
                        this.file_index_message =
                            Some(SharedString::from("Project files could not be indexed"));
                        this.ranked_files.clear();
                        this.clamp_highlight();
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn schedule_file_rank(&mut self, cx: &mut Context<Self>) {
        self.file_rank_task = None;
        self.file_rank_generation = self.file_rank_generation.wrapping_add(1);
        let generation = self.file_rank_generation;
        let Some(intelligence) = self.file_intelligence.clone() else {
            return;
        };
        let query = self.query.text().trim().to_owned();
        self.file_rank_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RANK_DEBOUNCE).await;
            let ranked = cx
                .background_spawn(
                    async move { intelligence.search_files(&query, FILE_RESULT_LIMIT) },
                )
                .await;
            this.update(cx, |this, cx| {
                if this.overlay != Some(Overlay::QuickOpen)
                    || this.file_rank_generation != generation
                {
                    return;
                }
                this.ranked_files = ranked;
                this.clamp_highlight();
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay.is_none() {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "escape" => self.close_overlay(cx),
            "up" => self.move_highlight(-1, cx),
            "down" => self.move_highlight(1, cx),
            "p" if modifiers.control => self.move_highlight(-1, cx),
            "n" if modifiers.control => self.move_highlight(1, cx),
            "enter" => self.run_highlighted(modifiers.platform, cx),
            _ => self.edit_query(event, cx),
        }
        cx.stop_propagation();
    }

    /// Everything the search field itself handles, through the key map shared
    /// with Quick Open and the terminal's find bar.
    fn edit_query(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(edit) = query_editor::edit_for(&event.keystroke) else {
            return;
        };
        let changed = match edit {
            Edit::Local(local) => self.query.apply(local),
            Edit::Clipboard(ClipboardEdit::Copy) => {
                query_editor::copy_selection(&self.query, cx);
                false
            }
            Edit::Clipboard(ClipboardEdit::Cut) => query_editor::cut_selection(&mut self.query, cx),
            Edit::Clipboard(ClipboardEdit::Paste) => cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .is_some_and(|text| self.query.insert(&text)),
        };

        if changed {
            self.query_changed(cx);
        } else {
            // The caret or selection moved even when the text did not.
            cx.notify();
        }
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        self.reset_selection();
        if self.overlay == Some(Overlay::QuickOpen) {
            self.refresh_quick_sessions();
            self.schedule_folder_rank(cx);
            self.schedule_file_rank(cx);
            cx.notify();
        } else {
            self.refresh_command_items();
            cx.notify();
        }
    }

    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.visible_count();
        if count == 0 {
            return;
        }
        self.highlight = (self.highlight as isize + delta).rem_euclid(count as isize) as usize;
        self.scroll_to_highlight();
        cx.notify();
    }

    /// Keyboard navigation must drag the viewport along with it; the list is
    /// taller than the window on any real machine.
    fn scroll_to_highlight(&self) {
        self.scroll_handle.scroll_to_item(self.highlight_child());
    }

    /// Index of the highlighted row among the scroll container's children,
    /// which include the section headers.
    fn highlight_child(&self) -> usize {
        match self.overlay {
            Some(Overlay::CommandPalette) => {
                row_child_index(self.highlight, Some(self.ranked_actions.len()))
            }
            Some(Overlay::QuickOpen) => {
                sectioned_row_child_index(self.highlight, &self.quick_section_lengths())
            }
            None => self.highlight,
        }
    }

    fn visible_count(&self) -> usize {
        match self.overlay {
            Some(Overlay::CommandPalette) => self.ranked_actions.len() + self.ranked_sessions.len(),
            Some(Overlay::QuickOpen) => self.quick_section_lengths().into_iter().sum(),
            None => 0,
        }
    }

    fn quick_section_lengths(&self) -> [usize; 3] {
        [
            self.ranked_sessions.len(),
            self.ranked_files.len(),
            self.quick_folder_count(),
        ]
    }

    fn quick_folder_count(&self) -> usize {
        if self.query.text().trim().is_empty() {
            self.quick_snapshot.recent.len() + self.quick_snapshot.folders.len()
        } else {
            self.ranked_items.len()
        }
    }

    fn quick_folder_at(&self, index: usize) -> Option<QuickOpenItem> {
        if self.query.text().trim().is_empty() {
            self.quick_snapshot
                .recent
                .iter()
                .chain(&self.quick_snapshot.folders)
                .nth(index)
                .cloned()
        } else {
            self.ranked_items
                .get(index)
                .map(|folder| folder.item.clone())
        }
    }

    fn clamp_highlight(&mut self) {
        self.highlight = self.highlight.min(self.visible_count().saturating_sub(1));
    }

    fn run_highlighted(&mut self, secondary: bool, cx: &mut Context<Self>) {
        match self.overlay {
            Some(Overlay::CommandPalette) => {
                let selection = if let Some(action) = self.ranked_actions.get(self.highlight) {
                    action
                        .item
                        .enabled
                        .then(|| CommandSelection::Action(action.item.command.clone()))
                } else {
                    self.ranked_sessions
                        .get(self.highlight.saturating_sub(self.ranked_actions.len()))
                        .map(|ranked| CommandSelection::Session(ranked.item.id.clone()))
                };
                if let Some(selection) = selection {
                    self.run_command_selection(selection, cx);
                }
            }
            Some(Overlay::QuickOpen) => {
                if let Some(selection) = self.current_quick_selection() {
                    match selection {
                        QuickSelection::File { cwd, reference } => {
                            cx.emit(NavigationEvent::OpenFile { cwd, reference });
                        }
                        QuickSelection::Session(id) => {
                            self.store
                                .write()
                                .expect("session store lock poisoned")
                                .select(id);
                        }
                        QuickSelection::Folder(item) if secondary => {
                            let cwd = item.path.to_string_lossy().into_owned();
                            self.store
                                .write()
                                .expect("session store lock poisoned")
                                .spawn_shell(SpawnOptions {
                                    cwd: Some(cwd),
                                    ..SpawnOptions::default()
                                });
                        }
                        QuickSelection::Folder(item) => {
                            let cwd = item.path.to_string_lossy().into_owned();
                            self.store
                                .write()
                                .expect("session store lock poisoned")
                                .spawn_default(SpawnOptions {
                                    cwd: Some(cwd),
                                    ..SpawnOptions::default()
                                });
                        }
                    }
                    self.close_overlay(cx);
                }
            }
            None => {}
        }
    }

    fn run_command_selection(&mut self, selection: CommandSelection, cx: &mut Context<Self>) {
        match selection {
            CommandSelection::Session(id) => {
                self.store
                    .write()
                    .expect("session store lock poisoned")
                    .select(id);
                self.close_overlay(cx);
            }
            CommandSelection::Action(command) => self.run_palette_command(command, cx),
        }
    }

    fn run_palette_command(&mut self, command: PaletteCommand, cx: &mut Context<Self>) {
        match command {
            PaletteCommand::SpawnAgent { agent, cwd, host } => {
                {
                    let mut store = self.store.write().expect("session store lock poisoned");
                    let mut options = SpawnOptions {
                        cwd: cwd.map(|path| path.to_string_lossy().into_owned()),
                        host: host.clone(),
                        ..SpawnOptions::default()
                    };
                    // Repo-preserving spawn: when no explicit directory was
                    // chosen and the spawn targets a remote host (or the
                    // active session lives on one), keep the active REPO —
                    // the daemon resolves its checkout on the target host.
                    let selected = store.selected_session();
                    let active_host = selected.and_then(|session| session.host.clone());
                    if options.cwd.is_none() && (host.is_some() || active_host.is_some()) {
                        options.same_repo_as = selected.map(|session| session.id.clone());
                        if host.is_none() && active_host.is_some() {
                            // Remote session spawning locally: its remote cwd
                            // is useless as a local path.
                            options.cwd = Some(store.local_fallback_directory());
                        }
                    }
                    store.spawn_kind(agent, options);
                }
                self.close_overlay(cx);
            }
            PaletteCommand::UnavailableAgent { setup_url } => {
                if let Some(url) = setup_url {
                    cx.open_url(&url);
                }
            }
            PaletteCommand::MigrateSelected { target_host } => {
                {
                    let mut store = self.store.write().expect("session store lock poisoned");
                    if let Some(id) = store.selected_session_id().cloned() {
                        store.migrate_session(id, target_host);
                    }
                }
                self.close_overlay(cx);
            }
            PaletteCommand::SyncPrefs { host } => {
                self.store
                    .write()
                    .expect("session store lock poisoned")
                    .sync_prefs(host);
                self.close_overlay(cx);
            }
            PaletteCommand::SpawnShell { host } => {
                self.store
                    .write()
                    .expect("session store lock poisoned")
                    .spawn_kind(
                        zeus_proto::AgentKind::SHELL,
                        SpawnOptions {
                            host,
                            ..SpawnOptions::default()
                        },
                    );
                self.close_overlay(cx);
            }
            PaletteCommand::OpenQuickOpen => {
                self.overlay = Some(Overlay::QuickOpen);
                self.query.clear();
                self.reset_selection();
                self.ranked_items.clear();
                self.refresh_quick_sessions();
                self.refresh_file_workspace(cx);
                self.schedule_file_rank(cx);
                self.refresh_directory_index(cx);
                cx.notify();
            }
            PaletteCommand::ToggleSidebar => {
                cx.emit(NavigationEvent::ToggleSidebar);
                self.close_overlay(cx);
            }
            PaletteCommand::OpenSessionOverview => {
                cx.emit(NavigationEvent::OpenOverview);
                self.close_overlay(cx);
            }
            PaletteCommand::ShowAgentWorkflow => {
                self.store
                    .write()
                    .expect("session store lock poisoned")
                    .set_lineage_view(crate::store::LineageView::Tree);
                self.close_overlay(cx);
            }
            PaletteCommand::OpenWorktrees => {
                cx.emit(NavigationEvent::OpenWorktrees);
                self.close_overlay(cx);
            }
            PaletteCommand::OpenBranches => {
                cx.emit(NavigationEvent::OpenBranches);
                self.close_overlay(cx);
            }
            PaletteCommand::GoToPullRequest => {
                cx.emit(NavigationEvent::GoToPullRequest);
                self.close_overlay(cx);
            }
            PaletteCommand::OpenSettings => {
                cx.emit(NavigationEvent::OpenSettings);
                self.close_overlay(cx);
            }
            PaletteCommand::OpenDocumentation => {
                cx.open_url(crate::settings::DOCS_URL);
                self.close_overlay(cx);
            }
            PaletteCommand::CheckForUpdates => {
                cx.emit(NavigationEvent::CheckForUpdates);
                self.close_overlay(cx);
            }
        }
    }

    /// Rebuild the palette's ranked rows for the current query. Cheap enough
    /// to run on every keystroke — a few hundred candidates against one
    /// matcher — and never run per frame.
    fn refresh_command_items(&mut self) {
        let (actions, sessions) = {
            let mut store = self.store.write().expect("session store lock poisoned");
            let projects: Vec<_> = store
                .sidebar_projection()
                .projects
                .iter()
                .map(|entry| entry.project.clone())
                .collect();
            let hosts = store.hosts().to_vec();
            let selected = store.selected_session().cloned();
            let default_host = store.default_spawn_host();
            let actions = palette::actions_for_default_host(
                store.preferences().default_agent.clone(),
                store.agent_catalog(),
                &projects,
                &hosts,
                selected.as_ref(),
                default_host.as_deref(),
            );
            (actions, store.ordered_sessions())
        };
        let query = FuzzyQuery::new(self.query.text());
        self.ranked_actions = palette::rank_actions(actions, &query, &mut self.matcher);
        self.ranked_sessions = palette::rank_sessions(sessions, &query, &mut self.matcher);
    }

    fn current_quick_selection(&self) -> Option<QuickSelection> {
        let mut index = self.highlight;
        if let Some(session) = self.ranked_sessions.get(index) {
            return Some(QuickSelection::Session(session.item.id.clone()));
        }
        index = index.saturating_sub(self.ranked_sessions.len());

        if let Some(hit) = self.ranked_files.get(index) {
            let cwd = self.file_root.clone()?;
            return Some(QuickSelection::File {
                cwd,
                reference: hit.relative_path.to_string_lossy().into_owned(),
            });
        }
        index = index.saturating_sub(self.ranked_files.len());
        self.quick_folder_at(index).map(QuickSelection::Folder)
    }

    fn render_overlay(&mut self, layout: OverlayLayout, cx: &mut Context<Self>) -> AnyElement {
        let colors = {
            let store = self.store.read().expect("session store lock poisoned");
            crate::app_theme::colors(&store.preferences().terminal_theme)
        };
        let content = match self.overlay {
            Some(Overlay::CommandPalette) => self.render_command_palette(layout, colors, cx),
            Some(Overlay::QuickOpen) => self.render_quick_open(layout, colors, cx),
            None => return div().into_any_element(),
        };
        div()
            .absolute()
            .inset_0()
            // A modal owns the entire wheel gesture, including the backdrop.
            // Without this, trackpad deltas hit the terminal underneath and its
            // precise-scroll accumulator releases them after the modal closes.
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .flex()
            .items_start()
            .justify_center()
            .pt(layout.top_inset)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .occlude()
                    .bg(rgba(0x00000055))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_overlay(cx);
                        }),
                    ),
            )
            .child(
                div()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.close_overlay(cx);
                    }))
                    .child(FloatingSurface::new(colors, content)),
            )
            .into_any_element()
    }

    fn render_search(&self, placeholder: &'static str, colors: SemanticColors) -> AnyElement {
        let field = div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(SEARCH_HEIGHT))
            // Line the query up with the rows' icon column below it.
            .px(px(LIST_PADDING_X + ROW_PADDING_X))
            .text_size(px(14.0));

        if self.query.is_empty() {
            return field
                .text_color(colors.tertiary)
                .child(placeholder)
                .into_any_element();
        }

        field
            .text_color(colors.primary)
            .child(query_label(&self.query))
            .into_any_element()
    }

    fn render_command_palette(
        &mut self,
        layout: OverlayLayout,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let actions = self.ranked_actions.clone();
        let sessions = self.ranked_sessions.clone();
        let action_count = actions.len();
        let session_count = sessions.len();
        let mut results = div()
            .id("command-palette-results")
            .track_scroll(&self.scroll_handle)
            .flex()
            .flex_col()
            .max_h(layout.list_height)
            .overflow_y_scroll()
            .px(px(LIST_PADDING_X))
            .py(px(LIST_PADDING_Y));

        if !actions.is_empty() {
            results = results.child(section_header("Actions", colors));
            for (index, action) in actions.into_iter().enumerate() {
                results = results.child(self.render_action_row(action, index, colors, cx));
            }
        }
        if !sessions.is_empty() {
            results = results.child(section_header("Sessions", colors));
            for (offset, session) in sessions.into_iter().enumerate() {
                results = results.child(self.render_session_row(
                    session,
                    action_count + offset,
                    colors,
                    cx,
                ));
            }
        }
        if action_count == 0 && session_count == 0 {
            results = results.child(empty_label("No matches", colors));
        }

        div()
            .w(layout.width)
            .text_color(colors.primary)
            .child(self.render_search("Type a command or session…", colors))
            .child(HairlineDivider::horizontal(colors))
            .child(results)
            .into_any_element()
    }

    fn render_action_row(
        &mut self,
        ranked: Ranked<PaletteAction>,
        index: usize,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let action = ranked.item;
        let command = action.command.clone();
        let enabled = action.enabled;
        let trailing = action
            .detail
            .clone()
            .map(SharedString::from)
            .or_else(|| action.shortcut.map(SharedString::from));
        palette_row(
            highlighted_label(action.title, &ranked.title_matches),
            sf_symbol(action.system_image, 12.5, colors.secondary),
            trailing,
            index == self.highlight,
            index,
            enabled,
            colors,
        )
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if *hovered {
                this.highlight = index;
                cx.notify();
            }
        }))
        .when(enabled, |row| {
            row.on_click(cx.listener(move |this, _, _, cx| {
                this.run_command_selection(CommandSelection::Action(command.clone()), cx);
            }))
        })
        .into_any_element()
    }

    fn render_session_row(
        &mut self,
        ranked: Ranked<SessionRecord>,
        index: usize,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let session = ranked.item;
        let id = session.id.clone();
        let dot_color = attention_color(session.attention(), colors);
        let chip = kind_label(session.effective_kind());
        palette_row(
            highlighted_label(session.title, &ranked.title_matches),
            div()
                .flex_none()
                .size(px(7.0))
                .rounded_full()
                .bg(dot_color)
                .into_any_element(),
            Some(SharedString::from(chip)),
            index == self.highlight,
            index,
            true,
            colors,
        )
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if *hovered {
                this.highlight = index;
                cx.notify();
            }
        }))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.run_command_selection(CommandSelection::Session(id.clone()), cx);
        }))
        .into_any_element()
    }

    fn render_quick_open(
        &mut self,
        layout: OverlayLayout,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let searching = !self.query.text().trim().is_empty();
        let sessions = self.ranked_sessions.clone();
        let files = self.ranked_files.clone();
        let session_count = sessions.len();
        let file_count = files.len();
        let folders: Vec<_> = if searching {
            self.ranked_items
                .iter()
                .map(|folder| (folder.item.clone(), folder.name_matches.clone()))
                .collect()
        } else {
            self.quick_snapshot
                .recent
                .iter()
                .chain(&self.quick_snapshot.folders)
                .cloned()
                .map(|folder| (folder, Vec::new()))
                .collect()
        };
        let folder_count = folders.len();
        let mut results = div()
            .id("quick-open-results")
            .track_scroll(&self.scroll_handle)
            .flex()
            .flex_col()
            .h(layout.list_height)
            .overflow_y_scroll()
            .px(px(LIST_PADDING_X))
            .py(px(LIST_PADDING_Y));

        if !sessions.is_empty() {
            results = results.child(section_header("Agent Sessions", colors));
            for (index, session) in sessions.into_iter().enumerate() {
                results = results.child(self.render_quick_session_row(session, index, colors, cx));
            }
        }
        if !files.is_empty() {
            results = results.child(section_header("Project Files", colors));
            for (offset, file) in files.into_iter().enumerate() {
                results =
                    results.child(self.render_file_row(file, session_count + offset, colors, cx));
            }
        }
        if !folders.is_empty() {
            results = results.child(section_header("Projects & Folders", colors));
            for (offset, (folder, matches)) in folders.into_iter().enumerate() {
                results = results.child(self.render_quick_folder_row(
                    folder,
                    &matches,
                    session_count + file_count + offset,
                    colors,
                    cx,
                ));
            }
        }
        if session_count + file_count + folder_count == 0 {
            let message = self.file_index_message.clone().unwrap_or_else(|| {
                SharedString::from(if self.directory_index.is_scanning() {
                    "Searching your workspace…"
                } else {
                    "No files, sessions, or folders match"
                })
            });
            results = results.child(empty_message(message, colors));
        }

        let action = match self.current_quick_selection() {
            Some(QuickSelection::File { .. }) => "Open file",
            Some(QuickSelection::Session(_)) => "Switch session",
            Some(QuickSelection::Folder(_)) => "Start agent",
            None => "Open",
        };

        div()
            .w(layout.width)
            .rounded(px(Radius::PANEL))
            .overflow_hidden()
            .text_color(colors.primary)
            .child(self.render_quick_search(colors))
            .child(HairlineDivider::horizontal(colors))
            .child(results)
            .child(HairlineDivider::horizontal(colors))
            .child(quick_footer(action, colors))
            .into_any_element()
    }

    fn render_quick_search(&self, colors: SemanticColors) -> AnyElement {
        let query = if self.query.is_empty() {
            div()
                .text_color(colors.tertiary)
                .child("Search project files or agent sessions…")
                .into_any_element()
        } else {
            query_label(&self.query)
        };
        div()
            .h(px(QUICK_SEARCH_HEIGHT))
            .px(px(15.0))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.0))
            .cursor_text()
            .child(sf_symbol("magnifyingglass", 13.0, colors.secondary))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_size(px(14.0))
                    .text_color(colors.primary)
                    .child(query),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.5))
                    .text_color(colors.tertiary)
                    .child("files  ·  sessions  ·  folders"),
            )
            .into_any_element()
    }

    fn render_quick_session_row(
        &mut self,
        ranked: Ranked<SessionRecord>,
        index: usize,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let session = ranked.item;
        let dot_color = attention_color(session.attention(), colors);
        let kind = kind_label(session.effective_kind());
        let location = session.host.as_ref().map_or_else(
            || abbreviated_path(Path::new(&session.cwd)),
            |host| format!("{host}  ·  {}", session.cwd),
        );
        div()
            .id(("quick-session-row", index))
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(px(QUICK_ROW_HEIGHT))
            .px(px(ROW_PADDING_X))
            .rounded(px(Radius::ROW))
            .bg(quick_row_fill(index == self.highlight, colors))
            .cursor_pointer()
            .hover(move |row| row.bg(colors.primary.alpha(0.07)))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .w(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().size(px(7.0)).rounded_full().bg(dot_color)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(px(8.0))
                            .text_size(px(13.0))
                            .child(highlighted_label(session.title, &ranked.title_matches))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(11.0))
                                    .text_color(colors.tertiary)
                                    .child(location),
                            ),
                    ),
            )
            .child(chip(kind, colors))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.highlight = index;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.highlight = index;
                this.run_highlighted(false, cx);
            }))
            .into_any_element()
    }

    fn render_file_row(
        &mut self,
        hit: SearchHit,
        index: usize,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = hit
            .relative_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| hit.preview.clone());
        let parent = hit
            .relative_path
            .parent()
            .map(|path| path.to_string_lossy().into_owned())
            .filter(|path| !path.is_empty());
        div()
            .id(("quick-file-row", index))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(9.0))
            .h(px(QUICK_ROW_HEIGHT))
            .px(px(ROW_PADDING_X))
            .rounded(px(Radius::ROW))
            .bg(quick_row_fill(index == self.highlight, colors))
            .cursor_pointer()
            .hover(move |row| row.bg(colors.primary.alpha(0.07)))
            .child(
                div()
                    .w(px(18.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(sf_symbol("doc.text", 12.0, colors.secondary)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_baseline()
                    .gap(px(7.0))
                    .text_size(px(13.0))
                    .child(name)
                    .when_some(parent, |row, parent| {
                        row.child(
                            div()
                                .truncate()
                                .text_size(px(11.0))
                                .text_color(colors.tertiary)
                                .child(parent),
                        )
                    }),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.highlight = index;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.highlight = index;
                this.run_highlighted(false, cx);
            }))
            .into_any_element()
    }

    fn render_quick_folder_row(
        &mut self,
        item: QuickOpenItem,
        name_matches: &[Range<usize>],
        index: usize,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let parent = relative_parent(&item.path);
        let icon_color = if item.is_git_repo {
            Palette::CLAY
        } else {
            colors.secondary
        };
        let row = div()
            .id(("quick-folder-row", index))
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(px(if parent.is_empty() {
                QUICK_ROW_HEIGHT
            } else {
                QUICK_ROW_HEIGHT_WITH_PATH
            }))
            .px(px(ROW_PADDING_X))
            .rounded(px(Radius::ROW))
            .bg(quick_row_fill(index == self.highlight, colors))
            .cursor_pointer()
            .hover(move |row| row.bg(colors.primary.alpha(0.07)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .min_w(px(0.0))
                    .child(
                        div()
                            .w(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(sf_symbol_weighted(
                                if item.is_git_repo {
                                    "folder.fill"
                                } else {
                                    "folder"
                                },
                                13.0,
                                SymbolWeight::Regular,
                                icon_color,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w(px(0.0))
                            .text_size(px(13.0))
                            .child(highlighted_label(item.name.clone(), name_matches))
                            .when(!parent.is_empty(), |column| {
                                column.child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(colors.tertiary)
                                        .child(parent.clone()),
                                )
                            }),
                    ),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.highlight = index;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.highlight = index;
                this.run_highlighted(false, cx);
            }));
        row.into_any_element()
    }
}

impl Focusable for NavigationOverlay {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for NavigationOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = OverlayLayout::for_viewport(window.viewport_size());
        let overlay = self.overlay.map(|_| self.render_overlay(layout, cx));
        let root = div()
            .id("navigation-overlay")
            .key_context("Zeus")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_command_palette))
            .on_action(cx.listener(Self::toggle_quick_open))
            .on_key_down(cx.listener(Self::on_key_down))
            .absolute()
            // Cached entity roots are laid out independently, so insets alone
            // leave this absolute root without a definite size and its height
            // collapses to its in-flow content, which is nothing.
            .size_full();
        if let Some(overlay) = overlay {
            root.inset_0().child(overlay)
        } else {
            root.size(px(0.0))
        }
    }
}

fn section_header(title: &'static str, colors: SemanticColors) -> AnyElement {
    div()
        .h(px(SECTION_HEADER_HEIGHT))
        .flex()
        .flex_none()
        .items_end()
        .px(px(ROW_PADDING_X))
        .pb(px(3.0))
        .text_size(px(11.0))
        .text_color(colors.tertiary)
        .child(title)
        .into_any_element()
}

fn empty_label(text: &'static str, colors: SemanticColors) -> AnyElement {
    div()
        .h(px(ROW_HEIGHT))
        .flex()
        .flex_none()
        .items_center()
        .px(px(ROW_PADDING_X))
        .text_size(px(13.0))
        .text_color(colors.tertiary)
        .child(text)
        .into_any_element()
}

fn empty_message(text: impl Into<SharedString>, colors: SemanticColors) -> AnyElement {
    div()
        .min_h(px(88.0))
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.0))
        .text_color(colors.tertiary)
        .child(text.into())
        .into_any_element()
}

fn quick_row_fill(highlighted: bool, colors: SemanticColors) -> gpui::Rgba {
    if highlighted {
        colors.primary.alpha(0.12)
    } else {
        colors.primary.alpha(0.0)
    }
}

fn quick_footer(action: &'static str, colors: SemanticColors) -> AnyElement {
    div()
        .h(px(QUICK_FOOTER_HEIGHT))
        .px(px(14.0))
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .text_size(px(10.5))
        .text_color(colors.tertiary)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.0))
                .child(footer_hint("↑↓", "Navigate", colors))
                .child(footer_hint("↵", action, colors)),
        )
        .child(footer_hint("esc", "Close", colors))
        .into_any_element()
}

fn footer_hint(key: &'static str, label: &'static str, colors: SemanticColors) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .child(
            div()
                .px(px(4.0))
                .py(px(1.0))
                .rounded(px(Radius::CHIP))
                .border_1()
                .border_color(colors.primary.alpha(0.08))
                .bg(colors.primary.alpha(0.035))
                .text_color(colors.secondary)
                .child(key),
        )
        .child(label)
        .into_any_element()
}

/// Rows and section headers are siblings in the scroll container, so scrolling
/// to row N means scrolling to child N plus every header above it. `sections`
/// is the size of the first section, or `None` for a headerless flat list.
const fn row_child_index(row: usize, first_section: Option<usize>) -> usize {
    let Some(first) = first_section else {
        return row;
    };
    // Each non-empty section above the row contributes one header child.
    row + (first > 0) as usize + (row >= first) as usize
}

/// Quick Open has three independently filtered sections. Each non-empty
/// section contributes one header before its selectable rows.
fn sectioned_row_child_index(row: usize, sections: &[usize]) -> usize {
    let mut consumed = 0;
    let mut headers = 0;
    for &length in sections {
        if length == 0 {
            continue;
        }
        headers += 1;
        if row < consumed + length {
            return row + headers;
        }
        consumed += length;
    }
    row + headers
}

/// A static caret. Blinking would need an autonomous frame timer, which is
/// exactly what PERF.md's idle-CPU budget forbids; the terminal cursor is
/// static for the same reason.
pub(crate) const CARET: &str = "▏";

/// Draw a query field's contents: caret at the cursor, or the selection washed
/// in the brand accent. Shared by the palette, Quick Open, and the find bar so
/// all three fields look like the same control.
pub fn query_label(editor: &QueryEditor) -> AnyElement {
    let (text, selection) = editor.display(CARET);
    highlighted_label_styled(
        text,
        selection.as_slice(),
        HighlightStyle {
            background_color: Some(Palette::CLAY.alpha(0.35).into()),
            ..HighlightStyle::default()
        },
    )
}

/// Paint the characters the query actually matched in the brand accent, so a
/// glance at the list explains why each row is there and in that order.
fn highlighted_label(text: impl Into<SharedString>, matches: &[Range<usize>]) -> AnyElement {
    highlighted_label_styled(
        text,
        matches,
        HighlightStyle {
            color: Some(Palette::CLAY.into()),
            font_weight: Some(FontWeight::SEMIBOLD),
            ..HighlightStyle::default()
        },
    )
}

fn highlighted_label_styled(
    text: impl Into<SharedString>,
    matches: &[Range<usize>],
    style: HighlightStyle,
) -> AnyElement {
    let text = text.into();
    if matches.is_empty() {
        return div().child(text).into_any_element();
    }
    StyledText::new(text)
        .with_highlights(matches.iter().map(|range| (range.clone(), style)))
        .into_any_element()
}

fn palette_row(
    title: AnyElement,
    leading: AnyElement,
    // Owned: agent chips are manifest ids now, not compile-time literals.
    trailing: Option<SharedString>,
    highlighted: bool,
    index: usize,
    enabled: bool,
    colors: SemanticColors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(format!("palette-row-{index}"))
        .flex()
        // Without this the rows are shrinkable flex children: a list taller
        // than its container squeezes every row toward min-content instead of
        // scrolling, and 40pt rows render as ~21pt of crammed text.
        .flex_none()
        .items_center()
        .justify_between()
        .h(px(ROW_HEIGHT))
        .px(px(ROW_PADDING_X))
        .rounded(px(Radius::ROW))
        .bg(if highlighted {
            colors.primary.alpha(0.10)
        } else {
            colors.primary.alpha(0.0)
        })
        .opacity(if enabled { 1.0 } else { 0.48 })
        .when(enabled, |row| row.cursor_pointer())
        .text_size(px(13.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.0))
                .min_w_0()
                .child(
                    div()
                        .w(px(18.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(leading),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(title),
                ),
        )
        .when_some(trailing, |row, trailing| row.child(chip(trailing, colors)))
}

fn chip(text: impl Into<gpui::SharedString>, colors: SemanticColors) -> AnyElement {
    div()
        .px(px(5.0))
        .py(px(2.0))
        .rounded(px(Radius::CHIP))
        .bg(colors.primary.alpha(0.06))
        .text_size(px(11.0))
        .text_color(colors.tertiary)
        .child(text.into())
        .into_any_element()
}

fn attention_color(attention: AttentionLevel, colors: SemanticColors) -> gpui::Rgba {
    match attention {
        AttentionLevel::NeedsInput => gpui::rgb(0xf59e0b),
        AttentionLevel::DoneUnseen => gpui::rgb(0x3b82f6),
        AttentionLevel::Working => colors.secondary,
        _ => colors.tertiary,
    }
}

/// Compact label for the navigator's kind column. The manifest id is already a
/// short lowercase word for every agent, so only the two non-agent kinds and
/// Claude's hyphenated id need shortening.
fn kind_label(kind: &AgentKind) -> String {
    match kind.id() {
        AgentKind::CLAUDE_CODE_ID => "claude".to_owned(),
        AgentKind::GENERIC_ID => "term".to_owned(),
        other => other.to_owned(),
    }
}

fn relative_parent(path: &Path) -> String {
    let Some(parent) = path.parent() else {
        return String::new();
    };
    let parent = parent.to_string_lossy().into_owned();
    if parent.is_empty() || parent == "/" {
        return parent;
    }
    let Some(home) = std::env::var_os("HOME") else {
        return parent;
    };
    let home = PathBuf::from(home);
    if parent == home.to_string_lossy() {
        return "~".into();
    }
    parent
        .strip_prefix(&format!("{}/", home.to_string_lossy()))
        .map_or(parent.clone(), |suffix| format!("~/{suffix}"))
}

fn abbreviated_path(path: &Path) -> String {
    let path = path.to_string_lossy().into_owned();
    let Some(home) = std::env::var_os("HOME") else {
        return path;
    };
    let home = PathBuf::from(home).to_string_lossy().into_owned();
    if path == home {
        return "~".into();
    }
    path.strip_prefix(&format!("{home}/"))
        .map_or(path.clone(), |suffix| format!("~/{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use gpui::{Entity, ScrollDelta, ScrollWheelEvent, TestAppContext, point};

    #[test]
    fn relative_parent_abbreviates_home_like_swift() {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
        assert_eq!(relative_parent(&home.join("project")), "~");
        assert_eq!(relative_parent(&home.join("fun/project")), "~/fun");
        assert_eq!(relative_parent(Path::new("/tmp/project")), "/tmp");
    }

    #[test]
    fn debounce_is_the_swift_value() {
        assert_eq!(RANK_DEBOUNCE, std::time::Duration::from_millis(25));
    }

    #[test]
    fn scrolling_to_a_row_counts_the_section_headers_above_it() {
        // Five actions, then sessions: "Actions" header, five rows, "Sessions"
        // header, then the session rows.
        assert_eq!(row_child_index(0, Some(5)), 1);
        assert_eq!(row_child_index(4, Some(5)), 5);
        assert_eq!(row_child_index(5, Some(5)), 7);
        // No actions matched: only the "Sessions" header sits above row 0.
        assert_eq!(row_child_index(0, Some(0)), 1);
        // A headerless flat list remains useful for command-style lists.
        assert_eq!(row_child_index(3, None), 3);

        // Quick Open skips empty sections and counts each visible header.
        assert_eq!(sectioned_row_child_index(0, &[3, 4, 2]), 1);
        assert_eq!(sectioned_row_child_index(2, &[3, 4, 2]), 3);
        assert_eq!(sectioned_row_child_index(3, &[3, 4, 2]), 5);
        assert_eq!(sectioned_row_child_index(7, &[3, 4, 2]), 10);
        assert_eq!(sectioned_row_child_index(3, &[0, 4, 0]), 4);
    }

    fn layout(width: f32, height: f32) -> OverlayLayout {
        OverlayLayout::for_viewport(gpui::size(px(width), px(height)))
    }

    #[test]
    fn overlay_never_grows_past_the_window_it_floats_in() {
        for (width, height) in [
            (1100.0, 700.0),
            (1800.0, 1100.0),
            (900.0, 495.0),
            (600.0, 360.0),
        ] {
            let layout = layout(width, height);
            let total = layout.top_inset
                + px(QUICK_SEARCH_HEIGHT + QUICK_FOOTER_HEIGHT + 2.0)
                + layout.list_height;
            assert!(
                total <= px(height),
                "{width}x{height} overflows by {:?}",
                total - px(height)
            );
            assert!(layout.width <= px(width));
        }
    }

    #[test]
    fn the_list_uses_the_height_the_window_actually_has() {
        // The old fixed 400pt list wasted a tall window and overflowed a short
        // one; both directions now track the viewport.
        assert!(layout(1400.0, 1100.0).list_height > px(400.0));
        assert!(layout(900.0, 460.0).list_height < px(400.0));
        // Beyond the cap the surface stops growing rather than becoming a wall.
        assert_eq!(layout(1600.0, 3000.0).list_height, px(MAX_LIST_HEIGHT));
        // A window too short for the minimum list gives up its top inset first,
        // down to the floor.
        let cramped = layout(800.0, 160.0);
        assert!(cramped.top_inset < px(160.0 / 12.0));
        assert_eq!(cramped.list_height, px(MIN_LIST_HEIGHT));
        assert_eq!(layout(800.0, 150.0).top_inset, px(MIN_TOP_INSET));
    }

    struct WheelHarness {
        overlay: Entity<NavigationOverlay>,
        background_scrolls: Arc<AtomicUsize>,
    }

    impl Render for WheelHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let background_scrolls = Arc::clone(&self.background_scrolls);
            div()
                .size_full()
                .child(div().absolute().inset_0().on_scroll_wheel(move |_, _, _| {
                    background_scrolls.fetch_add(1, AtomicOrdering::Relaxed);
                }))
                .child(crate::root::cached_window_overlay(self.overlay.clone()))
        }
    }

    #[gpui::test]
    fn modal_backdrop_consumes_wheel_events(cx: &mut TestAppContext) {
        let runtime = Arc::new(StoreRuntime::inert());
        let background_scrolls = Arc::new(AtomicUsize::new(0));
        let scroll_probe = Arc::clone(&background_scrolls);
        let (_view, cx) = cx.add_window_view(move |_window, cx| {
            let overlay = cx.new(|cx| NavigationOverlay::opened_for_test(runtime, cx));
            WheelHarness {
                overlay,
                background_scrolls: scroll_probe,
            }
        });

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(8.0), px(320.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-40.0))),
            ..ScrollWheelEvent::default()
        });

        assert_eq!(background_scrolls.load(AtomicOrdering::Relaxed), 0);
    }
}
