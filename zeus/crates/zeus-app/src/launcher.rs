//! Compact new-session destination opened in the main pane by Command-N.

use std::path::Path;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle,
    KeyDownEvent, MouseButton, PathPromptOptions, Render, Task, Window, div, prelude::*, px, rgba,
};
use zeus_proto::{AgentKind, Project};
use zeus_ui::{
    AgentKind as UiAgentKind, AgentLogo, Fill, FloatingSurface, Palette, Radius, SemanticColors,
};

use crate::AppServices;
use crate::agent_catalog::{AgentOption, default_agent_options, title_case_id};
use crate::composer::PromptComposer;
use crate::macos::sf_symbols::{SymbolWeight, sf_symbol, sf_symbol_weighted};
use crate::navigation::CARET;
use crate::query_editor::{self, ClipboardEdit, Edit, QueryEditor};
use crate::store::{
    LaunchRecipe, RecipeIssue, RecipeProject, RecipeRepair, RecipeWorktree, launch_project_ref,
};

const PANEL_WIDTH: f32 = 540.0;
const TITLE_HEIGHT: f32 = 30.0;
const TITLE_GAP: f32 = 14.0;
const CONTROL_SIZE: f32 = 32.0;
const CONTROL_RADIUS: f32 = 9.0;
const SHELF_HEIGHT: f32 = 34.0;
const PICKER_HEIGHT: f32 = 200.0;

/// Composer metrics. The text area is sized from the wrapped line count
/// rather than pinned at one height: a one-line prompt should not sit in a
/// half-empty box, and a twenty-line one should not vanish out of the bottom
/// of a fixed one — it grows to [`COMPOSER_MAX_LINES`] and then scrolls,
/// following the caret.
const COMPOSER_FONT_SIZE: f32 = 13.0;
const COMPOSER_LINE_HEIGHT: f32 = 19.0;
const COMPOSER_MIN_LINES: usize = 3;
const COMPOSER_MAX_LINES: usize = 9;
const COMPOSER_INSET: f32 = 8.0;
const COMPOSER_PADDING: f32 = 12.0;
const COMPOSER_PAD_TOP: f32 = 9.0;
const COMPOSER_PAD_BOTTOM: f32 = 6.0;
const COMPOSER_CONTROLS_HEIGHT: f32 = 38.0;

/// The width text actually wraps at, derived from the panel so the two cannot
/// drift apart: the panel, less the composer's margin, padding and border.
const COMPOSER_TEXT_WIDTH: f32 = PANEL_WIDTH - 2.0 * COMPOSER_INSET - 2.0 * COMPOSER_PADDING - 2.0;

const fn composer_text_height(lines: usize) -> f32 {
    let visible = if lines < COMPOSER_MIN_LINES {
        COMPOSER_MIN_LINES
    } else if lines > COMPOSER_MAX_LINES {
        COMPOSER_MAX_LINES
    } else {
        lines
    };
    visible as f32 * COMPOSER_LINE_HEIGHT + COMPOSER_PAD_TOP + COMPOSER_PAD_BOTTOM
}

pub(crate) enum LauncherEvent {
    Closed,
}

