//! Native code editor for the trailing workbench.
//!
//! `code_intelligence` owns filesystem discovery, containment and loading.
//! This module owns editing, asynchronous opens and saves, source history,
//! line targeting, virtualization, and lightweight lexical color.

use std::cell::Cell;
use std::ops::Range;
use std::path::PathBuf;
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
use crate::macos::sf_symbols::{SymbolWeight, sf_symbol, sf_symbol_weighted};
use crate::query_editor::{self, ClipboardEdit, Edit, LocalEdit, Motion, QueryEditor};
use zeus_ui::{FloatingSurface, Ink, Metrics, Radius, SemanticColors, Typo};

#[cfg(test)]
use crate::code_intelligence::SourceTarget;

const SOURCE_ROW_HEIGHT: f32 = 20.0;
const SOURCE_GUTTER_WIDTH: f32 = 52.0;

enum ViewerState {
    Empty,
    Loading { reference: String },
    Ready(Box<SourceDocument>),
    Error { reference: String, message: String },
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
    colors: SemanticColors,
    editor: gpui::Entity<CodeViewer>,
    caret_epoch: u64,
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

pub struct CodeViewer {
    tokio: tokio::runtime::Handle,
    focus: FocusHandle,
    workspace_cwd: Option<PathBuf>,
    intelligence: Option<Arc<CodeIntelligence>>,
    state: ViewerState,
    scroll: UniformListScrollHandle,
    generation: u64,
    _load_task: Option<Task<()>>,
    _search_task: Option<Task<()>>,
    _save_task: Option<Task<()>>,
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
}

impl CodeViewer {
    pub fn new(tokio: tokio::runtime::Handle, cx: &mut Context<Self>) -> Self {
        Self {
            tokio,
            focus: cx.focus_handle(),
            workspace_cwd: None,
            intelligence: None,
            state: ViewerState::Empty,
            scroll: UniformListScrollHandle::new(),
            generation: 0,
            _load_task: None,
            _search_task: None,
            _save_task: None,
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
        }
    }

    fn reveal_caret(&mut self) {
        self.caret_epoch = self.caret_epoch.wrapping_add(1);
    }

    fn toggle_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_open = !self.picker_open;
        if self.picker_open {
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
        self.workspace_cwd = cwd;
        self.intelligence = None;
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
        cx.notify();
    }

    fn must_preserve_document(&self) -> bool {
        matches!(
            &self.state,
            ViewerState::Ready(document)
                if document.is_dirty() || document.save_status == SaveStatus::Saving
        )
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

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.platform && event.keystroke.key == "s" {
            if matches!(self.state, ViewerState::Ready(_)) {
                self.save_document(cx);
                cx.stop_propagation();
            }
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
                        this.state = ViewerState::Ready(Box::new(SourceDocument::new(snapshot)));
                        if let Some(line) = target_line {
                            this.scroll
                                .scroll_to_item(line.saturating_sub(1), ScrollStrategy::Center);
                        }
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
            } => self.begin_open_reference(cwd, reference, record_history, cx),
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
            colors,
            editor: editor.clone(),
            caret_epoch: self.caret_epoch,
        };
        uniform_list("code-viewer-source", rows, move |range, window, cx| {
            let viewer = editor.read(cx);
            let ViewerState::Ready(document) = &viewer.state else {
                return Vec::new();
            };
            let target = document.snapshot.target.map(|target| target.line);
            range
                .map(|index| {
                    source_row(
                        document,
                        index,
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
    index: usize,
    targeted: bool,
    context: &SourceRowContext,
    window: &Window,
) -> AnyElement {
    let line = &document.lines[index];
    let source = document.editor.text()[line.range.clone()]
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    let selection = document.editor.selection().and_then(|selection| {
        let start = selection.start.max(line.range.start);
        let end = selection.end.min(line.range.end);
        (start < end).then_some(start - line.range.start..end - line.range.start)
    });
    let caret =
        (context.focused && selection.is_none() && document.line_index_for_cursor() == index)
            .then_some(document.editor.cursor().saturating_sub(line.range.start));
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
    let styled = highlighted_source(source, &context.extension, selection);
    let bounds_slot = Rc::new(Cell::new(None));
    let paint_bounds = Rc::clone(&bounds_slot);
    let click_bounds = Rc::clone(&bounds_slot);
    let drag_bounds = Rc::clone(&bounds_slot);
    let click_editor = context.editor.clone();
    let drag_editor = context.editor.clone();
    let colors = context.colors;
    div()
        .id(index)
        .relative()
        .h(px(SOURCE_ROW_HEIGHT))
        .min_w(px(context.content_width))
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
                .child(line.number.to_string()),
        )
        .child(
            div()
                .h_full()
                .min_w(px(0.0))
                .pl(px(10.0))
                .flex()
                .items_center()
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
                    let Some(line) = document.lines.get(index) else {
                        return;
                    };
                    document.editor.text()[line.range.clone()].to_owned()
                };
                let x = event.position.x - bounds.left() - px(SOURCE_GUTTER_WIDTH) - px(10.0);
                let local_offset = source_offset_at_x(&source, x, colors.primary, window);
                click_editor.update(cx, |this, cx| {
                    window.focus(&this.focus, cx);
                    this.selection_dragging = true;
                    this.place_cursor(
                        index,
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
                let Some(line) = document.lines.get(index) else {
                    return;
                };
                document.editor.text()[line.range.clone()].to_owned()
            };
            let x = event.position.x - bounds.left() - px(SOURCE_GUTTER_WIDTH) - px(10.0);
            let local_offset = source_offset_at_x(&source, x, colors.primary, window);
            drag_editor.update(cx, |this, cx| {
                this.place_cursor(index, local_offset, true, 1, cx);
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

fn highlighted_source(
    source: String,
    extension: &str,
    selection: Option<Range<usize>>,
) -> AnyElement {
    let mut ranges = lexical_highlights(&source, extension);
    if let Some(selection) = selection {
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
