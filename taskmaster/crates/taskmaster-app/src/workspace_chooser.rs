//! Window-level workspace chooser shared by the tab strip and Command-O.

use std::sync::Arc;

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent, MouseButton,
    PathPromptOptions, Render, Role, ScrollHandle, SharedString, Task, Window, div, prelude::*, px,
};
use taskmaster_proto::ProjectId;
use taskmaster_ui::{Icon, IconName, IconSize};

use crate::sidebar::validate_workspace_root;
use crate::store::StoreRuntime;
use crate::workspace_colors::WorkspaceColors;

pub(crate) enum WorkspaceChooserEvent {
    Closed,
}

pub(crate) struct WorkspaceChooser {
    runtime: Arc<StoreRuntime>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    open: bool,
    preview: bool,
    highlight: usize,
    error: Option<String>,
    submitted_root: Option<String>,
    _store_changes: Task<()>,
}

impl EventEmitter<WorkspaceChooserEvent> for WorkspaceChooser {}

impl Focusable for WorkspaceChooser {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl WorkspaceChooser {
    pub(crate) fn new(runtime: Arc<StoreRuntime>, preview: bool, cx: &mut Context<Self>) -> Self {
        let mut changes = runtime.changes();
        let store_changes = cx.spawn(async move |this, cx| {
            loop {
                match changes.recv().await {
                    Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if this
                            .update(cx, |this, cx| {
                                this.reconcile_add(cx);
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
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            open: false,
            preview,
            highlight: 0,
            error: None,
            submitted_root: None,
            _store_changes: store_changes,
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.highlight = 0;
        self.error = None;
        self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(crate) fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.emit(WorkspaceChooserEvent::Closed);
        cx.notify();
    }

    fn activate(&mut self, id: ProjectId, cx: &mut Context<Self>) {
        if self.submitted_root.is_some() {
            return;
        }
        let opened = self
            .runtime
            .store
            .write()
            .expect("session store lock poisoned")
            .open_workspace_tab(id);
        if opened {
            self.close(cx);
        }
    }

    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.submitted_root.is_some() {
            return;
        }
        if self.preview {
            self.error = Some("Choose a sample workspace above in preview mode.".into());
            cx.notify();
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Workspace".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let selected = match paths.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                _ => None,
            };
            let Some(path) = selected else { return };
            let result = validate_workspace_root(&path.to_string_lossy());
            let _ = this.update_in(cx, |this, window, cx| {
                if !this.open {
                    return;
                }
                window.focus(&this.focus, cx);
                match result {
                    Ok(root) => {
                        let (requested, already_opened) = {
                            let mut store = this
                                .runtime
                                .store
                                .write()
                                .expect("session store lock poisoned");
                            let requested = store.add_project(root.clone());
                            let already_opened = store
                                .active_workspace()
                                .is_some_and(|project| project.root == root);
                            (requested, already_opened)
                        };
                        this.error = None;
                        if requested {
                            this.submitted_root = Some(root);
                        } else if already_opened {
                            this.close(cx);
                        } else {
                            this.error =
                                Some("Another workspace is opening. Try again shortly.".into());
                        }
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn reconcile_add(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.submitted_root.as_ref() else {
            return;
        };
        let (pending, error, added) = {
            let store = self
                .runtime
                .store
                .read()
                .expect("session store lock poisoned");
            (
                store.pending_project_root() == Some(root.as_str()),
                store.project_add_error().map(str::to_owned),
                store
                    .active_workspace()
                    .is_some_and(|project| &project.root == root),
            )
        };
        if pending {
            return;
        }
        self.submitted_root = None;
        self.error = error;
        if added && self.open {
            self.close(cx);
        }
    }

    pub(crate) fn key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let projects = self
            .runtime
            .store
            .write()
            .expect("session store lock poisoned")
            .sidebar_projection();
        let count = projects.projects.len();
        // Folder and dismiss controls follow the workspace rows.
        match event.keystroke.key.as_str() {
            "escape" => self.close(cx),
            "up" => self.highlight = self.highlight.saturating_sub(1),
            "down" => self.highlight = (self.highlight + 1).min(count + 1),
            "tab" => {
                self.highlight = if event.keystroke.modifiers.shift {
                    self.highlight.checked_sub(1).unwrap_or(count + 1)
                } else {
                    (self.highlight + 1) % (count + 2)
                };
            }
            "enter" | "space" => {
                if let Some(group) = projects.projects.get(self.highlight) {
                    self.activate(group.project.id.clone(), cx);
                } else if self.highlight == count {
                    self.choose_folder(window, cx);
                } else {
                    self.close(cx);
                }
            }
            _ => return,
        }
        if self.highlight < count {
            self.scroll.scroll_to_item(self.highlight);
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for WorkspaceChooser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let (projection, appearance, active) = {
            let mut store = self
                .runtime
                .store
                .write()
                .expect("session store lock poisoned");
            (
                store.sidebar_projection(),
                crate::app_theme::colors(&store.preferences().terminal_theme).appearance,
                store.active_workspace_id().cloned(),
            )
        };
        let colors = WorkspaceColors::new(appearance);
        let mut list = div()
            .id("workspace-chooser-list")
            .debug_selector(|| "WORKSPACE_CHOOSER_LIST".into())
            .track_scroll(&self.scroll)
            .max_h(px(280.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(4.0));
        self.highlight = self.highlight.min(projection.projects.len() + 1);
        for (index, group) in projection.projects.iter().enumerate() {
            let project = &group.project;
            let id = project.id.clone();
            let highlighted = self.highlight == index;
            let label = format!("Open {} — {}", project.name, project.root);
            list = list.child(
                div()
                    .id(SharedString::from(format!("workspace-choice-{}", id.0)))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("WORKSPACE_CHOICE_{}", id.0)
                    })
                    .role(Role::Button)
                    .aria_label(label)
                    .min_h(px(44.0))
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .border_2()
                    .border_color(if highlighted {
                        colors.focus
                    } else {
                        colors.focus.alpha(0.0)
                    })
                    .bg(if highlighted {
                        colors.hover
                    } else {
                        colors.surface
                    })
                    .hover(move |row| row.bg(colors.hover))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .on_click(cx.listener(move |this, _, _, cx| this.activate(id.clone(), cx)))
                    .child(Icon::new(
                        IconName::Folder,
                        IconSize::REGULAR,
                        colors.secondary,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_ellipsis()
                                    .child(project.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(colors.secondary)
                                    .text_ellipsis()
                                    .child(project.root.clone()),
                            ),
                    )
                    .when(active.as_ref() == Some(&project.id), |row| {
                        row.child(Icon::new(
                            IconName::Check,
                            IconSize::REGULAR,
                            colors.secondary,
                        ))
                    }),
            );
        }
        let pending = self.submitted_root.is_some();
        let open_folder_highlighted = self.highlight == projection.projects.len();
        let panel = div()
            .id("workspace-chooser")
            .debug_selector(|| "WORKSPACE_CHOOSER".into())
            .role(Role::Dialog)
            .aria_label("Open a workspace")
            .occlude()
            .track_focus(&self.focus)
            .w(px(480.0))
            .max_h(px(f32::from(window.viewport_size().height) - 64.0))
            .p(px(16.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(colors.outline)
            .bg(colors.surface)
            .text_color(colors.primary)
            .text_size(px(12.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Open a workspace"),
                    )
                    .child(
                        div()
                            .id("close-workspace-chooser")
                            .debug_selector(|| "WORKSPACE_CHOOSER_CLOSE".into())
                            .role(Role::Button)
                            .aria_label("Close workspace chooser")
                            .size(px(32.0))
                            .rounded(px(8.0))
                            .border_2()
                            .border_color(if self.highlight == projection.projects.len() + 1 {
                                colors.focus
                            } else {
                                colors.focus.alpha(0.0)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(move |button| button.bg(colors.hover))
                            .child(Icon::new(
                                IconName::Close,
                                IconSize::REGULAR,
                                colors.secondary,
                            ))
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(colors.secondary)
                    .child("Return to a workspace or choose a folder to open."),
            )
            .when(!projection.projects.is_empty(), |panel| panel.child(list))
            .child(
                div()
                    .id("workspace-open-folder")
                    .debug_selector(|| "WORKSPACE_OPEN_FOLDER".into())
                    .role(Role::Button)
                    .aria_label("Open folder")
                    .min_h(px(36.0))
                    .px(px(8.0))
                    .rounded(px(8.0))
                    .border_2()
                    .border_color(if open_folder_highlighted {
                        colors.focus
                    } else {
                        colors.outline
                    })
                    .bg(colors.raised)
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .when(!pending, |button| {
                        button
                            .cursor_pointer()
                            .hover(move |button| button.bg(colors.hover))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.choose_folder(window, cx)),
                            )
                    })
                    .child(Icon::new(
                        IconName::Plus,
                        IconSize::REGULAR,
                        colors.secondary,
                    ))
                    .child(if pending {
                        "Opening workspace…"
                    } else {
                        "Open folder…"
                    }),
            )
            .when_some(self.error.clone(), |panel, error| {
                panel.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(colors.secondary)
                        .child(error),
                )
            });
        div()
            .absolute()
            .inset_0()
            .size_full()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().alpha(0.25))
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close(cx);
                    cx.stop_propagation();
                }),
            )
            .child(panel)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use gpui::{Entity, Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, point};
    use taskmaster_proto::{Project, SessionListResult};

    use super::*;

    struct ChooserHarness {
        chooser: Entity<WorkspaceChooser>,
        background_events: Arc<AtomicUsize>,
    }

    impl Render for ChooserHarness {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let clicks = Arc::clone(&self.background_events);
            let scrolls = Arc::clone(&self.background_events);
            div()
                .size_full()
                .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if this.chooser.read(cx).is_open() {
                        this.chooser
                            .update(cx, |chooser, cx| chooser.key_down(event, window, cx));
                        cx.stop_propagation();
                    }
                }))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .on_mouse_down(MouseButton::Left, move |_, _, _| {
                            clicks.fetch_add(1, Ordering::Relaxed);
                        })
                        .on_scroll_wheel(move |_, _, _| {
                            scrolls.fetch_add(1, Ordering::Relaxed);
                        }),
                )
                .child(crate::root::cached_window_overlay(self.chooser.clone()))
        }
    }

    #[gpui::test]
    fn chooser_opens_existing_workspaces_and_occludes_background(cx: &mut TestAppContext) {
        let runtime = Arc::new(StoreRuntime::inert());
        {
            let mut store = runtime.store.write().unwrap();
            store.hydrate(SessionListResult {
                sessions: Vec::new(),
                projects: vec![Project {
                    id: ProjectId::new("known"),
                    name: "Known workspace".into(),
                    root: "/work/known".into(),
                    pinned_order: None,
                }],
            });
            store.close_active_workspace_tab();
        }
        let events = Arc::new(AtomicUsize::new(0));
        let view_runtime = Arc::clone(&runtime);
        let view_events = Arc::clone(&events);
        let (harness, cx) = cx.add_window_view(move |window, cx| {
            let chooser = cx.new(|cx| {
                let mut chooser = WorkspaceChooser::new(view_runtime, true, cx);
                chooser.open(window, cx);
                chooser
            });
            ChooserHarness {
                chooser,
                background_events: view_events,
            }
        });
        let chooser = harness.read_with(cx, |harness, _| harness.chooser.clone());
        let panel = cx.debug_bounds("WORKSPACE_CHOOSER").unwrap();
        cx.simulate_event(ScrollWheelEvent {
            position: panel.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-30.0))),
            modifiers: Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.simulate_click(point(px(10.0), px(10.0)), Modifiers::default());
        assert_eq!(events.load(Ordering::Relaxed), 0);
        assert!(!chooser.read_with(cx, |chooser, _| chooser.is_open()));
        assert!(runtime.store.read().unwrap().workspace_tabs().is_empty());

        cx.update(|window, cx| chooser.update(cx, |chooser, cx| chooser.open(window, cx)));
        let choice = cx.debug_bounds("WORKSPACE_CHOICE_known").unwrap();
        cx.simulate_click(choice.center(), Modifiers::default());
        assert_eq!(
            runtime.store.read().unwrap().active_workspace_id(),
            Some(&ProjectId::new("known"))
        );
        assert_eq!(runtime.store.read().unwrap().workspace_tabs().len(), 1);
        assert!(!chooser.read_with(cx, |chooser, _| chooser.is_open()));

        cx.update(|window, cx| chooser.update(cx, |chooser, cx| chooser.open(window, cx)));
        cx.simulate_keystrokes("down enter");
        assert!(chooser.read_with(cx, |chooser, _| chooser.error.is_some()));
        assert!(
            runtime
                .store
                .read()
                .unwrap()
                .pending_project_root()
                .is_none(),
            "preview folder action cannot contact the Engine"
        );
        cx.simulate_keystrokes("tab enter");
        assert!(!chooser.read_with(cx, |chooser, _| chooser.is_open()));
    }
}