pub(crate) struct LauncherOverlay {
    services: Arc<AppServices>,
    focus: FocusHandle,
    prompt: PromptComposer,
    selected_harness: AgentKind,
    selected_root: String,
    selected_host: Option<String>,
    worktree_create: bool,
    worktree_branch: QueryEditor,
    title: QueryEditor,
    recipe_name: QueryEditor,
    loaded_recipe_id: Option<String>,
    mode: LauncherMode,
    field: LauncherField,
    /// Which picker, if any, is open — and where its keyboard highlight sits,
    /// so both are reachable without the mouse.
    picker: Option<Picker>,
    highlight: usize,
    open: bool,
    preview: bool,
    _store_changes: Task<()>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Picker {
    Harness,
    Project,
    Host,
    Recipe,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LauncherMode {
    Compose,
    SaveRecipe,
    RenameRecipe,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LauncherField {
    Prompt,
    Title,
    WorktreeBranch,
    RecipeName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProjectCommit {
    Recent(usize),
    ChooseFolder,
}

impl EventEmitter<LauncherEvent> for LauncherOverlay {}

impl LauncherOverlay {
    pub(crate) fn new(services: Arc<AppServices>, preview: bool, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let (selected_harness, selected_root) = initial_target(&services);
        let mut changes = services.store.changes();
        let store_changes = cx.spawn(async move |this, cx| {
            loop {
                match changes.recv().await {
                    Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });

        Self {
            services,
            focus,
            prompt: PromptComposer::default(),
            selected_harness,
            selected_root,
            selected_host: None,
            worktree_create: false,
            worktree_branch: QueryEditor::default(),
            title: QueryEditor::default(),
            recipe_name: QueryEditor::default(),
            loaded_recipe_id: None,
            mode: LauncherMode::Compose,
            field: LauncherField::Prompt,
            picker: None,
            highlight: 0,
            open: false,
            preview,
            _store_changes: store_changes,
        }
    }

    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A half-written prompt survives Escape. This used to clear on every
        // open, so closing the launcher by reflex — or bouncing off it to
        // check something — threw the prompt away with no way back. It is
        // cleared on submit, and only there.
        if self.prompt.is_empty() && self.loaded_recipe_id.is_none() {
            let (harness, root) = initial_target(&self.services);
            self.selected_harness = harness;
            self.selected_root = root;
            self.selected_host = None;
            self.worktree_create = false;
            self.worktree_branch.clear();
            self.title.clear();
        }
        self.picker = None;
        self.mode = LauncherMode::Compose;
        self.field = LauncherField::Prompt;
        self.open = true;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    pub(crate) fn show_recipes(&mut self) {
        self.toggle_picker(Picker::Recipe);
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.picker = None;
        self.mode = LauncherMode::Compose;
        self.field = LauncherField::Prompt;
        cx.emit(LauncherEvent::Closed);
        cx.notify();
    }

    fn harness_choices(&self) -> Vec<AgentOption> {
        let store = self
            .services
            .store
            .store
            .read()
            .expect("session store lock poisoned");
        default_agent_options(store.agent_catalog())
    }

    fn projects(&self) -> Vec<Project> {
        let store = self
            .services
            .store
            .store
            .read()
            .expect("session store lock poisoned");
        let mut projects: Vec<_> = store.projects().values().cloned().collect();
        projects.sort_by(|left, right| {
            left.pinned_order
                .unwrap_or(i64::MAX)
                .cmp(&right.pinned_order.unwrap_or(i64::MAX))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        projects
    }

    fn hosts(&self) -> Vec<zeus_proto::HostEntry> {
        self.services
            .store
            .store
            .read()
            .expect("session store lock poisoned")
            .hosts()
            .to_vec()
    }

    fn recipes(&self) -> Vec<LaunchRecipe> {
        self.services
            .store
            .store
            .read()
            .expect("session store lock poisoned")
            .launch_recipes()
            .to_vec()
    }

    fn loaded_recipe(&self) -> Option<LaunchRecipe> {
        let id = self.loaded_recipe_id.as_deref()?;
        self.recipes().into_iter().find(|recipe| recipe.id == id)
    }

    fn selected_host_label(&self) -> String {
        match self.selected_host.as_deref() {
            None => "This Mac".to_owned(),
            Some(id) => self
                .hosts()
                .into_iter()
                .find(|host| host.id == id)
                .map(|host| host.display_name().to_owned())
                .unwrap_or_else(|| id.to_owned()),
        }
    }

    fn draft_recipe(&self) -> LaunchRecipe {
        let typed_name = self.recipe_name.text().trim().to_owned();
        let loaded_name = self.loaded_recipe().map(|recipe| recipe.name);
        let store = self
            .services
            .store
            .store
            .read()
            .expect("session store lock poisoned");
        LaunchRecipe {
            id: self
                .loaded_recipe_id
                .clone()
                .unwrap_or_else(crate::store::new_recipe_id),
            name: if !typed_name.is_empty() {
                typed_name
            } else {
                loaded_name.unwrap_or_else(|| first_line_name(self.prompt.text()))
            },
            agent: self.selected_harness.clone(),
            project: launch_project_ref(
                &self.selected_root,
                self.selected_host.is_some(),
                store.projects(),
            ),
            host: self.selected_host.clone(),
            worktree: self.worktree_create.then(|| RecipeWorktree {
                create: true,
                branch: {
                    let branch = self.worktree_branch.text().trim();
                    (!branch.is_empty()).then(|| branch.to_owned())
                },
            }),
            initial_prompt: self.prompt.text().to_owned(),
            title: {
                let title = self.title.text().trim();
                (!title.is_empty()).then(|| title.to_owned())
            },
        }
    }

    fn apply_recipe(&mut self, recipe: &LaunchRecipe) {
        let store = self
            .services
            .store
            .store
            .read()
            .expect("session store lock poisoned");
        self.selected_harness = recipe.agent.clone();
        self.selected_root = match &recipe.project {
            RecipeProject::Project { id } => store
                .projects()
                .get(id)
                .map(|project| project.root.clone())
                .unwrap_or_default(),
            RecipeProject::Path { path } => path.clone(),
        };
        self.selected_host.clone_from(&recipe.host);
        self.worktree_create = recipe
            .worktree
            .as_ref()
            .is_some_and(|worktree| worktree.create)
            && recipe.host.is_none();
        self.worktree_branch.reset(
            recipe
                .worktree
                .as_ref()
                .and_then(|worktree| worktree.branch.clone())
                .unwrap_or_default(),
            usize::MAX,
        );
        self.title
            .reset(recipe.title.clone().unwrap_or_default(), usize::MAX);
        self.prompt.clear();
        if !recipe.initial_prompt.is_empty() {
            self.prompt.insert_multiline(&recipe.initial_prompt);
        }
        self.recipe_name.reset(recipe.name.clone(), usize::MAX);
        self.loaded_recipe_id = Some(recipe.id.clone());
        self.mode = LauncherMode::Compose;
        self.field = LauncherField::Prompt;
        self.picker = None;
    }

    fn clear_loaded_recipe(&mut self) {
        self.loaded_recipe_id = None;
        self.recipe_name.clear();
        self.mode = LauncherMode::Compose;
        self.field = LauncherField::Prompt;
    }

    fn diagnose_draft(&self) -> Result<(), RecipeIssue> {
        let draft = self.draft_recipe();
        self.services
            .store
            .store
            .read()
            .expect("session store lock poisoned")
            .diagnose_launch_recipe(&draft)
    }

    fn selected_harness_label(&self) -> String {
        self.harness_choices()
            .into_iter()
            .find(|choice| choice.kind == self.selected_harness)
            .map(|choice| choice.display_name)
            .unwrap_or_else(|| title_case_id(self.selected_harness.id()))
    }

    fn selected_project_label(&self) -> String {
        self.projects()
            .into_iter()
            .find(|project| project.root == self.selected_root)
            .map(|project| project.name)
            .or_else(|| {
                Path::new(&self.selected_root)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            })
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Choose project".to_owned())
    }

    /// Why the prompt cannot be sent yet, as something to show the user.
    /// `None` means it can. The submit button used to just sit there dimmed
    /// with no explanation, which reads as "broken" rather than "not yet".
    fn blocker(&self) -> Option<String> {
        if let Err(issue) = self.diagnose_draft() {
            return Some(issue.message());
        }
        if self.prompt.text().trim().is_empty() && self.loaded_recipe_id.is_none() {
            return Some("Describe the task, or launch a saved recipe".to_owned());
        }
        None
    }

    fn draft_issue(&self) -> Option<RecipeIssue> {
        self.diagnose_draft().err()
    }

    fn can_submit(&self) -> bool {
        !self.preview && self.blocker().is_none()
    }

    fn submit(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.can_submit() {
            return false;
        }
        let draft = self.draft_recipe();
        if self
            .services
            .store
            .store
            .write()
            .expect("session store lock poisoned")
            .spawn_launch_recipe(&draft, &crate::store::RecipeOverrides::default())
            .is_err()
        {
            return false;
        }
        self.prompt.clear();
        self.clear_loaded_recipe();
        self.close(cx);
        true
    }

    fn launch_stored_recipe(&mut self, recipe: &LaunchRecipe, cx: &mut Context<Self>) -> bool {
        if self.preview {
            return false;
        }
        let result = self
            .services
            .store
            .store
            .write()
            .expect("session store lock poisoned")
            .spawn_launch_recipe(recipe, &crate::store::RecipeOverrides::default());
        if result.is_err() {
            self.apply_recipe(recipe);
            cx.notify();
            return false;
        }
        self.prompt.clear();
        self.clear_loaded_recipe();
        self.close(cx);
        true
    }

    pub(crate) fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.picker.is_some() && self.handle_picker_key(event, window, cx) {
            return true;
        }
        let mods = event.keystroke.modifiers;
        if mods.platform && self.handle_command_key(event, cx) {
            return true;
        }
        if matches!(
            self.mode,
            LauncherMode::SaveRecipe | LauncherMode::RenameRecipe
        ) {
            return self.handle_name_mode_key(event, cx);
        }
        let shift = mods.shift;
        match event.keystroke.key.as_str() {
            "escape" => {
                if self.loaded_recipe_id.is_some() && self.field != LauncherField::Prompt {
                    self.field = LauncherField::Prompt;
                    cx.notify();
                    true
                } else {
                    self.close(cx);
                    true
                }
            }
            "enter" if shift => {
                self.prompt.insert_multiline("\n");
                cx.notify();
                true
            }
            "enter" => self.submit(cx),
            // Cycling the agent from the keyboard: the picker was mouse-only,
            // which is a strange thing to require of a surface you reached
            // with ⌘N and are about to leave with ↵.
            "tab" => {
                self.cycle_harness(if shift { -1 } else { 1 });
                cx.notify();
                true
            }
            "up" if self.field == LauncherField::Prompt => {
                self.prompt.move_up(shift);
                cx.notify();
                true
            }
            "down" if self.field == LauncherField::Prompt => {
                self.prompt.move_down(shift);
                cx.notify();
                true
            }
            _ => self.edit_active_field(event, cx),
        }
    }

    fn handle_command_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let shift = event.keystroke.modifiers.shift;
        match event.keystroke.key.as_str() {
            "s" if shift => {
                self.begin_save(true);
                cx.notify();
                true
            }
            "s" => {
                self.begin_save(false);
                cx.notify();
                true
            }
            "r" if !shift => {
                self.toggle_picker(Picker::Recipe);
                cx.notify();
                true
            }
            "backspace" => {
                if let Some(id) = self.highlighted_recipe_id() {
                    let _ = self
                        .services
                        .store
                        .store
                        .write()
                        .expect("session store lock poisoned")
                        .delete_launch_recipe(&id);
                    if self.loaded_recipe_id.as_deref() == Some(id.as_str()) {
                        self.clear_loaded_recipe();
                    }
                    cx.notify();
                    true
                } else {
                    false
                }
            }
            "d" => {
                if let Some(id) = self.highlighted_recipe_id() {
                    let copy = self
                        .services
                        .store
                        .store
                        .write()
                        .expect("session store lock poisoned")
                        .duplicate_launch_recipe(&id)
                        .ok()
                        .flatten();
                    if let Some(copy) = copy {
                        self.apply_recipe(&copy);
                    }
                    cx.notify();
                    true
                } else {
                    false
                }
            }
            "enter" if self.picker == Some(Picker::Recipe) => {
                if let Some(recipe) = self.highlighted_recipe() {
                    self.launch_stored_recipe(&recipe, cx)
                } else {
                    self.begin_save(self.loaded_recipe_id.is_some());
                    cx.notify();
                    true
                }
            }
            _ => false,
        }
    }

    fn handle_name_mode_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        match event.keystroke.key.as_str() {
            "escape" => {
                self.mode = LauncherMode::Compose;
                self.field = LauncherField::Prompt;
                cx.notify();
                true
            }
            "enter" => {
                self.commit_name_mode(cx);
                true
            }
            _ => {
                let handled = edit_single_line(&mut self.recipe_name, event, cx);
                if handled {
                    cx.notify();
                }
                handled
            }
        }
    }

    /// Arrow keys drive the open picker instead of the prompt behind it.
    fn handle_picker_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let count = match self.picker {
            Some(Picker::Harness) => self.harness_choices().len(),
            Some(Picker::Project) => self.projects().len() + 1,
            Some(Picker::Host) => self.hosts().len() + 1,
            Some(Picker::Recipe) => self.recipes().len() + 1,
            None => return false,
        };
        let mods = event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "escape" => {
                self.picker = None;
                cx.notify();
                true
            }
            "up" | "down" if count > 0 && mods.alt && self.picker == Some(Picker::Recipe) => {
                let delta = if event.keystroke.key == "up" { -1 } else { 1 };
                if let Some(id) = self.highlighted_recipe_id() {
                    let _ = self
                        .services
                        .store
                        .store
                        .write()
                        .expect("session store lock poisoned")
                        .reorder_launch_recipe(&id, delta);
                    self.highlight = if delta < 0 {
                        self.highlight.saturating_sub(1)
                    } else {
                        (self.highlight + 1).min(count.saturating_sub(2))
                    };
                    cx.notify();
                }
                true
            }
            "up" | "down" if count > 0 => {
                self.highlight = if event.keystroke.key == "up" {
                    self.highlight.saturating_sub(1)
                } else {
                    (self.highlight + 1).min(count - 1)
                };
                cx.notify();
                true
            }
            "enter" if mods.platform && self.picker == Some(Picker::Recipe) => {
                if let Some(recipe) = self.highlighted_recipe() {
                    self.launch_stored_recipe(&recipe, cx)
                } else {
                    false
                }
            }
            "enter" => {
                self.commit_highlight(window, cx);
                cx.notify();
                true
            }
            "r" if mods.platform && self.picker == Some(Picker::Recipe) => {
                if let Some(recipe) = self.highlighted_recipe() {
                    self.begin_rename(&recipe);
                    cx.notify();
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn commit_highlight(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.picker {
            Some(Picker::Harness) => {
                if let Some(choice) = self
                    .harness_choices()
                    .get(self.highlight)
                    .filter(|choice| choice.available)
                {
                    self.selected_harness = choice.kind.clone();
                }
            }
            Some(Picker::Project) => {
                let projects = self.projects();
                match project_commit(projects.len(), self.highlight) {
                    ProjectCommit::Recent(index) => {
                        self.selected_root.clone_from(&projects[index].root);
                        self.picker = None;
                        window.focus(&self.focus, cx);
                    }
                    ProjectCommit::ChooseFolder => {
                        self.choose_folder(window, cx);
                    }
                }
                return;
            }
            Some(Picker::Host) => {
                let next = if self.highlight == 0 {
                    None
                } else {
                    self.hosts()
                        .get(self.highlight - 1)
                        .map(|host| host.id.clone())
                };
                self.select_host(next);
            }
            Some(Picker::Recipe) => {
                if let Some(recipe) = self.highlighted_recipe() {
                    self.apply_recipe(&recipe);
                    window.focus(&self.focus, cx);
                    return;
                }
                self.begin_save(false);
                return;
            }
            None => return,
        }
        self.picker = None;
    }

    fn toggle_picker(&mut self, picker: Picker) {
        if self.picker == Some(picker) {
            self.picker = None;
            return;
        }
        self.highlight = match picker {
            Picker::Harness => self
                .harness_choices()
                .iter()
                .position(|choice| choice.kind == self.selected_harness),
            Picker::Project => self
                .projects()
                .iter()
                .position(|project| project.root == self.selected_root),
            Picker::Host => self.selected_host.as_ref().and_then(|id| {
                self.hosts()
                    .iter()
                    .position(|host| &host.id == id)
                    .map(|index| index + 1)
            }),
            Picker::Recipe => self
                .loaded_recipe_id
                .as_ref()
                .and_then(|id| self.recipes().iter().position(|recipe| &recipe.id == id)),
        }
        .unwrap_or(0);
        self.picker = Some(picker);
    }

    fn highlighted_recipe(&self) -> Option<LaunchRecipe> {
        let recipes = self.recipes();
        recipes.get(self.highlight).cloned()
    }

    fn highlighted_recipe_id(&self) -> Option<String> {
        if self.picker == Some(Picker::Recipe) {
            self.highlighted_recipe().map(|recipe| recipe.id)
        } else {
            self.loaded_recipe_id.clone()
        }
    }

    fn begin_save(&mut self, force_new: bool) {
        let name = if !force_new {
            self.loaded_recipe()
                .map(|recipe| recipe.name)
                .unwrap_or_else(|| first_line_name(self.prompt.text()))
        } else {
            first_line_name(self.prompt.text())
        };
        if force_new {
            self.loaded_recipe_id = None;
        }
        self.recipe_name.reset(name, usize::MAX);
        self.recipe_name.select_all();
        self.mode = LauncherMode::SaveRecipe;
        self.field = LauncherField::RecipeName;
        self.picker = None;
    }

    fn begin_rename(&mut self, recipe: &LaunchRecipe) {
        self.loaded_recipe_id = Some(recipe.id.clone());
        self.recipe_name.reset(recipe.name.clone(), usize::MAX);
        self.recipe_name.select_all();
        self.mode = LauncherMode::RenameRecipe;
        self.field = LauncherField::RecipeName;
        self.picker = None;
    }

    fn commit_name_mode(&mut self, cx: &mut Context<Self>) {
        let name = self.recipe_name.text().trim().to_owned();
        if name.is_empty() {
            return;
        }
        match self.mode {
            LauncherMode::RenameRecipe => {
                if let Some(id) = self.loaded_recipe_id.clone() {
                    let _ = self
                        .services
                        .store
                        .store
                        .write()
                        .expect("session store lock poisoned")
                        .update_preferences(|prefs| {
                            if let Some(recipe) = prefs
                                .launch_recipes
                                .iter_mut()
                                .find(|recipe| recipe.id == id)
                            {
                                recipe.name.clone_from(&name);
                            }
                        });
                }
            }
            LauncherMode::SaveRecipe => {
                let mut recipe = self.draft_recipe();
                recipe.name = name;
                if self.loaded_recipe_id.is_none() {
                    recipe.id = crate::store::new_recipe_id();
                }
                let id = recipe.id.clone();
                let _ = self
                    .services
                    .store
                    .store
                    .write()
                    .expect("session store lock poisoned")
                    .save_launch_recipe(recipe);
                self.loaded_recipe_id = Some(id);
            }
            LauncherMode::Compose => {}
        }
        self.mode = LauncherMode::Compose;
        self.field = LauncherField::Prompt;
        cx.notify();
    }

    fn select_host(&mut self, host: Option<String>) {
        if self.selected_host == host {
            return;
        }
        self.selected_host.clone_from(&host);
        if let Some(id) = host.as_deref() {
            self.worktree_create = false;
            self.selected_root = self
                .hosts()
                .into_iter()
                .find(|entry| entry.id == id)
                .and_then(|entry| entry.default_cwd)
                .unwrap_or_else(|| "~".to_owned());
        } else {
            let (_, root) = initial_target(&self.services);
            if !root.is_empty() {
                self.selected_root = root;
            }
        }
    }

    fn repair_issue(&mut self, issue: &RecipeIssue) {
        match issue.repair() {
            RecipeRepair::Agent => self.toggle_picker(Picker::Harness),
            RecipeRepair::Project => self.toggle_picker(Picker::Project),
            RecipeRepair::Host => self.toggle_picker(Picker::Host),
            RecipeRepair::Worktree => {
                self.worktree_create = false;
                self.picker = None;
            }
        }
    }

    /// Steps to the next installed agent, skipping any that cannot run.
    fn cycle_harness(&mut self, delta: isize) {
        let choices: Vec<_> = self
            .harness_choices()
            .into_iter()
            .filter(|choice| choice.available)
            .collect();
        if choices.is_empty() {
            return;
        }
        let current = choices
            .iter()
            .position(|choice| choice.kind == self.selected_harness)
            .unwrap_or(0);
        let count = choices.len() as isize;
        let next = (current as isize + delta).rem_euclid(count) as usize;
        self.selected_harness = choices[next].kind.clone();
    }

    fn edit_active_field(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let handled = match self.field {
            LauncherField::Prompt => return self.edit_prompt(event, cx),
            LauncherField::Title => edit_single_line(&mut self.title, event, cx),
            LauncherField::WorktreeBranch => edit_single_line(&mut self.worktree_branch, event, cx),
            LauncherField::RecipeName => edit_single_line(&mut self.recipe_name, event, cx),
        };
        if handled {
            cx.notify();
        }
        handled
    }

    fn edit_prompt(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(edit) = query_editor::edit_for(&event.keystroke) else {
            return false;
        };
        match edit {
            Edit::Local(local) => {
                self.prompt.apply(local);
            }
            Edit::Clipboard(ClipboardEdit::Copy) => {
                query_editor::copy_selection(self.prompt.editor(), cx);
            }
            Edit::Clipboard(ClipboardEdit::Cut) => {
                query_editor::cut_selection(self.prompt.editor_mut(), cx);
            }
            Edit::Clipboard(ClipboardEdit::Paste) => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.prompt.insert_multiline(&text);
                }
            }
        }
        cx.notify();
        true
    }

    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        close_picker_for_folder_choice(&mut self.picker);
        // The native sheet temporarily owns focus. Keep the composer focused
        // on both sides so a cancel or completion returns keyboard input to
        // the untouched draft.
        window.focus(&self.focus, cx);
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Start Here".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let selected = match paths.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                _ => None,
            };
            let _ = this.update_in(cx, |this, window, cx| {
                apply_folder_choice(&mut this.selected_root, selected.as_deref());
                window.focus(&this.focus, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn render_harness_picker(&self, colors: SemanticColors, cx: &mut Context<Self>) -> AnyElement {
        let mut list = div()
            .id("launcher-harness-list")
            .py(px(6.0))
            .w(px(350.0))
            .max_h(px(PICKER_HEIGHT))
            .overflow_y_scroll();
        for (index, choice) in self.harness_choices().into_iter().enumerate() {
            let selected = choice.kind == self.selected_harness;
            let enabled = choice.available;
            let highlighted = self.highlight == index;
            let kind = choice.kind.clone();
            let logo = ui_agent_kind(&choice.kind);
            let setup_url = choice.setup_url.clone();
            let unavailable = choice.unavailable_detail();
            list = list.child(
                div()
                    .id(format!("launcher-harness-{index}"))
                    .mx(px(6.0))
                    .min_h(px(42.0))
                    .px(px(9.0))
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .rounded(px(8.0))
                    .text_size(px(12.0))
                    .text_color(if enabled {
                        colors.primary
                    } else {
                        colors.tertiary
                    })
                    .when(highlighted && enabled, |row| {
                        row.bg(colors.primary.alpha(0.08))
                    })
                    .when(enabled, |row| {
                        row.cursor_pointer()
                            .hover(move |row| row.bg(colors.primary.alpha(0.06)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_harness = kind.clone();
                                this.picker = None;
                                cx.notify();
                            }))
                    })
                    .child(AgentLogo::new(logo, 21.0, colors))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(choice.display_name)
                            .when_some(unavailable, |label, unavailable| {
                                label.child(
                                    div()
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .text_size(px(9.5))
                                        .text_color(colors.tertiary)
                                        .child(unavailable),
                                )
                            }),
                    )
                    .when_some(setup_url, |row, url| {
                        row.child(
                            div()
                                .id(format!("launcher-harness-setup-{index}"))
                                .px(px(6.0))
                                .py(px(3.0))
                                .rounded(px(Radius::CHIP))
                                .cursor_pointer()
                                .text_size(px(9.5))
                                .text_color(colors.secondary)
                                .bg(Fill::subtle(colors))
                                .hover(move |button| button.bg(colors.primary.alpha(0.10)))
                                .on_click(move |_, _, cx| cx.open_url(&url))
                                .child("Setup…"),
                        )
                    })
                    .when(selected, |row| {
                        row.child(sf_symbol_weighted(
                            "checkmark",
                            9.0,
                            SymbolWeight::Semibold,
                            colors.secondary,
                        ))
                    }),
            );
        }
        FloatingSurface::new(colors, list).into_any_element()
    }

    fn render_project_picker(&self, colors: SemanticColors, cx: &mut Context<Self>) -> AnyElement {
        let projects = self.projects();
        let mut list = div()
            .id("launcher-project-list")
            .py(px(6.0))
            .w(px(310.0))
            .max_h(px(PICKER_HEIGHT))
            .overflow_y_scroll();
        for (index, project) in projects.into_iter().enumerate() {
            let selected = project.root == self.selected_root;
            let highlighted = self.highlight == index;
            let root = project.root.clone();
            list = list.child(
                div()
                    .id(format!("launcher-project-{index}"))
                    .mx(px(6.0))
                    .min_h(px(38.0))
                    .px(px(9.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .when(highlighted, |row| row.bg(colors.primary.alpha(0.08)))
                    .hover(move |row| row.bg(colors.primary.alpha(0.06)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_root.clone_from(&root);
                        this.picker = None;
                        cx.notify();
                    }))
                    .child(sf_symbol("folder", 12.0, colors.secondary))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(1.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(colors.primary)
                                    .child(project.name),
                            )
                            .child(
                                div()
                                    .text_size(px(9.0))
                                    .text_color(colors.tertiary)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(project.root),
                            ),
                    )
                    .when(selected, |row| {
                        row.child(sf_symbol_weighted(
                            "checkmark",
                            9.0,
                            SymbolWeight::Semibold,
                            colors.secondary,
                        ))
                    }),
            );
        }
        let choose_index = self.projects().len();
        let highlighted = self.highlight == choose_index;
        list = list.child(
            div()
                .id("launcher-project-choose-folder")
                .mx(px(6.0))
                .h(px(36.0))
                .px(px(9.0))
                .flex()
                .items_center()
                .gap(px(9.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .when(highlighted, |row| row.bg(colors.primary.alpha(0.08)))
                .hover(move |row| row.bg(colors.primary.alpha(0.06)))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.choose_folder(window, cx);
                }))
                .child(sf_symbol("folder.badge.plus", 12.0, colors.secondary))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(colors.primary)
                        .child("Choose Folder…"),
                ),
        );
        FloatingSurface::new(colors, list).into_any_element()
    }

    fn render_host_picker(&self, colors: SemanticColors, cx: &mut Context<Self>) -> AnyElement {
        let mut list = div()
            .id("launcher-host-list")
            .py(px(6.0))
            .w(px(280.0))
            .max_h(px(PICKER_HEIGHT))
            .overflow_y_scroll();
        let mut targets: Vec<(Option<String>, String, &'static str)> =
            vec![(None, "This Mac".to_owned(), "desktopcomputer")];
        for host in self.hosts() {
            targets.push((
                Some(host.id.clone()),
                host.display_name().to_owned(),
                "network",
            ));
        }
        for (index, (host_id, label, symbol)) in targets.into_iter().enumerate() {
            let selected = host_id.as_deref() == self.selected_host.as_deref();
            let highlighted = self.highlight == index;
            list = list.child(
                div()
                    .id(format!("launcher-host-{index}"))
                    .mx(px(6.0))
                    .h(px(34.0))
                    .px(px(9.0))
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .when(highlighted, |row| row.bg(colors.primary.alpha(0.08)))
                    .hover(move |row| row.bg(colors.primary.alpha(0.06)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_host(host_id.clone());
                        this.picker = None;
                        cx.notify();
                    }))
                    .child(sf_symbol(symbol, 12.0, colors.secondary))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .text_color(colors.primary)
                            .child(label),
                    )
                    .when(selected, |row| {
                        row.child(sf_symbol_weighted(
                            "checkmark",
                            9.0,
                            SymbolWeight::Semibold,
                            colors.secondary,
                        ))
                    }),
            );
        }
        FloatingSurface::new(colors, list).into_any_element()
    }

    fn render_recipe_picker(&self, colors: SemanticColors, cx: &mut Context<Self>) -> AnyElement {
        let recipes = self.recipes();
        let mut list = div()
            .id("launcher-recipe-list")
            .py(px(6.0))
            .w(px(360.0))
            .max_h(px(PICKER_HEIGHT))
            .overflow_y_scroll();
        let store = self
            .services
            .store
            .store
            .read()
            .expect("session store lock poisoned");
        for (index, recipe) in recipes.iter().enumerate() {
            let selected = self.loaded_recipe_id.as_deref() == Some(recipe.id.as_str());
            let highlighted = self.highlight == index;
            let issue = store.diagnose_launch_recipe(recipe).err();
            let subtitle = recipe_subtitle(recipe, issue.as_ref());
            let name = recipe.name.clone();
            let recipe = recipe.clone();
            list = list.child(
                div()
                    .id(format!("launcher-recipe-{index}"))
                    .mx(px(6.0))
                    .min_h(px(38.0))
                    .px(px(9.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .when(highlighted, |row| row.bg(colors.primary.alpha(0.08)))
                    .hover(move |row| row.bg(colors.primary.alpha(0.06)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.apply_recipe(&recipe);
                        cx.notify();
                    }))
                    .child(sf_symbol(
                        if issue.is_some() {
                            "exclamationmark.triangle"
                        } else {
                            "bookmark"
                        },
                        12.0,
                        if issue.is_some() {
                            colors.tertiary
                        } else {
                            colors.secondary
                        },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(1.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(if issue.is_some() {
                                        colors.secondary
                                    } else {
                                        colors.primary
                                    })
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(px(9.0))
                                    .text_color(colors.tertiary)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(subtitle),
                            ),
                    )
                    .when(selected, |row| {
                        row.child(sf_symbol_weighted(
                            "checkmark",
                            9.0,
                            SymbolWeight::Semibold,
                            colors.secondary,
                        ))
                    }),
            );
        }
        drop(store);
        let save_index = self.recipes().len();
        let highlighted = self.highlight == save_index;
        list = list.child(
            div()
                .id("launcher-recipe-save")
                .mx(px(6.0))
                .h(px(36.0))
                .px(px(9.0))
                .flex()
                .items_center()
                .gap(px(9.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .when(highlighted, |row| row.bg(colors.primary.alpha(0.08)))
                .hover(move |row| row.bg(colors.primary.alpha(0.06)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.begin_save(false);
                    cx.notify();
                }))
                .child(sf_symbol("plus", 12.0, colors.secondary))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(colors.primary)
                        .child("Save current as recipe…"),
                ),
        );
        FloatingSurface::new(colors, list).into_any_element()
    }

    fn render_panel(
        &self,
        colors: SemanticColors,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let can_submit = self.can_submit();
        let harness_open = self.picker == Some(Picker::Harness);
        let project_open = self.picker == Some(Picker::Project);
        let host_open = self.picker == Some(Picker::Host);
        let recipe_open = self.picker == Some(Picker::Recipe);
        let text_height = composer_text_height(self.prompt.line_count());
        let composer_height = text_height + COMPOSER_CONTROLS_HEIGHT;
        // The pickers hang off the bottom of the panel, which now moves with
        // the composer.
        let picker_top = TITLE_HEIGHT + TITLE_GAP + composer_height + SHELF_HEIGHT + 8.0;
        let issue = self.draft_issue();
        let blocker = self.blocker();
        let harness_label = self.selected_harness_label();
        let project_label = self.selected_project_label();
        let host_label = self.selected_host_label();
        let loaded_name = self.loaded_recipe().map(|recipe| recipe.name);
        let logo = ui_agent_kind(&self.selected_harness);
        let composer_fill = if colors.appearance == zeus_ui::Appearance::Dark {
            rgba(0x26282dff)
        } else {
            rgba(0xf2f1efff)
        };
        let shelf_fill = if colors.appearance == zeus_ui::Appearance::Dark {
            rgba(0x1d1f23ff)
        } else {
            rgba(0xe8e7e4ff)
        };

        // Wrapped lines are children of a scroll container so the composer's
        // handle can scroll BY LINE to keep the caret on screen — the whole
        // point of the rewrite. An empty prompt shows the placeholder in the
        // same row the caret is on, so the two do not fight over the baseline.
        let prompt = if self.prompt.is_empty() {
            div()
                .h(px(COMPOSER_LINE_HEIGHT))
                .flex()
                .items_center()
                .when(focused, |line| {
                    line.child(div().text_color(colors.primary.alpha(0.92)).child(CARET))
                })
                .child(
                    div()
                        .text_color(colors.tertiary)
                        .child("Describe the task…"),
                )
                .into_any_element()
        } else {
            div()
                .id("launcher-prompt-lines")
                .size_full()
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .track_scroll(self.prompt.scroll_handle())
                .children(self.prompt.render_lines(
                    px(COMPOSER_LINE_HEIGHT),
                    focused.then_some(CARET),
                    HighlightStyle {
                        background_color: Some(Palette::CLAY.alpha(0.35).into()),
                        ..HighlightStyle::default()
                    },
                ))
                .into_any_element()
        };

        let naming = matches!(
            self.mode,
            LauncherMode::SaveRecipe | LauncherMode::RenameRecipe
        );
        let heading = if naming {
            let (display, _) = self.recipe_name.display(if focused { CARET } else { "" });
            div()
                .id("launcher-recipe-name")
                .h(px(TITLE_HEIGHT))
                .px(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_text()
                .child(
                    div()
                        .text_size(px(18.0))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(colors.primary.alpha(0.94))
                        .child(if self.recipe_name.is_empty() && !focused {
                            "Name this recipe…".to_owned()
                        } else {
                            display
                        }),
                )
        } else {
            div()
                .id("launcher-heading")
                .h(px(TITLE_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(22.0))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(colors.primary.alpha(0.94))
                        .child(
                            loaded_name
                                .clone()
                                .unwrap_or_else(|| "What should we work on?".to_owned()),
                        ),
                )
        };

        let panel = div()
            .relative()
            .w(px(PANEL_WIDTH))
            .child(heading)
            .when_some(loaded_name.clone(), |panel, name| {
                panel.child(
                    div()
                        .mt(px(4.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .id("launcher-clear-recipe")
                                .h(px(22.0))
                                .px(px(8.0))
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .rounded(px(CONTROL_RADIUS))
                                .cursor_pointer()
                                .hover(move |chip| chip.bg(colors.primary.alpha(0.08)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.clear_loaded_recipe();
                                    cx.notify();
                                }))
                                .child(sf_symbol("bookmark.fill", 9.0, colors.secondary))
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(colors.secondary)
                                        .child(format!("Recipe · {name}")),
                                )
                                .child(sf_symbol("xmark", 8.0, colors.tertiary)),
                        ),
                )
            })
            .when_some(issue.clone(), |panel, issue| {
                let label = issue.repair_label();
                panel.child(
                    div()
                        .mt(px(8.0))
                        .mx(px(COMPOSER_INSET))
                        .px(px(10.0))
                        .py(px(8.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .rounded(px(CONTROL_RADIUS))
                        .bg(colors.primary.alpha(0.06))
                        .child(
                            div()
                                .min_w(px(0.0))
                                .flex_1()
                                .text_size(px(11.0))
                                .text_color(colors.secondary)
                                .child(issue.message()),
                        )
                        .child(
                            div()
                                .id("launcher-repair")
                                .h(px(22.0))
                                .px(px(8.0))
                                .flex()
                                .items_center()
                                .rounded(px(CONTROL_RADIUS - 1.0))
                                .cursor_pointer()
                                .text_size(px(10.0))
                                .text_color(colors.primary)
                                .bg(colors.primary.alpha(0.08))
                                .hover(move |button| button.bg(colors.primary.alpha(0.12)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.repair_issue(&issue);
                                    cx.notify();
                                }))
                                .child(label),
                        ),
                )
            })
            .child(
                div()
                    .relative()
                    .mt(px(TITLE_GAP))
                    .mx(px(COMPOSER_INSET))
                    .h(px(composer_height))
                    .rounded(px(Radius::PANEL))
                    .bg(composer_fill)
                    .border_1()
                    .border_color(if focused {
                        Palette::CLAY.alpha(0.42)
                    } else {
                        colors.primary.alpha(0.09)
                    })
                    .cursor_text()
                    .on_mouse_down(MouseButton::Left, {
                        let focus = self.focus.clone();
                        cx.listener(move |this, _, window, cx| {
                            this.field = LauncherField::Prompt;
                            window.focus(&focus, cx);
                            cx.notify();
                        })
                    })
                    .child(
                        div()
                            .h(px(text_height))
                            .px(px(COMPOSER_PADDING))
                            .pt(px(COMPOSER_PAD_TOP))
                            .pb(px(COMPOSER_PAD_BOTTOM))
                            .text_size(px(COMPOSER_FONT_SIZE))
                            .line_height(px(COMPOSER_LINE_HEIGHT))
                            .text_color(colors.primary)
                            .child(prompt),
                    )
                    .child(
                        div()
                            .h(px(COMPOSER_CONTROLS_HEIGHT))
                            .px(px(10.0))
                            .pb(px(8.0))
                            .flex()
                            .items_end()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .id("launcher-add-project")
                                            .h(px(CONTROL_SIZE))
                                            .px(px(9.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .gap(px(6.0))
                                            .rounded(px(CONTROL_RADIUS))
                                            .cursor_pointer()
                                            .hover(move |button| button.bg(Fill::subtle(colors)))
                                            .active(move |button| {
                                                button.bg(colors.primary.alpha(0.10))
                                            })
                                            .child(sf_symbol("plus", 11.0, colors.secondary))
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(colors.secondary)
                                                    .child("Choose folder"),
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.choose_folder(window, cx);
                                            })),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(colors.tertiary)
                                            .child(blocker.clone().unwrap_or_else(|| {
                                                "⇧↵  New line   ⇥  Agent   ⌘S  Save recipe"
                                                    .to_owned()
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(7.0))
                                    .child(
                                        div()
                                            .id("launcher-harness-button")
                                            .h(px(CONTROL_SIZE))
                                            .px(px(10.0))
                                            .flex()
                                            .items_center()
                                            .gap(px(7.0))
                                            .rounded(px(CONTROL_RADIUS))
                                            .cursor_pointer()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(colors.secondary)
                                            .bg(if harness_open {
                                                colors.primary.alpha(0.10)
                                            } else {
                                                Fill::subtle(colors)
                                            })
                                            .hover(move |button| {
                                                button.bg(colors.primary.alpha(0.09))
                                            })
                                            .active(move |button| {
                                                button.bg(colors.primary.alpha(0.12))
                                            })
                                            .child(AgentLogo::new(logo, 16.0, colors).badged(false))
                                            .child(harness_label)
                                            .child(sf_symbol("chevron.down", 7.5, colors.tertiary))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.toggle_picker(Picker::Harness);
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id("launcher-submit")
                                            .size(px(CONTROL_SIZE))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(CONTROL_RADIUS))
                                            .bg(if can_submit {
                                                colors.primary
                                            } else {
                                                Fill::subtle(colors)
                                            })
                                            .when(can_submit, |button| {
                                                button
                                                    .cursor_pointer()
                                                    .hover(move |button| button.opacity(0.86))
                                                    .active(move |button| button.opacity(0.72))
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.submit(cx);
                                                    }))
                                            })
                                            .child(sf_symbol_weighted(
                                                "chevron.up",
                                                10.0,
                                                SymbolWeight::Bold,
                                                if can_submit {
                                                    colors.background
                                                } else {
                                                    colors.tertiary
                                                },
                                            )),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .relative()
                    .mx(px(16.0))
                    .h(px(SHELF_HEIGHT))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded_bl(px(Radius::PANEL))
                    .rounded_br(px(Radius::PANEL))
                    .bg(shelf_fill)
                    .border_1()
                    .border_color(colors.primary.alpha(0.055))
                    .child(
                        div()
                            .id("launcher-project-button")
                            .h(px(CONTROL_SIZE - 2.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .gap(px(7.0))
                            .rounded(px(CONTROL_RADIUS - 1.0))
                            .cursor_pointer()
                            .bg(if project_open {
                                colors.primary.alpha(0.08)
                            } else {
                                colors.primary.alpha(0.0)
                            })
                            .hover(move |button| button.bg(colors.primary.alpha(0.08)))
                            .active(move |button| button.bg(colors.primary.alpha(0.11)))
                            .child(sf_symbol("folder", 11.0, colors.secondary))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(colors.primary.alpha(0.86))
                                    .child(project_label),
                            )
                            .child(sf_symbol("chevron.down", 8.0, colors.tertiary))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_picker(Picker::Project);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .id("launcher-host-button")
                                    .h(px(CONTROL_SIZE - 2.0))
                                    .px(px(8.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .rounded(px(CONTROL_RADIUS - 1.0))
                                    .cursor_pointer()
                                    .bg(if host_open {
                                        colors.primary.alpha(0.08)
                                    } else {
                                        colors.primary.alpha(0.0)
                                    })
                                    .hover(move |button| button.bg(colors.primary.alpha(0.08)))
                                    .child(sf_symbol(
                                        if self.selected_host.is_some() {
                                            "network"
                                        } else {
                                            "desktopcomputer"
                                        },
                                        11.0,
                                        colors.secondary,
                                    ))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(colors.primary.alpha(0.86))
                                            .child(host_label),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_picker(Picker::Host);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("launcher-worktree-button")
                                    .h(px(CONTROL_SIZE - 2.0))
                                    .px(px(8.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .rounded(px(CONTROL_RADIUS - 1.0))
                                    .when(self.selected_host.is_none(), |button| {
                                        button.cursor_pointer().hover(move |button| {
                                            button.bg(colors.primary.alpha(0.08))
                                        })
                                    })
                                    .bg(if self.worktree_create {
                                        colors.primary.alpha(0.08)
                                    } else {
                                        colors.primary.alpha(0.0)
                                    })
                                    .opacity(if self.selected_host.is_some() {
                                        0.45
                                    } else {
                                        1.0
                                    })
                                    .child(sf_symbol(
                                        "plus.square.on.square",
                                        11.0,
                                        colors.secondary,
                                    ))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(colors.primary.alpha(0.86))
                                            .child(
                                                if self.field == LauncherField::WorktreeBranch {
                                                    self.worktree_branch
                                                        .display(if focused { CARET } else { "" })
                                                        .0
                                                } else if !self.worktree_branch.is_empty() {
                                                    self.worktree_branch.text().to_owned()
                                                } else if self.worktree_create {
                                                    "Worktree".to_owned()
                                                } else {
                                                    "Checkout".to_owned()
                                                },
                                            ),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.selected_host.is_some() {
                                            return;
                                        }
                                        if this.worktree_create
                                            && this.field == LauncherField::WorktreeBranch
                                        {
                                            this.worktree_create = false;
                                            this.field = LauncherField::Prompt;
                                        } else {
                                            this.worktree_create = true;
                                            this.field = LauncherField::WorktreeBranch;
                                        }
                                        this.picker = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("launcher-title-button")
                                    .h(px(CONTROL_SIZE - 2.0))
                                    .px(px(8.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .rounded(px(CONTROL_RADIUS - 1.0))
                                    .cursor_pointer()
                                    .bg(if self.field == LauncherField::Title {
                                        colors.primary.alpha(0.08)
                                    } else {
                                        colors.primary.alpha(0.0)
                                    })
                                    .hover(move |button| button.bg(colors.primary.alpha(0.08)))
                                    .child(sf_symbol("textformat", 11.0, colors.secondary))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(colors.primary.alpha(0.86))
                                            .child(if self.field == LauncherField::Title {
                                                let (display, _) = self.title.display(if focused {
                                                    CARET
                                                } else {
                                                    ""
                                                });
                                                if display.is_empty() {
                                                    "Title".to_owned()
                                                } else {
                                                    display
                                                }
                                            } else if self.title.is_empty() {
                                                "Title".to_owned()
                                            } else {
                                                self.title.text().to_owned()
                                            }),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.field = LauncherField::Title;
                                        this.picker = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("launcher-recipe-button")
                                    .h(px(CONTROL_SIZE - 2.0))
                                    .px(px(8.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .rounded(px(CONTROL_RADIUS - 1.0))
                                    .cursor_pointer()
                                    .bg(if recipe_open {
                                        colors.primary.alpha(0.08)
                                    } else {
                                        colors.primary.alpha(0.0)
                                    })
                                    .hover(move |button| button.bg(colors.primary.alpha(0.08)))
                                    .child(sf_symbol("bookmark", 11.0, colors.secondary))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(colors.primary.alpha(0.86))
                                            .child("Recipes"),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_picker(Picker::Recipe);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("launcher-save-recipe")
                                    .h(px(CONTROL_SIZE - 2.0))
                                    .px(px(8.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .rounded(px(CONTROL_RADIUS - 1.0))
                                    .cursor_pointer()
                                    .hover(move |button| button.bg(colors.primary.alpha(0.08)))
                                    .child(sf_symbol(
                                        "square.and.arrow.down",
                                        11.0,
                                        colors.secondary,
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.begin_save(false);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .when(harness_open, |panel| {
                panel.child(
                    self.floating(picker_top, cx)
                        .right(px(COMPOSER_INSET))
                        .child(self.render_harness_picker(colors, cx)),
                )
            })
            .when(project_open, |panel| {
                panel.child(
                    self.floating(picker_top, cx)
                        .left(px(COMPOSER_INSET))
                        .child(self.render_project_picker(colors, cx)),
                )
            })
            .when(host_open, |panel| {
                panel.child(
                    self.floating(picker_top, cx)
                        .right(px(COMPOSER_INSET))
                        .child(self.render_host_picker(colors, cx)),
                )
            })
            .when(recipe_open, |panel| {
                panel.child(
                    self.floating(picker_top, cx)
                        .left(px(COMPOSER_INSET))
                        .child(self.render_recipe_picker(colors, cx)),
                )
            });

        panel.into_any_element()
    }

    /// Wrapper for a picker popover. It swallows its own mouse-down so the
    /// canvas behind it — which closes any open picker — does not tear the
    /// list away between press and release, which would eat the click.
    fn floating(&self, top: f32, cx: &mut Context<Self>) -> gpui::Div {
        div().absolute().top(px(top)).on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }),
        )
    }
}

impl Focusable for LauncherOverlay {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for LauncherOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = div()
            .id("new-session-launcher")
            .key_context("ZeusLauncher")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, window, cx| {
                this.handle_key_down(event, window, cx);
            }));
        if !self.open {
            return root.size(px(0.0));
        }

        // Soft-wrapping needs the text system, which only exists here. Doing
        // it before the panel is built is what lets the composer size itself
        // to the prompt and scroll the caret into view.
        self.prompt.layout(
            px(COMPOSER_TEXT_WIDTH),
            gpui::font(crate::fonts::ui_family()),
            px(COMPOSER_FONT_SIZE),
            window,
        );

        // The session workbench is intentionally always dark, independent of
        // macOS appearance. This is a destination in that workbench—not a
        // translucent window overlay—so paint the same fully opaque surface.
        let colors = SemanticColors::dark();
        let focused = self.focus.is_focused(window);
        root.size_full()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .bg(colors.background)
            // The entire empty workbench behaves like the editor's canvas: a
            // click anywhere returns to the prompt and dismisses whichever
            // picker was open, which previously stayed up until you found the
            // button again or pressed Escape.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.picker = None;
                    window.focus(&this.focus, cx);
                    cx.notify();
                }),
            )
            // Command-N is a high-frequency keyboard action; the destination
            // appears immediately rather than making the user wait on motion.
            .child(self.render_panel(colors, focused, cx))
    }
}

fn initial_target(services: &AppServices) -> (AgentKind, String) {
    let store = services
        .store
        .store
        .read()
        .expect("session store lock poisoned");
    let selected_root = store
        .selected_session()
        .and_then(|session| store.projects().get(&session.project_id))
        .map(|project| project.root.clone())
        .or_else(|| {
            store
                .projects()
                .values()
                .min_by(|left, right| left.name.cmp(&right.name))
                .map(|project| project.root.clone())
        })
        .unwrap_or_default();
    (store.preferences().default_agent.clone(), selected_root)
}

fn ui_agent_kind(kind: &AgentKind) -> UiAgentKind {
    UiAgentKind::from_id(kind.id())
}

fn project_commit(project_count: usize, highlight: usize) -> ProjectCommit {
    if highlight < project_count {
        ProjectCommit::Recent(highlight)
    } else {
        ProjectCommit::ChooseFolder
    }
}

fn close_picker_for_folder_choice(picker: &mut Option<Picker>) {
    *picker = None;
}

fn apply_folder_choice(selected_root: &mut String, chosen: Option<&Path>) -> bool {
    let Some(chosen) = chosen else {
        return false;
    };
    *selected_root = chosen.to_string_lossy().into_owned();
    true
}

fn recipe_subtitle(recipe: &LaunchRecipe, issue: Option<&RecipeIssue>) -> String {
    if let Some(issue) = issue {
        return issue.message();
    }
    let mut parts = Vec::new();
    parts.push(recipe.agent.id().to_owned());
    if let Some(host) = &recipe.host {
        parts.push(host.clone());
    }
    if recipe
        .worktree
        .as_ref()
        .is_some_and(|worktree| worktree.create)
    {
        parts.push("worktree".to_owned());
    }
    parts.join(" · ")
}

fn first_line_name(prompt: &str) -> String {
    let line = prompt
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .unwrap_or("Untitled recipe");
    let mut name = line.to_owned();
    if name.chars().count() > 48 {
        name = name.chars().take(48).collect();
        name.push('…');
    }
    name
}

fn edit_single_line(editor: &mut QueryEditor, event: &KeyDownEvent, cx: &mut gpui::App) -> bool {
    let Some(edit) = query_editor::edit_for(&event.keystroke) else {
        return false;
    };
    match edit {
        Edit::Local(local) => {
            editor.apply(local);
        }
        Edit::Clipboard(ClipboardEdit::Copy) => {
            query_editor::copy_selection(editor, cx);
        }
        Edit::Clipboard(ClipboardEdit::Cut) => {
            query_editor::cut_selection(editor, cx);
        }
        Edit::Clipboard(ClipboardEdit::Paste) => {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                editor.insert(&text);
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_ids_have_readable_fallback_labels() {
        assert_eq!(title_case_id("claude-code"), "Claude Code");
        assert_eq!(title_case_id("open_code"), "Open Code");
    }

    #[test]
    fn project_picker_always_ends_with_choose_folder() {
        assert_eq!(project_commit(0, 0), ProjectCommit::ChooseFolder);
        assert_eq!(project_commit(2, 0), ProjectCommit::Recent(0));
        assert_eq!(project_commit(2, 1), ProjectCommit::Recent(1));
        assert_eq!(project_commit(2, 2), ProjectCommit::ChooseFolder);
    }

    #[test]
    fn folder_chooser_closes_picker_and_preserves_draft_across_cancel_and_completion() {
        let mut prompt = PromptComposer::default();
        prompt.insert_multiline("keep this\nunfinished prompt");
        let mut picker = Some(Picker::Project);
        let mut selected = "/work/current".to_owned();

        close_picker_for_folder_choice(&mut picker);
        assert!(picker.is_none(), "native chooser must dismiss the popover");
        assert!(!apply_folder_choice(&mut selected, None));
        assert_eq!(selected, "/work/current");
        assert_eq!(prompt.text(), "keep this\nunfinished prompt");

        assert!(apply_folder_choice(
            &mut selected,
            Some(Path::new("/work/chosen"))
        ));
        assert_eq!(selected, "/work/chosen");
        assert_eq!(prompt.text(), "keep this\nunfinished prompt");
    }

    #[test]
    fn recipe_names_come_from_the_first_prompt_line() {
        assert_eq!(first_line_name(""), "Untitled recipe");
        assert_eq!(
            first_line_name("\n\nReview this PR\nmore"),
            "Review this PR"
        );
        assert!(first_line_name(&"a".repeat(80)).ends_with('…'));
    }

    #[test]
    fn recipe_subtitles_surface_the_exact_dependency_problem() {
        let recipe = LaunchRecipe {
            id: "r1".into(),
            name: "Review".into(),
            agent: AgentKind::CLAUDE_CODE,
            project: RecipeProject::Path {
                path: "/work/zeus".into(),
            },
            host: Some("forge".into()),
            worktree: None,
            initial_prompt: String::new(),
            title: None,
        };
        assert_eq!(recipe_subtitle(&recipe, None), "claude-code · forge");
        assert_eq!(
            recipe_subtitle(
                &recipe,
                Some(&RecipeIssue::MissingHost { id: "forge".into() })
            ),
            "Host forge is not in hosts.json"
        );
        assert_eq!(
            RecipeIssue::RemoteWorktree.repair_label(),
            "Turn off worktree"
        );
    }
}
