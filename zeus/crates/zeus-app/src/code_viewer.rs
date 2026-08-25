//! Native code editor for the trailing workbench.
//!
//! `code_intelligence` owns filesystem discovery, containment and loading.
//! This module owns editing, asynchronous opens and saves, source history,
//! line targeting, virtualization, and lightweight lexical color.

use std::cell::Cell;
use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, App, Context, CursorStyle, FocusHandle, Focusable,
    FontWeight, HighlightStyle, KeyDownEvent, ListHorizontalSizingBehavior, MouseButton,
    MouseDownEvent, Render, ScrollStrategy, SharedString, StyledText, Task, TextRun,
    UniformListScrollHandle, Window, canvas, div, font, prelude::*, px, rgba, uniform_list,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::code_intelligence::{
    CodeIntelligence, CodeIntelligenceError, SearchHit, SearchHitKind, SourceLine, SourceSnapshot,
    source_lines,
};
use crate::file_tree::{
    ChangedSnapshot, FileTreeIntent, FileTreeMode, FileTreeModel, FileTreeWorkspace, TreeEntryKind,
    VisibleTreeRow,
};
use crate::git_review::ChangeKind;
use crate::macos::sf_symbols::{SymbolWeight, sf_symbol, sf_symbol_weighted};
use crate::query_editor::{self, ClipboardEdit, Edit, LocalEdit, Motion, QueryEditor};
use zeus_ui::{FloatingSurface, Ink, Metrics, Radius, SemanticColors, Typo};

#[cfg(test)]
use crate::code_intelligence::SourceTarget;

const SOURCE_ROW_HEIGHT: f32 = 20.0;
const SOURCE_GUTTER_WIDTH: f32 = 52.0;
const FILE_TREE_HEIGHT: f32 = 226.0;
const FILE_TREE_ROW_HEIGHT: f32 = 23.0;
const MAX_DIRECTORY_LOADS: usize = 2;

enum ViewerState {
    Empty,
    Loading { reference: String },
    Ready(Box<SourceDocument>),
    Error { reference: String, message: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TreeWorkspaceState {
    NoWorkspace,
    Loading,
    Ready,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ChangedTreeState {
    Loading,
    Ready { truncated: bool },
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SaveStatus {
    Idle,
    Saving,
    Saved,
    Error(String),
}

#[derive(Clone, Debug)]
enum PendingTransition {
    Workspace(Option<PathBuf>),
    Open {
        cwd: PathBuf,
        reference: String,
        record_history: bool,
    },
}

struct SourceDocument {
    snapshot: SourceSnapshot,
    editor: QueryEditor,
    saved_text: String,
    lines: Vec<SourceLine>,
    preferred_column: Option<usize>,
    save_status: SaveStatus,
    revision: u64,
}

struct SourceRowContext {
    focused: bool,
    extension: String,
    content_width: f32,
    word_wrap: bool,
    colors: SemanticColors,
    editor: gpui::Entity<CodeViewer>,
    caret_epoch: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceVisualRow {
    line_index: usize,
    range: Range<usize>,
    first: bool,
    last: bool,
}

impl SourceDocument {
    fn new(snapshot: SourceSnapshot) -> Self {
        let cursor = snapshot
            .target
            .map(|target| {
                target_offset(&snapshot.text, &snapshot.lines, target.line, target.column)
            })
            .unwrap_or(0);
        let saved_text = snapshot.text.clone();
        let lines = snapshot.lines.clone();
        let mut editor = QueryEditor::default();
        editor.reset(snapshot.text.clone(), cursor);
        Self {
            snapshot,
            editor,
            saved_text,
            lines,
            preferred_column: None,
            save_status: SaveStatus::Idle,
            revision: 0,
        }
    }

    fn is_dirty(&self) -> bool {
        self.editor.text() != self.saved_text
    }

    fn line_index_for_cursor(&self) -> usize {
        line_index_for_offset(&self.lines, self.editor.cursor())
    }

    fn changed(&mut self) {
        self.lines = source_lines(self.editor.text());
        self.snapshot.target = None;
        self.preferred_column = None;
        self.save_status = SaveStatus::Idle;
        self.revision = self.revision.wrapping_add(1);
    }

    fn apply_local(&mut self, edit: LocalEdit) -> bool {
        let line_index = self.line_index_for_cursor();
        let line = self.lines[line_index].range.clone();
        let changed = match edit {
            LocalEdit::MoveLeft(Motion::Line, extend) => {
                self.editor.move_to(line.start, extend);
                false
            }
            LocalEdit::MoveRight(Motion::Line, extend) => {
                self.editor.move_to(line.end, extend);
                false
            }
            LocalEdit::DeleteBackward(Motion::Line) => self.editor.delete_to(line.start),
            LocalEdit::DeleteForward(Motion::Line) => self.editor.delete_to(line.end),
            LocalEdit::MoveLeft(Motion::Character, false)
                if self.editor.selection().is_none()
                    && self.editor.cursor() == line.start
                    && line_index > 0 =>
            {
                self.editor
                    .move_to(self.lines[line_index - 1].range.end, false);
                false
            }
            LocalEdit::MoveRight(Motion::Character, false)
                if self.editor.selection().is_none()
                    && self.editor.cursor() == line.end
                    && line_index + 1 < self.lines.len() =>
            {
                self.editor
                    .move_to(self.lines[line_index + 1].range.start, false);
                false
            }
            LocalEdit::DeleteBackward(Motion::Character)
                if self.editor.selection().is_none()
                    && self.editor.cursor() == line.start
                    && line_index > 0 =>
            {
                self.editor.delete_to(self.lines[line_index - 1].range.end)
            }
            LocalEdit::DeleteForward(Motion::Character)
                if self.editor.selection().is_none()
                    && self.editor.cursor() == line.end
                    && line_index + 1 < self.lines.len() =>
            {
                self.editor
                    .delete_to(self.lines[line_index + 1].range.start)
            }
            edit => self.editor.apply(edit),
        };
        if changed {
            self.changed();
        } else {
            self.preferred_column = None;
        }
        changed
    }

    fn insert(&mut self, insertion: &str) -> bool {
        let insertion = normalize_line_endings(insertion, self.line_ending());
        let changed = self.editor.insert_document(&insertion);
        if changed {
            self.changed();
        }
        changed
    }

    fn insert_newline(&mut self) -> bool {
        let line = &self.lines[self.line_index_for_cursor()].range;
        let indentation: String = self.editor.text()[line.clone()]
            .chars()
            .take_while(|character| matches!(character, ' ' | '\t'))
            .collect();
        self.insert(&format!("{}{}", self.line_ending(), indentation))
    }

    fn line_ending(&self) -> &'static str {
        if self.saved_text.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        }
    }

    fn move_vertical(&mut self, delta: isize, extend: bool) {
        let current = self.line_index_for_cursor();
        let current_range = self.lines[current].range.clone();
        let cursor = self
            .editor
            .cursor()
            .clamp(current_range.start, current_range.end);
        let column = self.preferred_column.unwrap_or_else(|| {
            self.editor.text()[current_range.start..cursor]
                .graphemes(true)
                .count()
        });
        self.preferred_column = Some(column);
        let Some(target) = current
            .checked_add_signed(delta)
            .filter(|index| *index < self.lines.len())
        else {
            self.editor.move_to(
                if delta < 0 {
                    0
                } else {
                    self.editor.text().len()
                },
                extend,
            );
            return;
        };
        let range = self.lines[target].range.clone();
        let offset = offset_at_grapheme_column(self.editor.text(), &range, column);
        self.editor.move_to(offset, extend);
    }

    fn set_cursor(&mut self, offset: usize, extend: bool) {
        self.editor.set_cursor(offset, extend);
        self.preferred_column = None;
    }

    fn revert(&mut self) {
        let cursor = self.editor.cursor().min(self.saved_text.len());
        self.editor.reset(self.saved_text.clone(), cursor);
        self.lines = source_lines(self.editor.text());
        self.preferred_column = None;
        self.save_status = SaveStatus::Idle;
        self.revision = self.revision.wrapping_add(1);
    }
}

fn line_index_for_offset(lines: &[SourceLine], offset: usize) -> usize {
    lines
        .iter()
        .rposition(|line| line.range.start <= offset)
        .unwrap_or(0)
}

fn offset_at_grapheme_column(text: &str, range: &Range<usize>, column: usize) -> usize {
    text[range.clone()]
        .grapheme_indices(true)
        .nth(column)
        .map_or(range.end, |(offset, _)| range.start + offset)
}

fn target_offset(text: &str, lines: &[SourceLine], line: usize, column: usize) -> usize {
    let line = &lines[line.saturating_sub(1).min(lines.len().saturating_sub(1))];
    text[line.range.clone()]
        .char_indices()
        .nth(column.saturating_sub(1))
        .map_or(line.range.end, |(offset, _)| line.range.start + offset)
}

fn normalize_line_endings(text: &str, line_ending: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    if line_ending == "\r\n" {
        normalized.replace('\n', "\r\n")
    } else {
        normalized
    }
}

/// Splits one logical line into display ranges while preserving every byte.
/// Whitespace is preferred as a boundary, but long tokens still wrap so code
/// cannot recreate a horizontal scrolling surface while wrapping is enabled.
pub(crate) fn word_wrap_ranges(text: &str, columns: usize) -> Vec<Range<usize>> {
    let columns = columns.max(1);
    if text.is_empty() {
        return std::iter::once(0..0).collect();
    }

    let mut ranges = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = start;
        let mut boundary = None;
        let mut used_columns = 0;
        let mut seen_non_whitespace = false;
        for (offset, character) in text[start..].char_indices() {
            let character_columns = if character == '\t' { 4 } else { 1 };
            if used_columns > 0 && used_columns + character_columns > columns {
                break;
            }
            end = start + offset + character.len_utf8();
            used_columns += character_columns;
            if character.is_whitespace() {
                if seen_non_whitespace {
                    boundary = Some(end);
                }
            } else {
                seen_non_whitespace = true;
            }
        }
        if end == text.len() {
            ranges.push(start..end);
            break;
        }
        let split = boundary.filter(|boundary| *boundary > start).unwrap_or(end);
        ranges.push(start..split);
        start = split;
    }
    ranges
}

fn source_visual_rows(document: &SourceDocument, columns: usize) -> Vec<SourceVisualRow> {
    let mut rows = Vec::new();
    for (line_index, line) in document.lines.iter().enumerate() {
        let source = document.editor.text()[line.range.clone()].trim_end_matches(['\r', '\n']);
        let ranges = word_wrap_ranges(source, columns);
        let last = ranges.len().saturating_sub(1);
        rows.extend(
            ranges
                .into_iter()
                .enumerate()
                .map(|(index, range)| SourceVisualRow {
                    line_index,
                    range,
                    first: index == 0,
                    last: index == last,
                }),
        );
    }
    rows
}

fn source_visual_index(document: &SourceDocument, line_index: usize, columns: usize) -> usize {
    document
        .lines
        .iter()
        .take(line_index)
        .map(|line| {
            let source = document.editor.text()[line.range.clone()].trim_end_matches(['\r', '\n']);
            word_wrap_ranges(source, columns).len()
        })
        .sum()
}

pub struct CodeViewer {
    tokio: tokio::runtime::Handle,
    focus: FocusHandle,
    workspace_cwd: Option<PathBuf>,
    intelligence: Option<Arc<CodeIntelligence>>,
    tree_workspace: Option<Arc<FileTreeWorkspace>>,
    tree_workspace_state: TreeWorkspaceState,
    changed_tree_state: ChangedTreeState,
    tree_model: FileTreeModel,
    tree_focused: bool,
    tree_query: QueryEditor,
    tree_scroll: UniformListScrollHandle,
    tree_generation: u64,
    tree_directory_loads: usize,
    tree_loading_directories: HashSet<PathBuf>,
    tree_notice: Option<String>,
    pending_tree_reference: Option<String>,
    pending_tree_path: Option<PathBuf>,
    pending_changed_snapshot: Option<ChangedSnapshot>,
    pending_changed_message: Option<String>,
    state: ViewerState,
    scroll: UniformListScrollHandle,
    generation: u64,
    _load_task: Option<Task<()>>,
    _search_task: Option<Task<()>>,
    _save_task: Option<Task<()>>,
    _tree_workspace_task: Option<Task<()>>,
    _tree_reveal_task: Option<Task<()>>,
    search_generation: u64,
    picker_open: bool,
    query: QueryEditor,
    results: Vec<SearchHit>,
    highlighted_result: usize,
    history: Vec<(PathBuf, String)>,
    history_index: usize,
    pending_transition: Option<PendingTransition>,
    /// Changes whenever the caret moves or the document changes. GPUI keys
    /// the blink animation by this value, so interaction always brings the
    /// caret back visibly before the next blink cycle begins.
    caret_epoch: u64,
    selection_dragging: bool,
    word_wrap: bool,
    viewport_width: f32,
}

impl CodeViewer {
    pub fn new(
        tokio: tokio::runtime::Handle,
        word_wrap: bool,
        viewport_width: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            tokio,
            focus: cx.focus_handle(),
            workspace_cwd: None,
            intelligence: None,
            tree_workspace: None,
            tree_workspace_state: TreeWorkspaceState::NoWorkspace,
            changed_tree_state: ChangedTreeState::Loading,
            tree_model: FileTreeModel::default(),
            tree_focused: false,
            tree_query: QueryEditor::default(),
            tree_scroll: UniformListScrollHandle::new(),
            tree_generation: 0,
            tree_directory_loads: 0,
            tree_loading_directories: HashSet::new(),
            tree_notice: None,
            pending_tree_reference: None,
            pending_tree_path: None,
            pending_changed_snapshot: None,
            pending_changed_message: None,
            state: ViewerState::Empty,
            scroll: UniformListScrollHandle::new(),
            generation: 0,
            _load_task: None,
            _search_task: None,
            _save_task: None,
            _tree_workspace_task: None,
            _tree_reveal_task: None,
            search_generation: 0,
            picker_open: false,
            query: QueryEditor::default(),
            results: Vec::new(),
            highlighted_result: 0,
            history: Vec::new(),
            history_index: 0,
            pending_transition: None,
            caret_epoch: 0,
            selection_dragging: false,
            word_wrap,
            viewport_width,
        }
    }

    pub fn set_word_wrap(&mut self, word_wrap: bool, cx: &mut Context<Self>) {
        if self.word_wrap == word_wrap {
            return;
        }
        self.word_wrap = word_wrap;
        self.recenter_scroll();
        cx.notify();
    }

    pub fn set_viewport_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if (self.viewport_width - width).abs() < 0.5 {
            return;
        }
        self.viewport_width = width;
        if self.word_wrap {
            self.recenter_scroll();
            cx.notify();
        }
    }

    fn recenter_scroll(&mut self) {
        let logical_line = match &self.state {
            ViewerState::Ready(document) => document.line_index_for_cursor(),
            _ => 0,
        };
        let item = match &self.state {
            ViewerState::Ready(document) if self.word_wrap => {
                source_visual_index(document, logical_line, self.wrap_columns())
            }
            _ => logical_line,
        };
        self.scroll = UniformListScrollHandle::new();
        self.scroll.scroll_to_item(item, ScrollStrategy::Center);
    }

    fn wrap_columns(&self) -> usize {
        ((self.viewport_width - SOURCE_GUTTER_WIDTH - 24.0) / 7.1)
            .floor()
            .max(8.0) as usize
    }

    fn reveal_caret(&mut self) {
        self.caret_epoch = self.caret_epoch.wrapping_add(1);
    }

    fn toggle_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_open = !self.picker_open;
        if self.picker_open {
            self.tree_focused = false;
            window.focus(&self.focus, cx);
            self.schedule_search(cx);
        } else {
            self.query.clear();
            self.results.clear();
            self.highlighted_result = 0;
        }
        cx.notify();
    }

    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        let intelligence = self.intelligence.clone();
        let workspace_cwd = self.workspace_cwd.clone();
        if intelligence.is_none() && workspace_cwd.is_none() {
            self.results.clear();
            return;
        }
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        let query = self.query.text().to_owned();
        let tokio = self.tokio.clone();
        self._search_task = Some(cx.spawn(async move |this, cx| {
            let result = tokio
                .spawn_blocking(move || {
                    let intelligence = match intelligence {
                        Some(intelligence) => intelligence,
                        None => Arc::new(CodeIntelligence::for_session(workspace_cwd?).ok()?),
                    };
                    let results = intelligence.search(&query, 40);
                    Some((intelligence, results))
                })
                .await
                .ok()
                .flatten();
            let _ = this.update(cx, |this, cx| {
                if this.search_generation != generation || !this.picker_open {
                    return;
                }
                if let Some((intelligence, results)) = result {
                    this.intelligence = Some(intelligence);
                    this.results = results;
                } else {
                    this.results.clear();
                }
                this.highlighted_result = 0;
                cx.notify();
            });
        }));
    }

    /// Selects the local workspace represented by the active agent. The file
    /// index remains lazy, but the picker can now be used before a source file
    /// has been opened. Unsaved changes are retained until the user saves or
    /// reverts them; the requested workspace switch then completes.
    pub fn set_workspace(&mut self, cwd: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.workspace_cwd == cwd {
            return;
        }
        if self.must_preserve_document() {
            self.pending_transition = Some(PendingTransition::Workspace(cwd));
            cx.notify();
            return;
        }
        self.apply_workspace(cwd, cx);
    }

    fn apply_workspace(&mut self, cwd: Option<PathBuf>, cx: &mut Context<Self>) {
        let pending_changed_snapshot = self.pending_changed_snapshot.take();
        let pending_changed_message = self.pending_changed_message.take();
        self.workspace_cwd = cwd.clone();
        self.intelligence = None;
        self.tree_workspace = None;
        self.tree_workspace_state = if cwd.is_some() {
            TreeWorkspaceState::Loading
        } else {
            TreeWorkspaceState::NoWorkspace
        };
        self.changed_tree_state = ChangedTreeState::Loading;
        self.tree_model.reset();
        self.tree_focused = false;
        self.tree_query.clear();
        self.tree_scroll = UniformListScrollHandle::new();
        self.tree_generation = self.tree_generation.wrapping_add(1);
        self.tree_directory_loads = 0;
        self.tree_loading_directories.clear();
        self.tree_notice = None;
        self.pending_tree_reference = None;
        self.pending_tree_path = None;
        self._tree_reveal_task = None;
        self.state = ViewerState::Empty;
        self.scroll = UniformListScrollHandle::new();
        self.generation = self.generation.wrapping_add(1);
        self.search_generation = self.search_generation.wrapping_add(1);
        self.picker_open = false;
        self.query.clear();
        self.results.clear();
        self.highlighted_result = 0;
        self.history.clear();
        self.history_index = 0;
        self.pending_transition = None;
        if let Some(snapshot) = pending_changed_snapshot {
            let truncated = snapshot.truncated;
            self.tree_model.set_changed(snapshot);
            self.changed_tree_state = ChangedTreeState::Ready { truncated };
        } else if let Some(message) = pending_changed_message {
            self.changed_tree_state = ChangedTreeState::Unavailable(message);
        }
        if let Some(cwd) = cwd {
            self.load_tree_workspace(cwd, cx);
        } else {
            self._tree_workspace_task = None;
        }
        cx.notify();
    }

    /// Review owns Git refresh cadence; Code consumes the exact same parsed
    /// snapshot rather than invoking or interpreting a second status format.
    pub fn set_changed_loading(&mut self, cx: &mut Context<Self>) {
        if self.workspace_transition_pending() {
            return;
        }
        if !matches!(self.changed_tree_state, ChangedTreeState::Ready { .. }) {
            self.changed_tree_state = ChangedTreeState::Loading;
            cx.notify();
        }
    }

    pub fn set_changed_snapshot(&mut self, snapshot: ChangedSnapshot, cx: &mut Context<Self>) {
        if self.workspace_transition_pending() {
            self.pending_changed_snapshot = Some(snapshot);
            self.pending_changed_message = None;
            return;
        }
        let truncated = snapshot.truncated;
        self.tree_model.set_changed(snapshot);
        self.changed_tree_state = ChangedTreeState::Ready { truncated };
        if let Some(reference) = self.pending_tree_reference.take()
            && let Some(path) = self.tree_model.changed_reference(&reference)
        {
            self.reveal_tree_path(path, cx);
        }
        self.scroll_tree_selection();
        cx.notify();
    }

    pub fn set_changed_unavailable(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        let message = message.into();
        if self.workspace_transition_pending() {
            self.pending_changed_snapshot = None;
            self.pending_changed_message = Some(message);
            return;
        }
        self.changed_tree_state = ChangedTreeState::Unavailable(message);
        cx.notify();
    }

    pub fn focus_file_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tree_focused = true;
        self.picker_open = false;
        window.focus(&self.focus, cx);
        self.scroll_tree_selection();
        cx.notify();
    }

    fn load_tree_workspace(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        let generation = self.tree_generation;
        self._tree_workspace_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    FileTreeWorkspace::for_session(cwd)
                        .map(Arc::new)
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.tree_generation != generation {
                    return;
                }
                match result {
                    Ok(workspace) => {
                        if matches!(this.changed_tree_state, ChangedTreeState::Loading)
                            && let Some(message) = workspace.changed_unavailable_message()
                        {
                            this.changed_tree_state = ChangedTreeState::Unavailable(message);
                        }
                        this.tree_workspace = Some(workspace);
                        this.tree_workspace_state = TreeWorkspaceState::Ready;
                        if this.tree_model.mode() == FileTreeMode::All {
                            this.ensure_tree_root(cx);
                        }
                        if let Some(path) = this.pending_tree_path.take() {
                            this.schedule_tree_reveal(path, cx);
                        }
                    }
                    Err(message) => {
                        this.tree_workspace_state = TreeWorkspaceState::Error(message);
                    }
                }
                cx.notify();
            });
        }));
    }

    fn set_tree_mode(&mut self, mode: FileTreeMode, cx: &mut Context<Self>) {
        if self.tree_model.mode() == mode {
            return;
        }
        self.tree_model.set_mode(mode);
        self.tree_scroll = UniformListScrollHandle::new();
        self.tree_notice = None;
        if mode == FileTreeMode::All {
            self.ensure_tree_root(cx);
        }
        self.scroll_tree_selection();
        cx.notify();
    }

    fn ensure_tree_root(&mut self, cx: &mut Context<Self>) {
        if !self.tree_model.has_listing(Path::new("")) {
            self.load_tree_directory(PathBuf::new(), cx);
        }
    }

    fn load_tree_directory(&mut self, directory: PathBuf, cx: &mut Context<Self>) {
        if self.tree_model.has_listing(&directory)
            || self.tree_loading_directories.contains(&directory)
            || self.tree_directory_loads >= MAX_DIRECTORY_LOADS
        {
            return;
        }
        let Some(workspace) = self.tree_workspace.clone() else {
            if matches!(self.tree_workspace_state, TreeWorkspaceState::Ready) {
                self.tree_notice = Some("The workspace is unavailable".to_owned());
            }
            return;
        };
        self.tree_directory_loads += 1;
        self.tree_loading_directories.insert(directory.clone());
        self.tree_notice = None;
        let generation = self.tree_generation;
        cx.spawn(async move |this, cx| {
            let task_directory = directory.clone();
            let result = cx
                .background_spawn(async move {
                    workspace
                        .load_directory(&task_directory)
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.tree_generation != generation {
                    return;
                }
                this.tree_directory_loads = this.tree_directory_loads.saturating_sub(1);
                this.tree_loading_directories.remove(&directory);
                match result {
                    Ok(listing) => {
                        if listing.truncated {
                            this.tree_notice = Some(format!(
                                "{} is capped at {} entries",
                                if directory.as_os_str().is_empty() {
                                    ".".to_owned()
                                } else {
                                    directory.to_string_lossy().into_owned()
                                },
                                crate::file_tree::MAX_DIRECTORY_ENTRIES
                            ));
                        }
                        this.tree_model.insert_listing(listing);
                        this.scroll_tree_selection();
                    }
                    Err(message) => this.tree_notice = Some(message),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn reveal_tree_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.tree_query.clear();
        self.tree_model.clear_filter();
        if self.tree_model.changed_contains(&path) {
            self.tree_model.set_mode(FileTreeMode::Changed);
            self.tree_model.select_path(path);
            self.pending_tree_path = None;
            self.scroll_tree_selection();
            cx.notify();
            return;
        }
        self.tree_model.set_mode(FileTreeMode::All);
        self.schedule_tree_reveal(path, cx);
    }

    fn schedule_tree_reveal(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(workspace) = self.tree_workspace.clone() else {
            self.pending_tree_path = Some(path);
            return;
        };
        self.pending_tree_path = Some(path.clone());
        self.tree_notice = None;
        let generation = self.tree_generation;
        self._tree_reveal_task = Some(cx.spawn(async move |this, cx| {
            let reveal_path = path.clone();
            let result = cx
                .background_spawn(async move {
                    workspace
                        .reveal(&reveal_path)
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.tree_generation != generation
                    || this.pending_tree_path.as_ref() != Some(&path)
                {
                    return;
                }
                this.pending_tree_path = None;
                match result {
                    Ok(listings) => {
                        for listing in listings {
                            this.tree_model.insert_listing(listing);
                        }
                        this.tree_model.expand_ancestors(&path);
                        this.tree_model.select_path(path);
                        this.scroll_tree_selection();
                    }
                    Err(message) => this.tree_notice = Some(message),
                }
                cx.notify();
            });
        }));
    }

    fn process_tree_intent(&mut self, intent: FileTreeIntent, cx: &mut Context<Self>) {
        match intent {
            FileTreeIntent::None => {}
            FileTreeIntent::OpenFile(path) => {
                let Some(cwd) = self.workspace_cwd.clone() else {
                    return;
                };
                let reference = path.to_string_lossy().into_owned();
                self.pending_tree_reference = Some(reference.clone());
                self.open_reference_inner(cwd, reference, true, cx);
            }
            FileTreeIntent::LoadDirectory(directory) => {
                self.load_tree_directory(directory, cx);
            }
        }
        self.scroll_tree_selection();
        cx.notify();
    }

    fn scroll_tree_selection(&self) {
        if let Some(index) = self.tree_model.selected_index() {
            self.tree_scroll
                .scroll_to_item(index, ScrollStrategy::Nearest);
        }
    }

    fn must_preserve_document(&self) -> bool {
        matches!(
            &self.state,
            ViewerState::Ready(document)
                if document.is_dirty() || document.save_status == SaveStatus::Saving
        )
    }

    fn workspace_transition_pending(&self) -> bool {
        match &self.pending_transition {
            Some(PendingTransition::Workspace(cwd)) => self.workspace_cwd != *cwd,
            Some(PendingTransition::Open { cwd, .. }) => self.workspace_cwd.as_ref() != Some(cwd),
            None => false,
        }
    }

    fn open_highlighted(&mut self, cx: &mut Context<Self>) {
        let Some(hit) = self.results.get(self.highlighted_result).cloned() else {
            return;
        };
        let Some(intelligence) = &self.intelligence else {
            return;
        };
        let mut reference = hit.relative_path.to_string_lossy().into_owned();
        if let Some(line) = hit.line {
            reference.push(':');
            reference.push_str(&line.to_string());
        }
        let cwd = intelligence.workspace_root().to_path_buf();
        self.picker_open = false;
        self.query.clear();
        self.results.clear();
        self.open_reference_inner(cwd, reference, true, cx);
    }

    fn handle_tree_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        match event.keystroke.key.as_str() {
            "escape" => {
                if self.tree_query.is_empty() {
                    self.tree_focused = false;
                } else {
                    self.tree_query.clear();
                    self.tree_model.clear_filter();
                }
            }
            "tab" => self.tree_focused = false,
            "up" => self.tree_model.move_selection(-1),
            "down" => self.tree_model.move_selection(1),
            "left" => self.tree_model.collapse_selected(),
            "right" => {
                let intent = self.tree_model.expand_selected();
                self.process_tree_intent(intent, cx);
            }
            "enter" => {
                let intent = self.tree_model.activate_selected();
                self.process_tree_intent(intent, cx);
            }
            _ => {
                let Some(edit) = query_editor::edit_for(&event.keystroke) else {
                    return false;
                };
                let changed = match edit {
                    Edit::Local(local) => self.tree_query.apply(local),
                    Edit::Clipboard(ClipboardEdit::Copy) => {
                        query_editor::copy_selection(&self.tree_query, cx);
                        false
                    }
                    Edit::Clipboard(ClipboardEdit::Cut) => {
                        query_editor::cut_selection(&mut self.tree_query, cx)
                    }
                    Edit::Clipboard(ClipboardEdit::Paste) => cx
                        .read_from_clipboard()
                        .and_then(|item| item.text())
                        .is_some_and(|text| self.tree_query.insert(&text)),
                };
                if changed {
                    self.tree_model.set_filter(self.tree_query.text());
                }
            }
        }
        self.scroll_tree_selection();
        cx.notify();
        true
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.platform
            && event.keystroke.modifiers.shift
            && event.keystroke.key == "e"
        {
            self.focus_file_tree(_window, cx);
            cx.stop_propagation();
            return;
        }
        if event.keystroke.modifiers.platform && event.keystroke.key == "s" {
            if matches!(self.state, ViewerState::Ready(_)) {
                self.save_document(cx);
                cx.stop_propagation();
            }
            return;
        }

        if self.tree_focused && self.handle_tree_key_down(event, cx) {
            cx.stop_propagation();
            return;
        }

        if self.picker_open {
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.picker_open = false;
                    self.query.clear();
                    self.results.clear();
                    cx.notify();
                }
                "up" => {
                    self.highlighted_result = self.highlighted_result.saturating_sub(1);
                    cx.notify();
                }
                "down" => {
                    self.highlighted_result =
                        (self.highlighted_result + 1).min(self.results.len().saturating_sub(1));
                    cx.notify();
                }
                "enter" => self.open_highlighted(cx),
                _ => {
                    let Some(edit) = query_editor::edit_for(&event.keystroke) else {
                        return;
                    };
                    let changed = match edit {
                        Edit::Local(local) => self.query.apply(local),
                        Edit::Clipboard(ClipboardEdit::Copy) => {
                            query_editor::copy_selection(&self.query, cx);
                            false
                        }
                        Edit::Clipboard(ClipboardEdit::Cut) => {
                            query_editor::cut_selection(&mut self.query, cx)
                        }
                        Edit::Clipboard(ClipboardEdit::Paste) => cx
                            .read_from_clipboard()
                            .and_then(|item| item.text())
                            .is_some_and(|text| self.query.insert(&text)),
                    };
                    if changed {
                        self.schedule_search(cx);
                    } else {
                        cx.notify();
                    }
                }
            }
            cx.stop_propagation();
            return;
        }

        let clipboard_text = (event.keystroke.modifiers.platform && event.keystroke.key == "v")
            .then(|| cx.read_from_clipboard().and_then(|item| item.text()))
            .flatten();
        let Some(document) = (match &mut self.state {
            ViewerState::Ready(document) => Some(document),
            _ => None,
        }) else {
            return;
        };

        match event.keystroke.key.as_str() {
            "enter" if !event.keystroke.modifiers.platform => {
                document.insert_newline();
            }
            "tab" if !event.keystroke.modifiers.platform => {
                document.insert("\t");
            }
            "up" => document.move_vertical(-1, event.keystroke.modifiers.shift),
            "down" => document.move_vertical(1, event.keystroke.modifiers.shift),
            _ => {
                let Some(edit) = query_editor::edit_for(&event.keystroke) else {
                    return;
                };
                match edit {
                    Edit::Local(local) => {
                        document.apply_local(local);
                    }
                    Edit::Clipboard(ClipboardEdit::Copy) => {
                        query_editor::copy_selection(&document.editor, cx);
                    }
                    Edit::Clipboard(ClipboardEdit::Cut) => {
                        if query_editor::cut_selection(&mut document.editor, cx) {
                            document.changed();
                        }
                    }
                    Edit::Clipboard(ClipboardEdit::Paste) => {
                        if let Some(text) = clipboard_text {
                            document.insert(&text);
                        }
                    }
                }
            }
        }
        self.reveal_caret();
        if let ViewerState::Ready(document) = &self.state {
            self.scroll
                .scroll_to_item(document.line_index_for_cursor(), ScrollStrategy::Nearest);
        }
        cx.notify();
        cx.stop_propagation();
    }

    fn save_document(&mut self, cx: &mut Context<Self>) {
        let Some(intelligence) = self.intelligence.clone() else {
            return;
        };
        let (snapshot, expected_text, new_text, revision) = match &mut self.state {
            ViewerState::Ready(document)
                if document.is_dirty() && document.save_status != SaveStatus::Saving =>
            {
                document.save_status = SaveStatus::Saving;
                (
                    document.snapshot.clone(),
                    document.saved_text.clone(),
                    document.editor.text().to_owned(),
                    document.revision,
                )
            }
            ViewerState::Ready(_) => {
                self.perform_pending_transition(cx);
                return;
            }
            _ => return,
        };
        cx.notify();

        let tokio = self.tokio.clone();
        let saved_path = snapshot.absolute_path.clone();
        self._save_task = Some(cx.spawn(async move |this, cx| {
            let saved_text = new_text.clone();
            let result = tokio
                .spawn_blocking(move || {
                    intelligence.save_source(&snapshot, &expected_text, &new_text)
                })
                .await
                .map_err(|error| format!("Code save stopped: {error}"))
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update(cx, |this, cx| {
                let mut should_transition = false;
                if let ViewerState::Ready(document) = &mut this.state
                    && document.snapshot.absolute_path == saved_path
                {
                    match result {
                        Ok(()) => {
                            document.saved_text = saved_text;
                            document.save_status =
                                if document.revision == revision && !document.is_dirty() {
                                    should_transition = true;
                                    SaveStatus::Saved
                                } else {
                                    SaveStatus::Idle
                                };
                        }
                        Err(message) => document.save_status = SaveStatus::Error(message),
                    }
                }
                if should_transition {
                    this.perform_pending_transition(cx);
                }
                cx.notify();
            });
        }));
    }

    fn revert_document(&mut self, cx: &mut Context<Self>) {
        if let ViewerState::Ready(document) = &mut self.state
            && document.save_status != SaveStatus::Saving
        {
            document.revert();
            self.perform_pending_transition(cx);
            cx.notify();
        }
    }

    fn place_cursor(
        &mut self,
        line_index: usize,
        local_offset: usize,
        extend: bool,
        click_count: usize,
        cx: &mut Context<Self>,
    ) {
        let ViewerState::Ready(document) = &mut self.state else {
            return;
        };
        let Some(line_range) = document
            .lines
            .get(line_index)
            .map(|line| line.range.clone())
        else {
            return;
        };
        let local_offset = local_offset.min(line_range.len());
        match click_count {
            2 => {
                let word = word_range_at(&document.editor.text()[line_range.clone()], local_offset);
                if let Some(word) = word {
                    document.set_cursor(line_range.start + word.start, false);
                    document.set_cursor(line_range.start + word.end, true);
                } else {
                    document.set_cursor(line_range.start + local_offset, extend);
                }
            }
            3.. => {
                let selection_end = document
                    .lines
                    .get(line_index + 1)
                    .map_or(line_range.end, |next| next.range.start);
                document.set_cursor(line_range.start, false);
                document.set_cursor(selection_end, true);
            }
            _ => document.set_cursor(line_range.start + local_offset, extend),
        }
        self.reveal_caret();
        cx.notify();
    }

    /// Opens a terminal-shaped reference relative to a session cwd. All path
    /// safety and parsing stay behind `CodeIntelligence`'s interface.
    pub fn open_reference(
        &mut self,
        cwd: impl Into<PathBuf>,
        reference: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let cwd = cwd.into();
        let reference = reference.into();
        if self.workspace_cwd.as_ref() != Some(&cwd) {
            self.set_workspace(Some(cwd.clone()), cx);
        }
        self.pending_tree_reference = Some(reference.clone());
        if let Some(path) = self.tree_model.changed_reference(&reference) {
            self.reveal_tree_path(path, cx);
        }
        self.open_reference_inner(cwd, reference, true, cx);
    }

    fn open_reference_inner(
        &mut self,
        cwd: PathBuf,
        reference: String,
        record_history: bool,
        cx: &mut Context<Self>,
    ) {
        if self.must_preserve_document() {
            self.pending_transition = Some(PendingTransition::Open {
                cwd,
                reference,
                record_history,
            });
            self.picker_open = false;
            self.query.clear();
            self.results.clear();
            cx.notify();
            return;
        }
        self.begin_open_reference(cwd, reference, record_history, cx);
    }

    fn begin_open_reference(
        &mut self,
        cwd: PathBuf,
        reference: String,
        record_history: bool,
        cx: &mut Context<Self>,
    ) {
        self.pending_transition = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.scroll = UniformListScrollHandle::new();
        self.state = ViewerState::Loading {
            reference: reference.clone(),
        };
        cx.notify();

        let tokio = self.tokio.clone();
        let history_cwd = cwd.clone();
        self._load_task = Some(cx.spawn(async move |this, cx| {
            let task_reference = reference.clone();
            let result = tokio
                .spawn_blocking(move || -> Result<_, CodeIntelligenceError> {
                    let intelligence = CodeIntelligence::for_session(&cwd)?;
                    let snapshot = intelligence.open_reference(&task_reference)?;
                    Ok((intelligence, snapshot))
                })
                .await
                .map_err(|error| format!("Code viewer stopped: {error}"))
                .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok((intelligence, snapshot)) => {
                        this.workspace_cwd = Some(history_cwd.clone());
                        this.intelligence = Some(Arc::new(intelligence));
                        let revealed_path = snapshot.relative_path.clone();
                        let target_line = snapshot.target.map(|target| target.line);
                        if record_history {
                            if this.history_index + 1 < this.history.len() {
                                this.history.truncate(this.history_index + 1);
                            }
                            let should_push =
                                this.history.last().is_none_or(|(current, current_ref)| {
                                    current != &history_cwd || current_ref != &reference
                                });
                            if should_push {
                                this.history.push((history_cwd, reference));
                                this.history_index = this.history.len().saturating_sub(1);
                            }
                        }
                        let document = SourceDocument::new(snapshot);
                        if let Some(line) = target_line {
                            let logical_line = line.saturating_sub(1);
                            let item = if this.word_wrap {
                                source_visual_index(&document, logical_line, this.wrap_columns())
                            } else {
                                logical_line
                            };
                            this.scroll.scroll_to_item(item, ScrollStrategy::Center);
                        }
                        this.state = ViewerState::Ready(Box::new(document));
                        this.reveal_tree_path(revealed_path, cx);
                    }
                    Err(message) => {
                        this.state = ViewerState::Error { reference, message };
                    }
                }
                cx.notify();
            });
        }));
    }

    fn perform_pending_transition(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_transition.take() else {
            return;
        };
        match pending {
            PendingTransition::Workspace(cwd) => self.apply_workspace(cwd, cx),
            PendingTransition::Open {
                cwd,
                reference,
                record_history,
            } => {
                if self.workspace_cwd.as_ref() != Some(&cwd) {
                    self.apply_workspace(Some(cwd.clone()), cx);
                }
                self.begin_open_reference(cwd, reference, record_history, cx);
            }
        }
    }

    fn navigate(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.history.is_empty() {
            return;
        }
        let next = self
            .history_index
            .saturating_add_signed(delta)
            .min(self.history.len() - 1);
        if next == self.history_index {
            return;
        }
        self.history_index = next;
        let (cwd, reference) = self.history[next].clone();
        self.open_reference_inner(cwd, reference, false, cx);
    }

    fn render_tree_mode_option(
        &self,
        mode: FileTreeMode,
        label: String,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.tree_model.mode() == mode;
        div()
            .id(match mode {
                FileTreeMode::Changed => "code-tree-changed",
                FileTreeMode::All => "code-tree-all",
            })
            .h(px(23.0))
            .px(px(7.0))
            .flex()
            .items_center()
            .rounded(px(Radius::CHIP))
            .bg(if selected {
                colors.primary.alpha(0.10)
            } else {
                colors.primary.alpha(0.0)
            })
            .cursor_pointer()
            .hover(move |button| button.bg(colors.primary.alpha(0.08)))
            .text_size(px(9.5))
            .font_weight(if selected {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .text_color(if selected {
                colors.primary
            } else {
                colors.tertiary
            })
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.focus_file_tree(window, cx);
                this.set_tree_mode(mode, cx);
                cx.stop_propagation();
            }))
            .into_any_element()
    }

    fn render_file_tree(
        &self,
        colors: SemanticColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = self.tree_model.rows_arc();
        let selected = self.tree_model.selected_path().map(Path::to_path_buf);
        let tree = cx.entity();
        let row_count = rows.len();
        let body = if rows.is_empty() {
            let filtering = !self.tree_query.is_empty();
            let message = if filtering {
                "No loaded files match this filter".to_owned()
            } else {
                match self.tree_model.mode() {
                    FileTreeMode::Changed => match &self.changed_tree_state {
                        ChangedTreeState::Loading => "Reading Git status…".to_owned(),
                        ChangedTreeState::Ready { .. } => "No changed files".to_owned(),
                        ChangedTreeState::Unavailable(message) => message.clone(),
                    },
                    FileTreeMode::All => match &self.tree_workspace_state {
                        TreeWorkspaceState::NoWorkspace => "Select a local session".to_owned(),
                        TreeWorkspaceState::Loading => "Locating the workspace…".to_owned(),
                        TreeWorkspaceState::Error(message) => message.clone(),
                        TreeWorkspaceState::Ready
                            if self.tree_loading_directories.contains(Path::new("")) =>
                        {
                            "Loading repository root…".to_owned()
                        }
                        TreeWorkspaceState::Ready => "This directory is empty".to_owned(),
                    },
                }
            };
            div()
                .size_full()
                .px(px(18.0))
                .flex()
                .items_center()
                .justify_center()
                .text_center()
                .text_size(px(10.0))
                .text_color(colors.tertiary)
                .child(message)
                .into_any_element()
        } else {
            let visible_rows = Arc::clone(&rows);
            uniform_list("code-file-tree-rows", row_count, move |range, _, cx| {
                range
                    .map(|index| {
                        let row = visible_rows[index].clone();
                        let expanded = row.kind == TreeEntryKind::Directory
                            && tree.read(cx).tree_model.is_expanded(&row.relative_path);
                        render_file_tree_row(
                            row,
                            index,
                            selected.as_ref(),
                            expanded,
                            colors,
                            tree.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.tree_scroll)
            .size_full()
            .into_any_element()
        };
        let changed_label = format!("Changed {}", self.tree_model.changed_count());
        let query = if self.tree_query.is_empty() {
            div()
                .text_color(colors.tertiary)
                .child("Filter files  ·  ⌘⇧E")
                .into_any_element()
        } else {
            crate::navigation::query_label(&self.tree_query)
        };
        let root = self
            .tree_workspace
            .as_ref()
            .and_then(|workspace| workspace.root().file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Workspace".to_owned());
        let notice = self.tree_notice.clone().or_else(|| {
            matches!(
                self.changed_tree_state,
                ChangedTreeState::Ready { truncated: true }
            )
            .then(|| {
                format!(
                    "Changed files capped at {}",
                    crate::file_tree::MAX_VISIBLE_ROWS
                )
            })
        });
        let focused = self.tree_focused && self.focus.is_focused(window);

        div()
            .h(px(FILE_TREE_HEIGHT))
            .flex_none()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(colors.primary.alpha(0.09))
            .bg(colors.background)
            .child(
                div()
                    .h(px(32.0))
                    .flex_none()
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(self.render_tree_mode_option(
                        FileTreeMode::Changed,
                        changed_label,
                        colors,
                        cx,
                    ))
                    .child(self.render_tree_mode_option(
                        FileTreeMode::All,
                        "All files".to_owned(),
                        colors,
                        cx,
                    ))
                    .child(
                        div()
                            .ml(px(4.0))
                            .min_w(px(0.0))
                            .flex_1()
                            .truncate()
                            .text_right()
                            .text_size(px(9.0))
                            .text_color(colors.primary.alpha(0.28))
                            .child(root),
                    ),
            )
            .child(
                div()
                    .id("code-tree-filter")
                    .h(px(30.0))
                    .mx(px(8.0))
                    .mb(px(4.0))
                    .px(px(8.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(px(Radius::CHIP))
                    .bg(colors.primary.alpha(if focused { 0.065 } else { 0.035 }))
                    .border_1()
                    .border_color(colors.primary.alpha(if focused { 0.16 } else { 0.055 }))
                    .cursor_text()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.focus_file_tree(window, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(sf_symbol("magnifyingglass", 9.5, colors.tertiary))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .font_family(crate::fonts::mono_family())
                            .text_size(px(10.0))
                            .text_color(colors.primary)
                            .child(query),
                    ),
            )
            .child(div().min_h(px(0.0)).flex_1().overflow_hidden().child(body))
            .when_some(notice, |tree, notice| {
                tree.child(
                    div()
                        .h(px(22.0))
                        .flex_none()
                        .px(px(9.0))
                        .flex()
                        .items_center()
                        .truncate()
                        .border_t_1()
                        .border_color(colors.primary.alpha(0.055))
                        .text_size(px(8.5))
                        .text_color(Ink::ATTENTION)
                        .child(notice),
                )
            })
            .into_any_element()
    }

    fn render_toolbar(
        &self,
        document: Option<&SourceDocument>,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let can_back = !self.history.is_empty() && self.history_index > 0;
        let can_forward = self.history_index + 1 < self.history.len();
        let picker_open = self.picker_open;
        let (path, location) = document.map_or_else(
            || ("No file open".to_owned(), None),
            |document| {
                (
                    document
                        .snapshot
                        .relative_path
                        .to_string_lossy()
                        .into_owned(),
                    document.snapshot.target.map(|target| {
                        if target.column > 1 {
                            format!("{}:{}", target.line, target.column)
                        } else {
                            target.line.to_string()
                        }
                    }),
                )
            },
        );
        let dirty = document.is_some_and(SourceDocument::is_dirty);
        let saving = document.is_some_and(|document| document.save_status == SaveStatus::Saving);
        let save_status = document.and_then(|document| match &document.save_status {
            SaveStatus::Saved if !dirty => Some(("Saved".to_owned(), Ink::FRESH)),
            SaveStatus::Error(message) => Some((message.clone(), Ink::DANGER)),
            _ if self.pending_transition.is_some() => Some((
                "Save or revert to finish switching files".to_owned(),
                Ink::ATTENTION,
            )),
            _ => None,
        });
        let nav_button = |id: &'static str,
                          symbol: &'static str,
                          enabled: bool,
                          delta: isize,
                          cx: &mut Context<Self>| {
            div()
                .id(id)
                .size(px(24.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(Radius::BADGE))
                .text_color(if enabled {
                    colors.secondary
                } else {
                    colors.primary.alpha(0.20)
                })
                .when(enabled, |button| {
                    button
                        .cursor_pointer()
                        .hover(move |button| button.bg(colors.primary.alpha(0.07)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.navigate(delta, cx);
                            cx.stop_propagation();
                        }))
                })
                .child(sf_symbol_weighted(
                    symbol,
                    10.0,
                    SymbolWeight::Semibold,
                    if enabled {
                        colors.secondary
                    } else {
                        colors.primary.alpha(0.20)
                    },
                ))
        };

        div()
            .h(px(Metrics::TITLE_BAR))
            .flex_none()
            .px(px(9.0))
            .flex()
            .items_center()
            .gap(px(3.0))
            .border_b_1()
            .border_color(colors.primary.alpha(0.06))
            .child(nav_button(
                "code-history-back",
                "chevron.left",
                can_back,
                -1,
                cx,
            ))
            .child(nav_button(
                "code-history-forward",
                "chevron.right",
                can_forward,
                1,
                cx,
            ))
            .child(
                div()
                    .id("code-open-file-picker")
                    .size(px(24.0))
                    .ml(px(2.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(Radius::BADGE))
                    .bg(if picker_open {
                        colors.primary.alpha(0.09)
                    } else {
                        colors.primary.alpha(0.0)
                    })
                    .cursor_pointer()
                    .hover(move |button| button.bg(colors.primary.alpha(0.07)))
                    .child(sf_symbol("magnifyingglass", 10.5, colors.secondary))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_picker(window, cx);
                        cx.stop_propagation();
                    })),
            )
            .child(
                div()
                    .ml(px(2.0))
                    .min_w(px(0.0))
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(sf_symbol(
                        "doc.text",
                        11.5,
                        if document.is_some() {
                            colors.secondary
                        } else {
                            colors.tertiary
                        },
                    ))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .font_family(crate::fonts::mono_family())
                            .text_size(px(Typo::META.size))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.secondary)
                            .child(path),
                    ),
            )
            .when(dirty, |bar| {
                bar.child(div().size(px(6.0)).rounded_full().bg(Ink::ATTENTION))
            })
            .when_some(location, |bar, location| {
                bar.child(
                    div()
                        .px(px(6.0))
                        .h(px(20.0))
                        .flex()
                        .items_center()
                        .rounded(px(Radius::CHIP))
                        .bg(colors.primary.alpha(0.055))
                        .font_family(crate::fonts::mono_family())
                        .text_size(px(9.5))
                        .text_color(colors.tertiary)
                        .child(location),
                )
            })
            .when_some(save_status, |bar, (message, color)| {
                bar.child(
                    div()
                        .max_w(px(190.0))
                        .truncate()
                        .text_size(px(9.5))
                        .text_color(color)
                        .child(message),
                )
            })
            .when(dirty && !saving, |bar| {
                bar.child(
                    div()
                        .id("code-revert-file")
                        .h(px(23.0))
                        .px(px(7.0))
                        .flex()
                        .items_center()
                        .rounded(px(Radius::CHIP))
                        .cursor_pointer()
                        .text_size(px(10.0))
                        .text_color(colors.tertiary)
                        .hover(move |button| button.bg(colors.primary.alpha(0.07)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.revert_document(cx);
                            cx.stop_propagation();
                        }))
                        .child("Revert"),
                )
            })
            .when(dirty || saving, |bar| {
                bar.child(
                    div()
                        .id("code-save-file")
                        .h(px(23.0))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .rounded(px(Radius::CHIP))
                        .bg(if saving {
                            colors.primary.alpha(0.05)
                        } else {
                            rgba(0xd9775733)
                        })
                        .text_size(px(10.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if saving {
                            colors.tertiary
                        } else {
                            rgba(0xeda98eff)
                        })
                        .when(!saving, |button| {
                            button
                                .cursor_pointer()
                                .hover(|button| button.bg(rgba(0xd9775749)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.save_document(cx);
                                    cx.stop_propagation();
                                }))
                        })
                        .child(sf_symbol(
                            if saving {
                                "ellipsis"
                            } else {
                                "square.and.arrow.down"
                            },
                            9.5,
                            if saving {
                                colors.tertiary
                            } else {
                                rgba(0xeda98eff)
                            },
                        ))
                        .child(if saving { "Saving…" } else { "Save" }),
                )
            })
            .into_any_element()
    }

    fn render_source(
        &self,
        document: &SourceDocument,
        colors: SemanticColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = document.lines.len();
        let extension = document
            .snapshot
            .relative_path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_owned();
        let content_width = document
            .lines
            .iter()
            .map(|line| {
                document.editor.text()[line.range.clone()]
                    .trim_end()
                    .chars()
                    .count()
            })
            .max()
            .unwrap_or(0) as f32
            * 7.1
            + SOURCE_GUTTER_WIDTH
            + 24.0;
        let editor = cx.entity();
        let focused =
            self.focus.is_focused(window) && window.is_window_active() && !self.picker_open;
        let row_context = SourceRowContext {
            focused,
            extension,
            content_width: content_width.max(320.0),
            word_wrap: self.word_wrap,
            colors,
            editor: editor.clone(),
            caret_epoch: self.caret_epoch,
        };
        if self.word_wrap {
            let visual_rows = Arc::new(source_visual_rows(document, self.wrap_columns()));
            let visual_row_count = visual_rows.len();
            return uniform_list(
                "code-viewer-source-wrapped",
                visual_row_count,
                move |range, window, cx| {
                    let viewer = editor.read(cx);
                    let ViewerState::Ready(document) = &viewer.state else {
                        return Vec::new();
                    };
                    let target = document.snapshot.target.map(|target| target.line);
                    range
                        .map(|display_index| {
                            let visual = &visual_rows[display_index];
                            source_row(
                                document,
                                display_index,
                                visual,
                                target == Some(visual.line_index + 1),
                                &row_context,
                                window,
                            )
                        })
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element();
        }

        uniform_list("code-viewer-source", rows, move |range, window, cx| {
            let viewer = editor.read(cx);
            let ViewerState::Ready(document) = &viewer.state else {
                return Vec::new();
            };
            let target = document.snapshot.target.map(|target| target.line);
            range
                .map(|index| {
                    let source_len = document.editor.text()[document.lines[index].range.clone()]
                        .trim_end_matches(['\r', '\n'])
                        .len();
                    let visual = SourceVisualRow {
                        line_index: index,
                        range: 0..source_len,
                        first: true,
                        last: true,
                    };
                    source_row(
                        document,
                        index,
                        &visual,
                        target == Some(index + 1),
                        &row_context,
                        window,
                    )
                })
                .collect::<Vec<_>>()
        })
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .track_scroll(&self.scroll)
        .size_full()
        .into_any_element()
    }

    fn render_picker(&self, colors: SemanticColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.picker_open {
            return None;
        }
        let query_empty = self.query.is_empty();
        let mut results = div()
            .id("code-search-results")
            .max_h(px(330.0))
            .overflow_y_scroll()
            .py(px(4.0));
        if self.results.is_empty() {
            results = results.child(
                div()
                    .h(px(72.0))
                    .px(px(18.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(Typo::META.size))
                    .text_color(colors.tertiary)
                    .child(if self.intelligence.is_some() {
                        if query_empty {
                            "Indexing workspace…"
                        } else {
                            "No matching files or symbols"
                        }
                    } else {
                        "Open one file to establish the workspace"
                    }),
            );
        } else {
            for (index, hit) in self.results.iter().take(40).enumerate() {
                let selected = index == self.highlighted_result;
                let path = hit.relative_path.to_string_lossy().into_owned();
                let preview = hit.preview.clone();
                let symbol = hit.kind == SearchHitKind::Symbol;
                results = results.child(
                    div()
                        .id(("code-search-result", index))
                        .min_h(px(39.0))
                        .px(px(9.0))
                        .py(px(5.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(px(Radius::ROW))
                        .bg(if selected {
                            colors.primary.alpha(0.085)
                        } else {
                            colors.primary.alpha(0.0)
                        })
                        .cursor_pointer()
                        .hover(move |row| row.bg(colors.primary.alpha(0.07)))
                        .child(sf_symbol(
                            if symbol { "curlybraces" } else { "doc.text" },
                            11.5,
                            if symbol {
                                rgba(0xc792eaff)
                            } else {
                                colors.secondary
                            },
                        ))
                        .child(
                            div()
                                .min_w(px(0.0))
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(1.0))
                                .child(
                                    div()
                                        .truncate()
                                        .font_family(crate::fonts::mono_family())
                                        .text_size(px(10.5))
                                        .font_weight(if symbol {
                                            FontWeight::MEDIUM
                                        } else {
                                            FontWeight::NORMAL
                                        })
                                        .text_color(colors.primary)
                                        .child(preview),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .font_family(crate::fonts::mono_family())
                                        .text_size(px(9.0))
                                        .text_color(colors.tertiary)
                                        .child(path),
                                ),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.highlighted_result = index;
                            this.open_highlighted(cx);
                            cx.stop_propagation();
                        })),
                );
            }
        }

        let query = if query_empty {
            div()
                .text_color(colors.tertiary)
                .child("Search files and symbols…")
                .into_any_element()
        } else {
            crate::navigation::query_label(&self.query)
        };
        Some(
            div()
                .absolute()
                .top(px(40.0))
                .left(px(8.0))
                .right(px(8.0))
                .occlude()
                .child(FloatingSurface::new(
                    colors,
                    div()
                        .rounded(px(Radius::PANEL))
                        .overflow_hidden()
                        .child(
                            div()
                                .id("code-search-input")
                                .h(px(Metrics::TITLE_BAR))
                                .px(px(10.0))
                                .flex()
                                .items_center()
                                .gap(px(7.0))
                                .border_b_1()
                                .border_color(colors.primary.alpha(0.08))
                                .cursor_text()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        window.focus(&this.focus, cx);
                                        cx.stop_propagation();
                                    }),
                                )
                                .child(sf_symbol("magnifyingglass", 11.0, colors.tertiary))
                                .child(
                                    div()
                                        .min_w(px(0.0))
                                        .flex_1()
                                        .font_family(crate::fonts::mono_family())
                                        .text_size(px(11.0))
                                        .text_color(colors.primary)
                                        .child(query),
                                ),
                        )
                        .child(results),
                ))
                .into_any_element(),
        )
    }

    fn render_message(
        &self,
        colors: SemanticColors,
        symbol: &'static str,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
    ) -> AnyElement {
        div()
            .size_full()
            .px(px(28.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .text_center()
            .child(sf_symbol(symbol, 28.0, colors.tertiary))
            .child(
                div()
                    .text_size(px(Typo::ROW_EMPHASIZED.size))
                    .font_weight(Typo::ROW_EMPHASIZED.weight)
                    .text_color(colors.primary.alpha(0.88))
                    .child(title.into()),
            )
            .child(
                div()
                    .max_w(px(300.0))
                    .text_size(px(Typo::META.size))
                    .text_color(colors.tertiary)
                    .child(body.into()),
            )
            .into_any_element()
    }
}

impl Focusable for CodeViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn render_file_tree_row(
    row: VisibleTreeRow,
    index: usize,
    selected: Option<&PathBuf>,
    expanded: bool,
    colors: SemanticColors,
    tree: gpui::Entity<CodeViewer>,
) -> AnyElement {
    let is_selected = selected == Some(&row.relative_path);
    let path = row.relative_path.clone();
    let kind = row.kind;
    let status = row.status.clone();
    let status_badge = status.as_ref().map(|status| {
        let color = file_status_color(status.primary_kind());
        div()
            .flex_none()
            .font_family(crate::fonts::mono_family())
            .text_size(px(8.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color)
            .child(status.decoration())
            .into_any_element()
    });
    div()
        .id(("code-file-tree-row", index))
        .h(px(FILE_TREE_ROW_HEIGHT))
        .w_full()
        .pl(px(7.0 + row.depth as f32 * 13.0))
        .pr(px(8.0))
        .flex()
        .items_center()
        .gap(px(5.0))
        .bg(if is_selected {
            colors.primary.alpha(0.085)
        } else {
            colors.primary.alpha(0.0)
        })
        .cursor_pointer()
        .hover(move |item| item.bg(colors.primary.alpha(0.065)))
        .child(
            div()
                .w(px(10.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .when(kind == TreeEntryKind::Directory, |slot| {
                    slot.child(sf_symbol(
                        if expanded {
                            "chevron.down"
                        } else {
                            "chevron.right"
                        },
                        7.5,
                        colors.tertiary,
                    ))
                }),
        )
        .child(sf_symbol(
            if kind == TreeEntryKind::Directory {
                if expanded { "folder.fill" } else { "folder" }
            } else {
                "doc.text"
            },
            10.5,
            if kind == TreeEntryKind::Directory {
                colors.secondary
            } else {
                colors.tertiary
            },
        ))
        .child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .truncate()
                .font_family(crate::fonts::mono_family())
                .text_size(px(9.5))
                .text_color(if is_selected {
                    colors.primary
                } else {
                    colors.secondary
                })
                .child(row.label),
        )
        .when_some(status_badge, |item, badge| item.child(badge))
        .on_click(move |_, window, cx| {
            tree.update(cx, |this, cx| {
                this.focus_file_tree(window, cx);
                this.tree_model.select_path(path.clone());
                let intent = this.tree_model.activate_selected();
                this.process_tree_intent(intent, cx);
            });
            cx.stop_propagation();
        })
        .into_any_element()
}

fn file_status_color(kind: ChangeKind) -> gpui::Rgba {
    match kind {
        ChangeKind::Added => Ink::FRESH,
        ChangeKind::Deleted | ChangeKind::Unmerged => Ink::DANGER,
        ChangeKind::Renamed | ChangeKind::Copied => rgba(0xc792eaff),
        ChangeKind::Modified | ChangeKind::TypeChanged | ChangeKind::Unknown(_) => Ink::ATTENTION,
    }
}

impl Render for CodeViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = SemanticColors::dark();
        let document = match &self.state {
            ViewerState::Ready(document) => Some(document.as_ref()),
            _ => None,
        };
        let body = match &self.state {
            ViewerState::Empty => self.render_message(
                colors,
                "cursorarrow.click.2",
                "Open code from the terminal",
                "⌘-click a file path, stack frame, or compiler location to inspect it here.",
            ),
            ViewerState::Loading { reference } => self.render_message(
                colors,
                "ellipsis",
                "Opening file",
                format!("Resolving {reference}…"),
            ),
            ViewerState::Ready(document) => self.render_source(document, colors, window, cx),
            ViewerState::Error { reference, message } => self.render_message(
                colors,
                "exclamationmark.triangle",
                format!("Couldn’t open {reference}"),
                message.clone(),
            ),
        };
        let picker = self.render_picker(colors, cx);
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(colors.background)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selection_dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selection_dragging = false),
            )
            .child(self.render_file_tree(colors, window, cx))
            .child(self.render_toolbar(document, colors, cx))
            .child(div().min_h(px(0.0)).flex_1().overflow_hidden().child(body))
            .when_some(picker, |viewer, picker| viewer.child(picker))
    }
}

fn word_range_at(text: &str, offset: usize) -> Option<Range<usize>> {
    let offset = offset.min(text.len());
    let mut previous = None;
    for (start, word) in text.split_word_bound_indices() {
        let end = start + word.len();
        if !word
            .chars()
            .any(|character| character.is_alphanumeric() || character == '_')
        {
            continue;
        }
        let range = start..end;
        if start <= offset && offset < end || start == offset {
            return Some(range);
        }
        if end == offset {
            previous = Some(range);
        }
    }
    previous
}

fn source_offset_at_x(source: &str, x: gpui::Pixels, color: gpui::Rgba, window: &Window) -> usize {
    if source.is_empty() {
        return 0;
    }
    let run = TextRun {
        len: source.len(),
        font: font(crate::fonts::mono_family()),
        color: color.into(),
        ..TextRun::default()
    };
    window
        .text_system()
        .shape_line(source.to_owned().into(), px(11.5), &[run], None)
        .closest_index_for_x(x.max(px(0.0)))
}

fn source_row(
    document: &SourceDocument,
    display_index: usize,
    visual: &SourceVisualRow,
    targeted: bool,
    context: &SourceRowContext,
    window: &Window,
) -> AnyElement {
    let line_index = visual.line_index;
    let visual_range = visual.range.clone();
    let first = visual.first;
    let last = visual.last;
    let line = &document.lines[line_index];
    let full_source = document.editor.text()[line.range.clone()]
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    let selection = document.editor.selection().and_then(|selection| {
        let start = selection.start.max(line.range.start);
        let end = selection.end.min(line.range.end);
        if start >= end {
            return None;
        }
        let logical = start - line.range.start..end - line.range.start;
        let start = logical.start.max(visual_range.start);
        let end = logical.end.min(visual_range.end);
        (start < end).then_some(start..end)
    });
    let logical_caret = document.editor.cursor().saturating_sub(line.range.start);
    let caret_in_range = logical_caret >= visual_range.start
        && (logical_caret < visual_range.end || last && logical_caret == visual_range.end);
    let caret = (context.focused
        && selection.is_none()
        && document.line_index_for_cursor() == line_index
        && caret_in_range)
        .then_some(logical_caret.saturating_sub(visual_range.start));
    let source = full_source[visual_range.clone()].to_owned();
    let caret_x = caret.map(|caret| {
        if source.is_empty() {
            return px(0.0);
        }
        let run = TextRun {
            len: source.len(),
            font: font(crate::fonts::mono_family()),
            color: context.colors.primary.into(),
            ..TextRun::default()
        };
        window
            .text_system()
            .shape_line(source.clone().into(), px(11.5), &[run], None)
            .x_for_index(caret.min(source.len()))
    });
    let styled = highlighted_source_range(
        &full_source,
        visual_range.clone(),
        &context.extension,
        selection,
    );
    let bounds_slot = Rc::new(Cell::new(None));
    let paint_bounds = Rc::clone(&bounds_slot);
    let click_bounds = Rc::clone(&bounds_slot);
    let drag_bounds = Rc::clone(&bounds_slot);
    let click_editor = context.editor.clone();
    let drag_editor = context.editor.clone();
    let click_range = visual_range.clone();
    let drag_range = visual_range;
    let colors = context.colors;
    div()
        .id(display_index)
        .relative()
        .h(px(SOURCE_ROW_HEIGHT))
        .min_w(px(if context.word_wrap {
            0.0
        } else {
            context.content_width
        }))
        .w_full()
        .flex()
        .items_center()
        .cursor(CursorStyle::IBeam)
        .bg(if targeted {
            rgba(0xd977571c)
        } else {
            colors.background
        })
        .when(targeted, |row| {
            row.border_l_2().border_color(rgba(0xd97757ff))
        })
        .child(
            div()
                .w(px(SOURCE_GUTTER_WIDTH))
                .h_full()
                .flex_none()
                .pr(px(10.0))
                .flex()
                .items_center()
                .justify_end()
                .border_r_1()
                .border_color(colors.primary.alpha(0.055))
                .font_family(crate::fonts::mono_family())
                .text_size(px(10.0))
                .text_color(if targeted {
                    rgba(0xd97757ff)
                } else {
                    colors.primary.alpha(0.26)
                })
                .child(if first {
                    line.number.to_string()
                } else {
                    String::new()
                }),
        )
        .child(
            div()
                .h_full()
                .min_w(px(0.0))
                .flex_1()
                .pl(px(10.0))
                .flex()
                .items_center()
                .whitespace_nowrap()
                .font_family(crate::fonts::mono_family())
                .text_size(px(11.5))
                .text_color(rgba(0xd8dee9ff))
                .child(styled),
        )
        .when_some(caret_x, |row, caret_x| {
            row.child(
                div()
                    .id(SharedString::from(format!(
                        "code-caret-{}",
                        context.caret_epoch
                    )))
                    .absolute()
                    .left(px(SOURCE_GUTTER_WIDTH + 10.0) + caret_x)
                    .top(px(2.0))
                    .w(px(1.5))
                    .h(px(SOURCE_ROW_HEIGHT - 4.0))
                    .rounded(px(0.75))
                    .bg(rgba(0xf0aa8fff))
                    .with_animation(
                        SharedString::from(format!("code-caret-blink-{}", context.caret_epoch)),
                        Animation::new(Duration::from_millis(1_000)).repeat(),
                        |caret, delta| caret.opacity(if delta < 0.5 { 1.0 } else { 0.0 }),
                    ),
            )
        })
        .on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, window, cx| {
                let Some(bounds): Option<gpui::Bounds<gpui::Pixels>> = click_bounds.get() else {
                    return;
                };
                let source = {
                    let viewer = click_editor.read(cx);
                    let ViewerState::Ready(document) = &viewer.state else {
                        return;
                    };
                    let Some(line) = document.lines.get(line_index) else {
                        return;
                    };
                    let source =
                        document.editor.text()[line.range.clone()].trim_end_matches(['\r', '\n']);
                    source[click_range.clone()].to_owned()
                };
                let x = event.position.x - bounds.left() - px(SOURCE_GUTTER_WIDTH) - px(10.0);
                let local_offset =
                    click_range.start + source_offset_at_x(&source, x, colors.primary, window);
                click_editor.update(cx, |this, cx| {
                    window.focus(&this.focus, cx);
                    this.tree_focused = false;
                    this.selection_dragging = true;
                    this.place_cursor(
                        line_index,
                        local_offset,
                        event.modifiers.shift,
                        event.click_count,
                        cx,
                    );
                });
                cx.stop_propagation();
            },
        )
        .on_mouse_move(move |event: &gpui::MouseMoveEvent, window, cx| {
            if event.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let Some(bounds): Option<gpui::Bounds<gpui::Pixels>> = drag_bounds.get() else {
                return;
            };
            let source = {
                let viewer = drag_editor.read(cx);
                if !viewer.selection_dragging {
                    return;
                }
                let ViewerState::Ready(document) = &viewer.state else {
                    return;
                };
                let Some(line) = document.lines.get(line_index) else {
                    return;
                };
                let source =
                    document.editor.text()[line.range.clone()].trim_end_matches(['\r', '\n']);
                source[drag_range.clone()].to_owned()
            };
            let x = event.position.x - bounds.left() - px(SOURCE_GUTTER_WIDTH) - px(10.0);
            let local_offset =
                drag_range.start + source_offset_at_x(&source, x, colors.primary, window);
            drag_editor.update(cx, |this, cx| {
                this.place_cursor(line_index, local_offset, true, 1, cx);
            });
            cx.stop_propagation();
        })
        .child(
            canvas(
                move |bounds, _, _| paint_bounds.set(Some(bounds)),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .into_any_element()
}

fn highlighted_source_range(
    source: &str,
    visual_range: Range<usize>,
    extension: &str,
    selection: Option<Range<usize>>,
) -> AnyElement {
    let mut ranges = lexical_highlights(source, extension);
    ranges = ranges
        .into_iter()
        .filter_map(|(range, style)| {
            let start = range.start.max(visual_range.start);
            let end = range.end.min(visual_range.end);
            (start < end).then_some((start - visual_range.start..end - visual_range.start, style))
        })
        .collect();
    if let Some(selection) = selection {
        let selection = selection.start - visual_range.start..selection.end - visual_range.start;
        ranges.retain(|(range, _)| range.end <= selection.start || range.start >= selection.end);
        ranges.push((
            selection,
            HighlightStyle {
                background_color: Some(rgba(0xd9775766).into()),
                ..HighlightStyle::default()
            },
        ));
        ranges.sort_by_key(|(range, _)| range.start);
    }
    let source = source[visual_range].to_owned();
    if ranges.is_empty() {
        return div().child(source).into_any_element();
    }
    StyledText::new(source)
        .with_highlights(ranges)
        .into_any_element()
}

fn lexical_highlights(source: &str, extension: &str) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut ranges = Vec::new();
    let comment_start = match extension {
        "py" | "rb" | "sh" | "bash" | "zsh" | "fish" | "toml" | "yaml" | "yml" => source.find('#'),
        "sql" => source.find("--"),
        _ => source.find("//"),
    };
    let code_end = comment_start.unwrap_or(source.len());
    if let Some(start) = comment_start {
        ranges.push((
            start..source.len(),
            HighlightStyle {
                color: Some(rgba(0x718096ff).into()),
                font_style: Some(gpui::FontStyle::Italic),
                ..HighlightStyle::default()
            },
        ));
    }

    let bytes = source.as_bytes();
    let mut cursor = 0;
    while cursor < code_end {
        let quote = bytes[cursor];
        if quote != b'"' && quote != b'\'' && quote != b'`' {
            cursor += 1;
            continue;
        }
        let start = cursor;
        cursor += 1;
        while cursor < code_end {
            if bytes[cursor] == b'\\' {
                cursor = (cursor + 2).min(code_end);
            } else if bytes[cursor] == quote {
                cursor += 1;
                break;
            } else {
                cursor += 1;
            }
        }
        ranges.push((
            start..cursor,
            HighlightStyle {
                color: Some(rgba(0xd7ba7dff).into()),
                ..HighlightStyle::default()
            },
        ));
    }

    let keywords = match extension {
        "rs" => RUST_KEYWORDS,
        "swift" => SWIFT_KEYWORDS,
        "py" => PYTHON_KEYWORDS,
        "js" | "jsx" | "ts" | "tsx" => JS_KEYWORDS,
        _ => COMMON_KEYWORDS,
    };
    for keyword in keywords {
        for (start, _) in source[..code_end].match_indices(keyword) {
            let end = start + keyword.len();
            let left_ok = start == 0 || !is_ident(source.as_bytes()[start - 1]);
            let right_ok = end == code_end || !is_ident(source.as_bytes()[end]);
            if left_ok && right_ok && !ranges.iter().any(|(range, _)| range.contains(&start)) {
                ranges.push((
                    start..end,
                    HighlightStyle {
                        color: Some(rgba(0xc792eaff).into()),
                        font_weight: Some(FontWeight::MEDIUM),
                        ..HighlightStyle::default()
                    },
                ));
            }
        }
    }
    ranges.sort_by_key(|(range, _)| range.start);
    ranges
}

const fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];
const SWIFT_KEYWORDS: &[&str] = &[
    "actor",
    "async",
    "await",
    "case",
    "class",
    "defer",
    "else",
    "enum",
    "extension",
    "false",
    "for",
    "func",
    "guard",
    "if",
    "import",
    "in",
    "init",
    "let",
    "nil",
    "protocol",
    "return",
    "self",
    "static",
    "struct",
    "switch",
    "throw",
    "true",
    "try",
    "var",
    "while",
];
const PYTHON_KEYWORDS: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "None", "not", "or", "pass", "raise", "return", "True", "try", "while", "with",
    "yield",
];
const JS_KEYWORDS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "while",
    "yield",
];
const COMMON_KEYWORDS: &[&str] = &[
    "class", "const", "else", "enum", "false", "for", "function", "if", "import", "let", "null",
    "return", "static", "struct", "true", "type", "var", "while",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str, target: Option<SourceTarget>) -> SourceDocument {
        SourceDocument::new(SourceSnapshot {
            absolute_path: PathBuf::from("/workspace/src/main.rs"),
            relative_path: PathBuf::from("src/main.rs"),
            language: crate::code_intelligence::SourceLanguage::Rust,
            text: text.to_owned(),
            lines: source_lines(text),
            target,
        })
    }

    #[test]
    fn highlights_keywords_strings_and_comments_without_overlapping() {
        let source = "pub fn main() { let value = \"hello\"; // note";
        let ranges = lexical_highlights(source, "rs");
        assert!(
            ranges
                .iter()
                .any(|(range, _)| &source[range.clone()] == "pub")
        );
        assert!(
            ranges
                .iter()
                .any(|(range, _)| &source[range.clone()] == "fn")
        );
        assert!(
            ranges
                .iter()
                .any(|(range, _)| &source[range.clone()] == "\"hello\"")
        );
        assert!(
            ranges
                .iter()
                .any(|(range, _)| &source[range.clone()] == "// note")
        );
    }

    #[test]
    fn keyword_boundaries_do_not_color_identifiers() {
        let source = "format for before";
        let ranges = lexical_highlights(source, "rs");
        let words: Vec<_> = ranges
            .iter()
            .map(|(range, _)| &source[range.clone()])
            .collect();
        assert_eq!(words, vec!["for"]);
    }

    #[test]
    fn word_selection_uses_identifier_and_unicode_boundaries() {
        let source = "alpha_beta café";
        assert_eq!(word_range_at(source, 4), Some(0..10));
        assert_eq!(word_range_at(source, 10), Some(0..10));
        assert_eq!(word_range_at(source, 12), Some(11..16));
        assert_eq!(word_range_at(source, 11), Some(11..16));
    }

    #[test]
    fn word_selection_ignores_whitespace_and_punctuation() {
        let source = "alpha  +  beta";
        assert_eq!(word_range_at(source, 7), None);
        assert_eq!(word_range_at(source, 8), None);
    }

    #[test]
    fn word_wrap_ranges_preserve_text_and_utf8_boundaries() {
        let source = "alpha beta café_and_a_very_long_token";
        let ranges = word_wrap_ranges(source, 8);
        let rebuilt = ranges
            .iter()
            .map(|range| &source[range.clone()])
            .collect::<String>();

        assert_eq!(rebuilt, source);
        assert!(ranges.len() > 1);
        assert!(
            ranges
                .iter()
                .all(|range| source.is_char_boundary(range.start))
        );
        assert!(
            ranges
                .iter()
                .all(|range| source.is_char_boundary(range.end))
        );
        assert!(
            ranges
                .iter()
                .all(|range| source[range.clone()].chars().count() <= 8)
        );
    }

    #[test]
    fn word_wrap_keeps_indentation_with_the_first_code_fragment() {
        let source = "        alpha_beta_gamma";
        let ranges = word_wrap_ranges(source, 12);

        assert!(ranges.len() > 1);
        assert_ne!(&source[ranges[0].clone()], "        ");
        assert_eq!(
            ranges
                .iter()
                .map(|range| &source[range.clone()])
                .collect::<String>(),
            source
        );
    }

    #[test]
    fn wrapped_source_rows_keep_logical_line_identity() {
        let document = document("let short = 1;\nlet longer = alpha_beta_gamma;", None);
        let rows = source_visual_rows(&document, 10);

        assert_eq!(rows[0].line_index, 0);
        assert!(rows.iter().filter(|row| row.line_index == 1).count() > 1);
        assert_eq!(source_visual_index(&document, 1, 10), 2);
    }

    #[test]
    fn target_type_is_one_based() {
        let target = SourceTarget {
            line: 12,
            column: 4,
        };
        assert_eq!((target.line, target.column), (12, 4));
    }

    #[test]
    fn source_document_edits_at_the_target_and_preserves_indentation() {
        let mut document = document(
            "fn main() {\n    call();\n}\n",
            Some(SourceTarget { line: 2, column: 5 }),
        );
        assert_eq!(document.editor.cursor(), "fn main() {\n    ".len());
        document.insert("await ");
        assert!(document.editor.text().contains("    await call();"));
        assert!(document.is_dirty());

        document.apply_local(LocalEdit::MoveRight(Motion::Line, false));
        document.insert_newline();
        assert!(document.editor.text().contains("await call();\n    \n}"));
    }

    #[test]
    fn source_document_moves_and_deletes_across_crlf_as_one_line_break() {
        let mut document = document("one\r\ntwo", None);
        document.editor.move_to(5, false);
        document.apply_local(LocalEdit::DeleteBackward(Motion::Character));
        assert_eq!(document.editor.text(), "onetwo");
        assert_eq!(document.editor.cursor(), 3);
    }

    #[test]
    fn vertical_motion_keeps_the_preferred_grapheme_column() {
        let mut document = document("abcd\nx\nwxyz", None);
        document.editor.move_to(4, false);
        document.move_vertical(1, false);
        assert_eq!(document.editor.cursor(), 6);
        document.move_vertical(1, false);
        assert_eq!(document.editor.cursor(), document.editor.text().len());
    }
}
