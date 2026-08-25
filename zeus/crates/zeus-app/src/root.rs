use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Context, CursorStyle, DragMoveEvent, Entity, FocusHandle, Focusable,
    FontWeight, KeyDownEvent, KeyUpEvent, Modifiers, ModifiersChangedEvent, MouseButton, Render,
    StyleRefinement, Subscription, Task, Window, actions, deferred, div, prelude::*, px, rgba,
};
use zeus_proto::{AgentKind, SessionId};
use zeus_ui::{FloatingSurface, Ink, Radius, SemanticColors, Typo};

use crate::AppServices;
use crate::inspector::{InspectorEvent, WorkbenchInspector};
use crate::macos::sf_symbols::{SymbolWeight, sf_symbol, sf_symbol_weighted};
use crate::navigation::{
    NavigationEvent, NavigationOverlay, ToggleCommandPalette, ToggleQuickOpen,
};
use crate::notifications::{InAppBanner, NotificationSound};
use crate::seam::{SeamSlide, toggle_has_settled};
use crate::session_surfaces::SessionSurfaces;
use crate::sidebar::{PreviewScenario, Sidebar, SidebarEvent};
use crate::sounds::{self, AfplayPlayer, SoundGate, StatusSound};
use crate::store::SpawnOptions;
use crate::surface_shell::UtilitySurfaces;
use crate::terminal_pane::{ShellChrome, TerminalPane, TerminalPaneEvent, TerminalViewport};
use crate::updates::UpdatePhase;
use crate::workbench::{
    HorizontalLayout, HorizontalLayoutInput, MAX_INSPECTOR_WIDTH, MIN_TERMINAL_WIDTH,
    WorkbenchLayout, solve_horizontal_layout,
};

const WINDOW_BOUNDS_SAVE_DELAY: Duration = Duration::from_millis(150);

pub(crate) fn cached_window_overlay<T: Render>(view: Entity<T>) -> impl IntoElement {
    view.cached(StyleRefinement::default().absolute().inset_0())
}

#[cfg(target_os = "macos")]
use crate::macos::{menu_bar::NativeMenuBar, notifier::NativeNotifier};

actions!(
    zeus,
    [CloseSession, ReopenSession, OpenWorkspace, OpenNewAgent]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NewSessionShortcut {
    Default,
    Shell,
    Codex,
}

/// The session-creation shortcut policy, kept separate from dispatch so it can
/// be regression-tested without constructing the full daemon-backed root view.
fn new_session_shortcut(key: &str, modifiers: Modifiers) -> Option<NewSessionShortcut> {
    if !modifiers.platform {
        return None;
    }
    match key {
        "t" if modifiers.alt => Some(NewSessionShortcut::Shell),
        "t" if !modifiers.shift => Some(NewSessionShortcut::Default),
        "n" if modifiers.shift => Some(NewSessionShortcut::Codex),
        _ => None,
    }
}

/// Session navigation owns only its explicit shortcut. Returning `None` leaves
/// arrow keys available to the focused text field or terminal.
fn session_navigation_delta(
    key: &str,
    modifiers: Modifiers,
    arrow_surface_visible: bool,
) -> Option<isize> {
    if !modifiers.platform || arrow_surface_visible {
        return None;
    }
    match key {
        "up" | "left" if modifiers.alt => Some(-1),
        "down" | "right" if modifiers.alt => Some(1),
        "[" | "{" => Some(-1),
        "]" | "}" => Some(1),
        _ => None,
    }
}

/// Drag payload for the sidebar resize seam. Renders nothing -- it exists so
/// GPUI keeps routing mouse moves to the root while the seam is being dragged.
#[derive(Clone, Copy)]
struct DraggedSidebarEdge;

impl Render for DraggedSidebarEdge {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// Drag payload for the horizontal workbench divider.
#[derive(Clone, Copy)]
struct DraggedTerminalEdge;

impl Render for DraggedTerminalEdge {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// Drag payload for the workbench/inspector seam.
#[derive(Clone, Copy)]
struct DraggedInspectorEdge;

impl Render for DraggedInspectorEdge {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// Advances one panel's seam by a frame and returns the width to paint,
/// clearing the slide once it lands. An unfinished slide asks for the next
/// frame itself: the seam is a plain animated width rather than a GPUI
/// animation element, so nothing else will tick the window.
///
/// Takes the slide by `&mut Option<_>` rather than hanging off `RootView` so
/// both seams can be advanced in one pass without borrowing all of `self`.
fn advance_seam(slide: &mut Option<SeamSlide>, settled: f32, now: Instant, window: &Window) -> f32 {
    match *slide {
        Some(active) if !active.is_done(now) => {
            window.request_animation_frame();
            active.seam_at(settled, now)
        }
        Some(_) => {
            *slide = None;
            settled
        }
        None => settled,
    }
}

pub struct RootView {
    sidebar: Entity<Sidebar>,
    terminal: Option<Entity<TerminalPane>>,
    navigation: Option<Entity<NavigationOverlay>>,
    session_surfaces: Option<Entity<SessionSurfaces>>,
    utility_surfaces: Option<Entity<UtilitySurfaces>>,
    inspector: Option<Entity<WorkbenchInspector>>,
    services: Arc<AppServices>,
    focus: FocusHandle,
    resize_origin: Option<(f32, f32)>,
    /// The sidebar open/close currently being painted, if any.
    sidebar_slide: Option<SeamSlide>,
    /// The sidebar seam width painted on the last frame. A new slide starts
    /// from this rather than from the settled width so it picks up wherever the
    /// previous frame left the panel.
    sidebar_seam: f32,
    auxiliary_terminal: Option<Entity<TerminalPane>>,
    auxiliary_id: Option<SessionId>,
    auxiliary_parent: Option<SessionId>,
    auxiliary_spawn_parent: Option<SessionId>,
    collapsed_auxiliary_parents: HashSet<SessionId>,
    workbench_layout: WorkbenchLayout,
    terminal_resize_origin: Option<(f32, f32)>,
    terminal_available_height: f32,
    inspector_open: bool,
    inspector_width: f32,
    inspector_resize_origin: Option<(f32, f32)>,
    /// The inspector's mirror of `sidebar_slide` / `sidebar_seam`.
    inspector_slide: Option<SeamSlide>,
    inspector_seam: f32,
    /// When the inspector last opened or closed, so a held ⌘⇧D cannot outrun
    /// its slide. The sidebar's equivalent lives on the sidebar itself, which
    /// owns its own visibility; the inspector's lives here because RootView is
    /// what owns that flag.
    inspector_toggled_at: Option<Instant>,
    /// Debounces move/resize persistence while retaining the newest placement
    /// in memory immediately (the quit hook flushes that value synchronously).
    window_bounds_save: Option<Task<()>>,
    status_banner: Option<InAppBanner>,
    status_banner_generation: u64,
    sound_gate: SoundGate,
    preview: bool,
    #[cfg(target_os = "macos")]
    menu_bar: Option<NativeMenuBar>,
    #[cfg(target_os = "macos")]
    notifier: NativeNotifier,
    _subscriptions: Vec<Subscription>,
    _service_events: Task<()>,
    _surface_sync: Option<Task<()>>,
    _workbench_sync: Task<()>,
}

impl Focusable for RootView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl RootView {
    pub(crate) fn new(
        services: Arc<AppServices>,
        preview: bool,
        preview_scenario: PreviewScenario,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let sidebar = cx.new(|cx| {
            Sidebar::new(
                Some(Arc::clone(&services.store)),
                preview,
                preview_scenario,
                cx,
            )
        });
        let terminal = {
            let runtime = Arc::clone(&services.store);
            let tokio = Arc::clone(&services.tokio);
            Some(cx.new(|cx| {
                if preview {
                    TerminalPane::new_preview(runtime, tokio, window, cx)
                } else {
                    let mut terminal = TerminalPane::new(runtime, tokio, window, cx);
                    terminal.show_startup_welcome();
                    terminal
                }
            }))
        };
        let navigation = (!preview).then(|| {
            let runtime = Arc::clone(&services.store);
            cx.new(|cx| NavigationOverlay::new(runtime, window, cx))
        });
        let session_surfaces = (!preview).then(|| {
            let runtime = Arc::clone(&services.store);
            cx.new(|cx| SessionSurfaces::new(runtime, cx))
        });
        let utility_surfaces = (!preview).then(|| {
            let runtime = Arc::clone(&services.store);
            let tokio = Arc::clone(&services.tokio);
            let updates = services.updates.clone();
            cx.new(|cx| UtilitySurfaces::new(runtime, tokio, updates, window, cx))
        });
        let inspector = {
            let runtime = Arc::clone(&services.store);
            let tokio = Arc::clone(&services.tokio);
            Some(cx.new(|cx| {
                let mut inspector = WorkbenchInspector::new(runtime, tokio, cx);
                inspector.set_preview_account(preview);
                inspector
            }))
        };
        if let (Some(terminal), Some(navigation), Some(utility_surfaces)) =
            (&terminal, &navigation, &utility_surfaces)
        {
            let navigation = navigation.clone();
            let utility_surfaces = utility_surfaces.clone();
            terminal.update(cx, |terminal, _| {
                terminal.set_shell_entities(navigation, utility_surfaces);
            });
        }
        if let Some(terminal) = &terminal {
            let terminal = terminal.clone();
            cx.defer_in(window, move |_, window, cx| {
                terminal.update(cx, |terminal, cx| terminal.focus(window, cx));
            });
        }
        if let Some(terminal) = &terminal {
            cx.subscribe(terminal, |this, _, event, cx| match event {
                TerminalPaneEvent::ToggleSidebar => {
                    this.sidebar.update(cx, |sidebar, cx| sidebar.toggle(cx));
                }
                TerminalPaneEvent::ToggleInspector => this.toggle_inspector(cx),
                TerminalPaneEvent::OpenWorkspace { root } => {
                    this.services
                        .store
                        .store
                        .write()
                        .expect("session store lock poisoned")
                        .add_project(root.clone());
                    this.sidebar.update(cx, |sidebar, cx| {
                        sidebar.show_new_agent_in_workspace(root.clone(), cx);
                    });
                }
                TerminalPaneEvent::OpenFileReference { reference, cwd, .. } => {
                    let inspector = this.inspector.clone();
                    this.reveal_inspector(cx);
                    if let Some(inspector) = inspector {
                        inspector.update(cx, |inspector, cx| {
                            inspector.open_file_reference(cwd.clone(), reference.clone(), cx);
                        });
                    }
                }
            })
            .detach();
        }
        cx.subscribe_in(&sidebar, window, |this, _, event, window, cx| {
            if matches!(event, SidebarEvent::SessionActivated)
                && let Some(terminal) = &this.terminal
            {
                terminal.update(cx, |terminal, cx| {
                    terminal.dismiss_startup_welcome(cx);
                    terminal.focus(window, cx);
                });
                this.sync_auxiliary_terminal(window, cx);
            }
            if matches!(event, SidebarEvent::AgentSpawnRequested)
                && let Some(terminal) = &this.terminal
            {
                terminal.update(cx, |terminal, cx| {
                    terminal.dismiss_startup_welcome(cx);
                });
            }
            if let SidebarEvent::Update(command) = event {
                this.services.updates.send(command.clone());
            }
            if matches!(event, SidebarEvent::AddRemoteHost)
                && let Some(surfaces) = &this.utility_surfaces
            {
                surfaces.update(cx, |surfaces, cx| {
                    surfaces.open_add_remote_host(window, cx);
                });
            }
            if matches!(event, SidebarEvent::VisibilityChanged) {
                this.begin_sidebar_slide(cx);
            }
            cx.notify();
        })
        .detach();
        if let Some(navigation) = &navigation {
            cx.subscribe(navigation, |this, _, event, cx| match event {
                NavigationEvent::ToggleSidebar => {
                    this.sidebar.update(cx, |sidebar, cx| sidebar.toggle(cx));
                }
                NavigationEvent::OpenOverview => {
                    if let Some(surfaces) = &this.session_surfaces {
                        surfaces.update(cx, |surfaces, cx| surfaces.open_overview(cx));
                    }
                }
                NavigationEvent::OpenWorktrees => {
                    if let Some(surfaces) = &this.utility_surfaces {
                        surfaces.update(cx, |surfaces, cx| surfaces.open_worktrees(cx));
                    }
                }
                NavigationEvent::OpenSettings => {
                    if let Some(surfaces) = &this.utility_surfaces {
                        surfaces.update(cx, |surfaces, cx| surfaces.open_settings(cx));
                    }
                }
                NavigationEvent::CheckForUpdates => {
                    this.services.updates.check(true);
                }
                NavigationEvent::OpenFile { cwd, reference } => {
                    let inspector = this.inspector.clone();
                    this.reveal_inspector(cx);
                    if let Some(inspector) = inspector {
                        inspector.update(cx, |inspector, cx| {
                            inspector.open_file_reference(cwd.clone(), reference.clone(), cx);
                        });
                    }
                }
            })
            .detach();
        }
        if let Some(inspector) = &inspector {
            cx.subscribe_in(
                inspector,
                window,
                |this, _, event, window, cx| match event {
                    InspectorEvent::Close => this.set_inspector_open(false, cx),
                    InspectorEvent::OpenSettings => {
                        if let Some(surfaces) = &this.utility_surfaces {
                            surfaces.update(cx, |surfaces, cx| surfaces.open_settings(cx));
                        }
                    }
                    InspectorEvent::AddRemoteHost => {
                        if let Some(surfaces) = &this.utility_surfaces {
                            surfaces.update(cx, |surfaces, cx| {
                                surfaces.open_add_remote_host(window, cx);
                            });
                        }
                    }
                    InspectorEvent::Update(command) => {
                        this.services.updates.send(command.clone());
                    }
                },
            )
            .detach();
        }

        let mut status_events = services.store.status_events();
        let mut snapshots = services.store.snapshots();
        let mut usage = services.usage_tx.subscribe();
        let mut updates = services.updates.subscribe();
        let initial_usage = *usage.borrow();
        if let Some(inspector) = &inspector {
            inspector.update(cx, |inspector, cx| inspector.set_usage(initial_usage, cx));
        }
        // Seed the current state: `watch` only wakes on changes, and an
        // unsupported build settles before this view exists.
        let initial_update = services.updates.state();
        sidebar.update(cx, |sidebar, cx| {
            sidebar.set_update(initial_update.clone(), cx)
        });
        if let Some(inspector) = &inspector {
            inspector.update(cx, |inspector, cx| inspector.set_update(initial_update, cx));
        }

        #[cfg(target_os = "macos")]
        let mut menu_bar = objc2_foundation::MainThreadMarker::new()
            .and_then(|mtm| NativeMenuBar::new(mtm, Arc::clone(&services.store.store)));
        #[cfg(target_os = "macos")]
        if let Some(menu_bar) = &mut menu_bar {
            menu_bar.update(&snapshots.borrow());
        }
        #[cfg(target_os = "macos")]
        let notifier = NativeNotifier::new(services.store.notification_action_sender());

        let activation_services = Arc::clone(&services);
        let activation = cx.observe_window_activation(window, move |_this, window, _cx| {
            activation_services
                .store
                .store
                .write()
                .expect("session store lock poisoned")
                .set_active(window.is_window_active());
        });
        let bounds_observer = (!preview).then(|| {
            cx.observe_window_bounds(window, |this, window, cx| {
                this.window_bounds_changed(window, cx);
            })
        });

        let service_sidebar = sidebar.clone();
        let service_inspector = inspector.clone();
        let service_events = cx.spawn(async move |this, cx| {
            loop {
                tokio::select! {
                    status = status_events.recv() => {
                        let Ok(status) = status else { break };
                        let _ = this.update(cx, |this, cx| {
                            #[cfg(target_os = "macos")]
                            let app_is_active = this
                                .services
                                .store
                                .store
                                .read()
                                .expect("session store lock poisoned")
                                .app_is_active();
                            if let Some(sound) = status.sound {
                                let sound = match sound {
                                    NotificationSound::NeedsInput => StatusSound::NeedsInput,
                                    NotificationSound::Done => StatusSound::Done,
                                    NotificationSound::Frozen => StatusSound::Frozen,
                                };
                                if this.sound_gate.should_play(sound, Instant::now()) {
                                    let _ = sounds::play(&AfplayPlayer, sound);
                                }
                            }
                            #[cfg(target_os = "macos")]
                            if let Some(notification) = &status.notification
                                && (!app_is_active || status.in_app_banner.is_none())
                            {
                                this.notifier.post(notification);
                            }
                            if let Some(banner) = status.in_app_banner {
                                this.status_banner_generation =
                                    this.status_banner_generation.wrapping_add(1);
                                let generation = this.status_banner_generation;
                                this.status_banner = Some(banner);
                                cx.notify();
                                cx.spawn(async move |this, cx| {
                                    cx.background_executor()
                                        .timer(Duration::from_secs(7))
                                        .await;
                                    let _ = this.update(cx, |this, cx| {
                                        if this.status_banner_generation == generation {
                                            this.status_banner = None;
                                            cx.notify();
                                        }
                                    });
                                })
                                .detach();
                            }
                        });
                    }
                    changed = snapshots.changed() => {
                        if changed.is_err() { break; }
                        let snapshot = snapshots.borrow_and_update().clone();
                        let _ = this.update(cx, |this, _cx| {
                            #[cfg(target_os = "macos")]
                            if let Some(menu_bar) = &mut this.menu_bar {
                                menu_bar.update(&snapshot);
                            }
                        });
                    }
                    changed = usage.changed() => {
                        if changed.is_err() { break; }
                        let snapshot = *usage.borrow_and_update();
                        if let Some(inspector) = &service_inspector {
                            inspector.update(cx, |inspector, cx| {
                                inspector.set_usage(snapshot, cx);
                            });
                        }
                    }
                    changed = updates.changed() => {
                        if changed.is_err() { break; }
                        let state = updates.borrow_and_update().clone();
                        let installing = state.phase == UpdatePhase::Installing;
                        service_sidebar.update(cx, |sidebar, cx| {
                            sidebar.set_update(state.clone(), cx);
                        });
                        if let Some(inspector) = &service_inspector {
                            inspector.update(cx, |inspector, cx| {
                                inspector.set_update(state, cx);
                            });
                        }
                        // The swap helper is already polling for this process
                        // to exit; quitting is what lets the install proceed.
                        if installing {
                            cx.update(|cx| cx.quit());
                        }
                    }
                }
            }
        });
        let surface_sync =
            terminal
                .as_ref()
                .zip(session_surfaces.as_ref())
                .map(|(terminal, surfaces)| {
                    let terminal = terminal.clone();
                    let surfaces = surfaces.clone();
                    let mut changes = services.store.changes();
                    cx.spawn(async move |_this, cx| {
                        loop {
                            match changes.recv().await {
                                Ok(())
                                | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                    let buffers = terminal
                                        .update(cx, |terminal, _| terminal.resident_buffers());
                                    surfaces.update(cx, |surfaces, _| {
                                        surfaces.sync_resident_buffers(buffers);
                                    });
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                            }
                        }
                    })
                });
        let mut workbench_changes = services.store.changes();
        let workbench_sync = cx.spawn_in(window, async move |this, cx| {
            loop {
                match workbench_changes.recv().await {
                    Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if this
                            .update_in(cx, |this, window, cx| {
                                this.sync_auxiliary_terminal(window, cx);
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
        let (workbench_layout, inspector_open, inspector_width) = {
            let store = services
                .store
                .store
                .read()
                .expect("session store lock poisoned");
            let prefs = store.preferences();
            (
                WorkbenchLayout::from_fraction(prefs.workbench_primary_fraction),
                prefs.inspector_open,
                prefs.inspector_width,
            )
        };
        if inspector_open && let Some(inspector) = &inspector {
            inspector.update(cx, |inspector, cx| inspector.set_visible(true, cx));
        }
        if preview && let Some(path) = std::env::var_os("ZEUS_PREVIEW_SCREENSHOT") {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(2500))
                    .await;
                let _ = this.update_in(cx, |_this, window, _cx| match window.render_to_image() {
                    Ok(image) => {
                        if let Err(error) = image.save(&path) {
                            eprintln!("zeus: preview screenshot failed: {error}");
                        } else {
                            eprintln!(
                                "zeus: preview screenshot saved ({}x{})",
                                image.width(),
                                image.height()
                            );
                        }
                    }
                    Err(error) => {
                        eprintln!("zeus: preview screenshot failed: {error}");
                    }
                });
            })
            .detach();
        }
        // Seed both seams from the restored layout so the first frame paints
        // the settled panels instead of sliding them open at launch.
        let sidebar_seam = if sidebar.read(cx).is_visible() {
            sidebar.read(cx).width()
        } else {
            0.0
        };
        let inspector_seam = if inspector_open { inspector_width } else { 0.0 };
        let mut root = Self {
            sidebar,
            terminal,
            navigation,
            session_surfaces,
            utility_surfaces,
            inspector,
            services,
            focus: cx.focus_handle(),
            resize_origin: None,
            sidebar_slide: None,
            sidebar_seam,
            auxiliary_terminal: None,
            auxiliary_id: None,
            auxiliary_parent: None,
            auxiliary_spawn_parent: None,
            collapsed_auxiliary_parents: HashSet::new(),
            workbench_layout,
            terminal_resize_origin: None,
            terminal_available_height: 0.0,
            inspector_open,
            inspector_width,
            inspector_slide: None,
            inspector_seam,
            inspector_toggled_at: None,
            inspector_resize_origin: None,
            window_bounds_save: None,
            status_banner: None,
            status_banner_generation: 0,
            sound_gate: SoundGate::default(),
            preview,
            #[cfg(target_os = "macos")]
            menu_bar,
            #[cfg(target_os = "macos")]
            notifier,
            _subscriptions: std::iter::once(activation).chain(bounds_observer).collect(),
            _service_events: service_events,
            _surface_sync: surface_sync,
            _workbench_sync: workbench_sync,
        };
        root.sync_auxiliary_terminal(window, cx);
        if !preview {
            // Do not rely on AppKit emitting a move/resize after the observer
            // is installed: even an untouched first launch should become the
            // placement restored by the next launch.
            root.window_bounds_changed(window, cx);
        }
        root
    }

    fn window_bounds_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let placement = crate::current_window_placement(window, cx);
        self.services
            .store
            .store
            .write()
            .expect("session store lock poisoned")
            .remember_window_placement(placement);

        if self.window_bounds_save.is_some() {
            return;
        }
        self.window_bounds_save = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(WINDOW_BOUNDS_SAVE_DELAY)
                .await;
            let _ = this.update_in(cx, |this, _window, _cx| {
                this.window_bounds_save.take();
                if let Err(error) = this
                    .services
                    .store
                    .store
                    .write()
                    .expect("session store lock poisoned")
                    .persist_preferences()
                {
                    eprintln!("zeus: could not remember window placement: {error}");
                }
            });
        }));
    }

    fn colors(&self) -> SemanticColors {
        let store = self
            .services
            .store
            .store
            .read()
            .expect("session store lock poisoned");
        crate::app_theme::colors(&store.preferences().terminal_theme)
    }

    /// Narrowest useful inspector in either orientation. The shared horizontal
    /// solver uses the conservative mirrored width for both arrangements so a
    /// flip changes placement, never the terminal allocation.
    fn inspector_min_width(&self) -> f32 {
        crate::inspector::min_width()
    }

    /// Mirrored workbench: sidebar trailing, inspector leading. Read from
    /// preferences the same way colors are, so a Settings toggle takes effect
    /// on the next paint without any extra plumbing.
    fn sidebar_on_right(&self) -> bool {
        self.services
            .store
            .store
            .read()
            .expect("session store lock poisoned")
            .preferences()
            .sidebar_on_right
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(surfaces) = &self.utility_surfaces
            && surfaces.read(cx).is_open()
        {
            let global_overlay_shortcut = event.keystroke.modifiers.platform
                && matches!(event.keystroke.key.as_str(), "h" | "," | "k" | "p");
            if !global_overlay_shortcut {
                surfaces.update(cx, |surfaces, cx| {
                    surfaces.key_down(event, _window, cx);
                });
                cx.stop_propagation();
                return;
            }
        }
        if let Some(surfaces) = &self.session_surfaces {
            surfaces.update(cx, |surfaces, cx| {
                surfaces.handle_key_down(event, _window, cx);
            });
        }
        if !event.keystroke.modifiers.platform {
            return;
        }
        match event.keystroke.key.as_str() {
            "k" => {
                if let Some(navigation) = &self.navigation {
                    navigation.update(cx, |navigation, cx| {
                        navigation.toggle_command_palette(&ToggleCommandPalette, _window, cx);
                    });
                }
            }
            "p" => {
                if let Some(navigation) = &self.navigation {
                    navigation.update(cx, |navigation, cx| {
                        navigation.toggle_quick_open(&ToggleQuickOpen, _window, cx);
                    });
                }
            }
            "h" if event.keystroke.modifiers.shift => {
                if let Some(surfaces) = &self.utility_surfaces {
                    surfaces.update(cx, |surfaces, cx| surfaces.toggle_history(cx));
                }
            }
            "," => {
                if let Some(surfaces) = &self.utility_surfaces {
                    surfaces.update(cx, |surfaces, cx| surfaces.open_settings(cx));
                }
            }
            "b" => self.sidebar.update(cx, |sidebar, cx| sidebar.toggle(cx)),
            "d" if event.keystroke.modifiers.shift => self.toggle_inspector(cx),
            "e" if event.keystroke.modifiers.shift => {
                let inspector = self.inspector.clone();
                self.reveal_inspector(cx);
                let Some(inspector) = inspector else {
                    return;
                };
                inspector.update(cx, |inspector, cx| {
                    inspector.focus_code_tree(_window, cx);
                });
            }
            key @ ("t" | "n") => match new_session_shortcut(key, event.keystroke.modifiers) {
                Some(NewSessionShortcut::Default) => {
                    if !self.spawn_default() {
                        return;
                    }
                }
                Some(NewSessionShortcut::Shell) => {
                    if !self.spawn(None) {
                        return;
                    }
                }
                Some(NewSessionShortcut::Codex) => {
                    if !self.spawn(Some(AgentKind::CODEX)) {
                        return;
                    }
                }
                None => return,
            },
            // ⌥⌘W: worktrees overview. ⌘⇧W archives the selected session;
            // plain ⌘W is bound globally to CloseSession.
            "w" if event.keystroke.modifiers.alt => {
                if let Some(surfaces) = &self.utility_surfaces {
                    surfaces.update(cx, |surfaces, cx| surfaces.open_worktrees(cx));
                } else {
                    return;
                }
            }
            "w" if event.keystroke.modifiers.shift => {
                let archived = self
                    .sidebar
                    .update(cx, |sidebar, cx| sidebar.archive_selected(cx));
                if !archived {
                    return;
                }
            }
            "r" if !event.keystroke.modifiers.shift => {
                let renaming = self
                    .sidebar
                    .update(cx, |sidebar, cx| sidebar.rename_selected(_window, cx));
                if !renaming {
                    return;
                }
            }
            // ⇧⌘J retains the attention-navigation command that previously
            // occupied plain ⌘J.
            "j" if event.keystroke.modifiers.shift => {
                let selected = self
                    .sidebar
                    .update(cx, |sidebar, cx| sidebar.select_next_needing_input(cx));
                if !selected {
                    return;
                }
            }
            // ⌘J opens (or focuses) a terminal owned by the selected agent's
            // workbench, below the primary pane.
            "j" => {
                if !self.open_auxiliary_terminal(_window, cx) {
                    return;
                }
            }
            digit @ ("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8") => {
                // ⌘1–⌘8 select the nth session, matching the sidebar's row
                // hints; selection also focuses the terminal via
                // SessionActivated.
                let index = (digit.as_bytes()[0] - b'1') as usize;
                let selected = self
                    .sidebar
                    .update(cx, |sidebar, cx| sidebar.select_shortcut(index, cx));
                if !selected {
                    return;
                }
            }
            // ⌘9 jumps to the last session, the browser convention.
            "9" => {
                let selected = self
                    .sidebar
                    .update(cx, |sidebar, cx| sidebar.select_last(cx));
                if !selected {
                    return;
                }
            }
            // ⌃⌘↑/⌃⌘↓ move the selected row within its project group.
            "up" | "down" if event.keystroke.modifiers.control && !self.arrow_surface_visible() => {
                let delta = if event.keystroke.key == "up" { -1 } else { 1 };
                let moved = self
                    .sidebar
                    .update(cx, |sidebar, cx| sidebar.reorder_selected(delta, cx));
                if !moved {
                    return;
                }
            }
            // The explicit session-navigation shortcut steps through sidebar
            // order, wrapping. The switcher and overview own arrows while open.
            key if session_navigation_delta(
                key,
                event.keystroke.modifiers,
                self.arrow_surface_visible(),
            )
            .is_some() =>
            {
                let delta = session_navigation_delta(
                    key,
                    event.keystroke.modifiers,
                    self.arrow_surface_visible(),
                )
                .expect("guard checked navigation shortcut");
                let selected = self
                    .sidebar
                    .update(cx, |sidebar, cx| sidebar.select_relative(delta, cx));
                if !selected {
                    return;
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Spawns a shell (`None`) or a specific agent straight from a shortcut,
    /// bypassing the sidebar's picker. No-ops in preview, which has no daemon
    /// to spawn into. Reports whether the spawn was dispatched.
    fn spawn(&self, agent: Option<AgentKind>) -> bool {
        if self.preview {
            return false;
        }
        let mut store = self
            .services
            .store
            .store
            .write()
            .expect("session store lock poisoned");
        match agent {
            Some(agent) => {
                let host = store.default_spawn_host();
                store.spawn_kind(
                    agent,
                    SpawnOptions {
                        host,
                        ..SpawnOptions::default()
                    },
                );
            }
            None => store.spawn_shell(SpawnOptions::default()),
        }
        true
    }

    fn spawn_default(&self) -> bool {
        if self.preview {
            return false;
        }
        self.services
            .store
            .store
            .write()
            .expect("session store lock poisoned")
            .spawn_default(SpawnOptions::default());
        true
    }

    fn open_auxiliary_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.preview {
            return false;
        }
        self.sync_auxiliary_terminal(window, cx);
        let selected = self
            .services
            .store
            .store
            .read()
            .expect("session store lock poisoned")
            .selected_session_id()
            .cloned();
        let Some(parent) = selected else {
            return false;
        };
        if self.auxiliary_parent.as_ref() == Some(&parent) && self.auxiliary_terminal.is_some() {
            self.collapsed_auxiliary_parents.insert(parent);
            self.auxiliary_terminal = None;
            self.auxiliary_id = None;
            self.auxiliary_parent = None;
            if let Some(primary) = &self.terminal {
                primary.update(cx, |terminal, cx| terminal.focus(window, cx));
            }
            cx.notify();
            return true;
        }
        if self.collapsed_auxiliary_parents.remove(&parent) {
            self.sync_auxiliary_terminal(window, cx);
            if let Some(terminal) = &self.auxiliary_terminal {
                terminal.update(cx, |terminal, cx| terminal.focus(window, cx));
                return true;
            }
        }
        if self.auxiliary_spawn_parent.as_ref() == Some(&parent) {
            return true;
        }
        let spawned = self
            .services
            .store
            .store
            .write()
            .expect("session store lock poisoned")
            .spawn_auxiliary_terminal(parent.clone());
        if spawned {
            self.auxiliary_spawn_parent = Some(parent);
            cx.notify();
        }
        spawned
    }

    /// Reconciles the UI-owned pane entity with the daemon-owned child shell.
    /// The relationship survives app restarts because it lives in the session
    /// record; the GPUI entity remains disposable rendering state.
    fn sync_auxiliary_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview {
            return;
        }
        let (selected, auxiliary, spawn_failed) = {
            let store = self
                .services
                .store
                .store
                .read()
                .expect("session store lock poisoned");
            let selected = store.selected_session_id().cloned();
            let auxiliary = selected
                .as_ref()
                .and_then(|parent| store.auxiliary_terminal_for(parent));
            (selected, auxiliary, store.last_action_error().is_some())
        };

        if selected
            .as_ref()
            .is_some_and(|parent| self.collapsed_auxiliary_parents.contains(parent))
        {
            // Collapsing a pane is UI-only: keep its daemon shell alive so
            // the next ⌘J restores the same scrollback and process state.
            self.auxiliary_terminal = None;
            self.auxiliary_id = None;
            self.auxiliary_parent = None;
            return;
        }

        if let Some(session) = auxiliary {
            let parent = session
                .parent
                .clone()
                .expect("auxiliary terminal has an owning session");
            if self.auxiliary_id.as_ref() == Some(&session.id)
                && self.auxiliary_parent.as_ref() == Some(&parent)
            {
                self.auxiliary_spawn_parent = None;
                return;
            }

            let runtime = Arc::clone(&self.services.store);
            let tokio = Arc::clone(&self.services.tokio);
            let id = session.id.clone();
            let terminal =
                cx.new(|cx| TerminalPane::new_fixed(runtime, tokio, id.clone(), window, cx));
            if let (Some(navigation), Some(utility_surfaces)) =
                (&self.navigation, &self.utility_surfaces)
            {
                terminal.update(cx, |terminal, _| {
                    terminal.set_shell_entities(navigation.clone(), utility_surfaces.clone());
                });
            }
            let should_focus = self.auxiliary_spawn_parent.as_ref() == Some(&parent);
            self.auxiliary_id = Some(session.id.clone());
            self.auxiliary_parent = Some(parent);
            self.auxiliary_terminal = Some(terminal.clone());
            self.auxiliary_spawn_parent = None;
            if should_focus {
                terminal.update(cx, |terminal, cx| terminal.focus(window, cx));
            }
            cx.notify();
            return;
        }

        let spawn_still_pending = selected
            .as_ref()
            .is_some_and(|selected| self.auxiliary_spawn_parent.as_ref() == Some(selected))
            && !spawn_failed;
        if spawn_still_pending {
            return;
        }
        let had_auxiliary_state = self.auxiliary_terminal.is_some()
            || self.auxiliary_id.is_some()
            || self.auxiliary_parent.is_some()
            || self.auxiliary_spawn_parent.is_some();
        self.auxiliary_terminal = None;
        self.auxiliary_id = None;
        self.auxiliary_parent = None;
        self.auxiliary_spawn_parent = None;
        if had_auxiliary_state {
            cx.notify();
        }
    }

    /// True while the ⌃Tab switcher or the overview is up: both drive their
    /// own arrow-key navigation, so ⌘↑/⌘↓ stays out of their way.
    fn arrow_surface_visible(&self) -> bool {
        let store = self
            .services
            .store
            .store
            .read()
            .expect("session store lock poisoned");
        store.switcher_state().is_visible() || store.overview_state().is_visible()
    }

    /// Cmd+W: close the selected session with the sidebar ✕ semantics.
    /// With no session selected the action propagates to the global
    /// handler in main.rs, which closes the window instead.
    fn close_selected_session(
        &mut self,
        _: &CloseSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .auxiliary_terminal
            .as_ref()
            .is_some_and(|terminal| terminal.read(cx).is_focused(window))
            && let Some(id) = self.auxiliary_id.clone()
        {
            self.services
                .store
                .store
                .write()
                .expect("session store lock poisoned")
                .remove_sessions(vec![id]);
            if let Some(terminal) = &self.terminal {
                terminal.update(cx, |terminal, cx| terminal.focus(window, cx));
            }
            return;
        }
        let closed = self
            .sidebar
            .update(cx, |sidebar, cx| sidebar.close_selected_now(cx));
        if !closed {
            cx.propagate();
        }
    }

    /// Cmd+Shift+T: reopen the most recently closed session (daemon-backed,
    /// survives restarts).
    fn reopen_last_session(
        &mut self,
        _: &ReopenSession,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.reopen_last(cx));
    }

    fn open_new_agent(&mut self, _: &OpenNewAgent, _window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.show_new_agent(cx));
    }

    fn open_workspace(&mut self, _: &OpenWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |terminal, cx| {
                terminal.choose_workspace_folder(window, cx);
            });
        }
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(surfaces) = &self.session_surfaces {
            surfaces.update(cx, |surfaces, cx| {
                surfaces.handle_key_up(event, window, cx);
            });
        }
    }

    fn on_modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(surfaces) = &self.session_surfaces {
            surfaces.update(cx, |surfaces, cx| {
                surfaces.handle_modifiers_changed(event, window, cx);
            });
        }
    }

    /// The settled seam width: what the sidebar wrapper is worth once nothing
    /// is animating. This -- not the painted seam -- is what the terminal is
    /// told about, so the PTY hears one resize per toggle rather than one per
    /// animation frame.
    fn settled_sidebar_seam(&self, cx: &App) -> f32 {
        let sidebar = self.sidebar.read(cx);
        if sidebar.is_visible() {
            sidebar.width()
        } else {
            0.0
        }
    }

    /// Starts sliding the seam toward the visibility the sidebar just adopted.
    /// Reduced-motion users get the settled width immediately.
    fn begin_sidebar_slide(&mut self, cx: &mut Context<Self>) {
        let to = self.settled_sidebar_seam(cx);
        self.sidebar_slide = (!cx.reduce_motion())
            .then(|| SeamSlide::begin(self.sidebar_seam, to))
            .flatten();
        if self.sidebar_slide.is_none() {
            self.sidebar_seam = to;
        }
    }

    /// The inspector's settled seam. Like the sidebar's, this is what the
    /// terminal is told about, so a slide costs no PTY resizes.
    fn settled_inspector_seam(&self) -> f32 {
        if self.inspector_open {
            self.inspector_width
                .clamp(self.inspector_min_width(), MAX_INSPECTOR_WIDTH)
        } else {
            0.0
        }
    }

    fn begin_inspector_slide(&mut self, cx: &mut Context<Self>) {
        let to = self.settled_inspector_seam();
        self.inspector_slide = (!cx.reduce_motion())
            .then(|| SeamSlide::begin(self.inspector_seam, to))
            .flatten();
        if self.inspector_slide.is_none() {
            self.inspector_seam = to;
        }
    }

    /// The grab strip that straddles the sidebar/terminal seam.
    ///
    /// Two things make this reliable, and both are easy to lose:
    ///  - `deferred` + `occlude` put the strip above the terminal card, which
    ///    is a later sibling and would otherwise win the hit test on the half
    ///    of the strip that overhangs it.
    ///  - the drag is tracked with `on_drag`/`on_drag_move` (see `RootView::
    ///    render`) rather than `on_mouse_move`, because plain move listeners
    ///    only fire while the hitbox is hovered -- so any pointer motion that
    ///    outran the 9px strip silently dropped the resize.
    fn resize_handle(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .relative()
            .flex_none()
            .w(px(0.0))
            .h_full()
            .child(deferred(
                div()
                    .id("sidebar-resize-handle")
                    .absolute()
                    .left(px(-4.5))
                    .top(px(0.0))
                    .w(px(9.0))
                    .h_full()
                    .cursor(CursorStyle::ResizeLeftRight)
                    .occlude()
                    .on_drag(DraggedSidebarEdge, |edge, _, _, cx| {
                        cx.stop_propagation();
                        cx.new(|_| *edge)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            let width = this.sidebar.read(cx).width();
                            this.resize_origin = Some((f32::from(event.position.x), width));
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
                            if event.click_count == 2 {
                                this.sidebar
                                    .update(cx, |sidebar, cx| sidebar.reset_width(cx));
                                cx.stop_propagation();
                            }
                            this.finish_resize(cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.finish_resize(cx)),
                    ),
            ))
            .into_any_element()
    }

    fn terminal_resize_handle(&self, cx: &mut Context<Self>) -> AnyElement {
        let line = rgba(0xffffff18);
        div()
            .relative()
            .flex_none()
            .h(px(1.0))
            .w_full()
            .bg(line)
            .child(deferred(
                div()
                    .id("terminal-resize-handle")
                    .absolute()
                    .top(px(-4.0))
                    .left(px(0.0))
                    .h(px(9.0))
                    .w_full()
                    .cursor(CursorStyle::ResizeUpDown)
                    .occlude()
                    .on_drag(DraggedTerminalEdge, |edge, _, _, cx| {
                        cx.stop_propagation();
                        cx.new(|_| *edge)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            let primary = this
                                .workbench_layout
                                .pane_heights(this.terminal_available_height)
                                .primary;
                            this.terminal_resize_origin =
                                Some((f32::from(event.position.y), primary));
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
                            if event.click_count == 2 {
                                this.workbench_layout.reset();
                            }
                            this.finish_terminal_resize(cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.finish_terminal_resize(cx)),
                    ),
            ))
            .into_any_element()
    }

    fn drag_resize(&mut self, pointer_x: f32, cx: &mut Context<Self>) {
        let Some((origin_x, base_width)) = self.resize_origin else {
            return;
        };
        let width = dragged_panel_width(base_width, pointer_x - origin_x, !self.sidebar_on_right());
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_width(width, cx));
    }

    fn drag_terminal_resize(&mut self, pointer_y: f32, cx: &mut Context<Self>) {
        let Some((origin_y, base_height)) = self.terminal_resize_origin else {
            return;
        };
        self.workbench_layout.resize_primary(
            base_height + pointer_y - origin_y,
            self.terminal_available_height,
        );
        cx.notify();
    }

    fn finish_terminal_resize(&mut self, cx: &mut Context<Self>) {
        if self.terminal_resize_origin.take().is_none() {
            return;
        }
        let fraction = self.workbench_layout.primary_fraction();
        if let Err(error) = self
            .services
            .store
            .store
            .write()
            .expect("session store lock poisoned")
            .update_preferences(|prefs| prefs.workbench_primary_fraction = fraction)
        {
            eprintln!("zeus: could not remember workbench split: {error}");
        }
        cx.notify();
    }

    /// End of a resize drag: the live width only lived in the sidebar's UI
    /// state, so write it through to preferences now.
    fn finish_resize(&mut self, cx: &mut Context<Self>) {
        if self.resize_origin.take().is_some() {
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.commit_width(cx));
        }
    }

    /// The single gate every inspector open and close passes through -- ⌘⇧D,
    /// the terminal chrome button, and the panel's own ✕ -- so the debounce
    /// only has to hold here.
    fn set_inspector_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.inspector_open == open {
            return;
        }
        let now = Instant::now();
        if !toggle_has_settled(self.inspector_toggled_at.map(|at| now.duration_since(at))) {
            return;
        }
        self.inspector_toggled_at = Some(now);
        self.inspector_open = open;
        if let Some(inspector) = &self.inspector {
            inspector.update(cx, |inspector, cx| inspector.set_visible(open, cx));
        }
        if let Err(error) = self
            .services
            .store
            .store
            .write()
            .expect("session store lock poisoned")
            .update_preferences(|prefs| prefs.inspector_open = open)
        {
            eprintln!("zeus: could not remember inspector visibility: {error}");
        }
        self.begin_inspector_slide(cx);
        cx.notify();
    }

    fn toggle_inspector(&mut self, cx: &mut Context<Self>) {
        self.set_inspector_open(!self.inspector_open, cx);
    }

    /// Source navigation is an explicit destination, so it must not be lost
    /// behind the short debounce that protects repeated panel toggles.
    fn reveal_inspector(&mut self, cx: &mut Context<Self>) {
        self.inspector_toggled_at = None;
        self.set_inspector_open(true, cx);
    }

    fn inspector_resize_handle(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .relative()
            .flex_none()
            .w(px(0.0))
            .h_full()
            .child(deferred(
                div()
                    .id("inspector-resize-handle")
                    .absolute()
                    .left(px(-4.5))
                    .top(px(0.0))
                    .w(px(9.0))
                    .h_full()
                    .cursor(CursorStyle::ResizeLeftRight)
                    .occlude()
                    .on_drag(DraggedInspectorEdge, |edge, _, _, cx| {
                        cx.stop_propagation();
                        cx.new(|_| *edge)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            this.inspector_resize_origin =
                                Some((f32::from(event.position.x), this.inspector_width));
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
                            if event.click_count == 2 {
                                this.inspector_width = 400.0_f32
                                    .max(this.inspector_min_width())
                                    .min(MAX_INSPECTOR_WIDTH);
                            }
                            this.finish_inspector_resize(cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.finish_inspector_resize(cx)),
                    ),
            ))
            .into_any_element()
    }

    fn drag_inspector_resize(&mut self, pointer_x: f32, cx: &mut Context<Self>) {
        let Some((origin_x, base_width)) = self.inspector_resize_origin else {
            return;
        };
        let width = dragged_panel_width(base_width, pointer_x - origin_x, self.sidebar_on_right());
        self.inspector_width = width.clamp(self.inspector_min_width(), MAX_INSPECTOR_WIDTH);
        if let Some(inspector) = &self.inspector {
            inspector.update(cx, |inspector, cx| {
                inspector.set_panel_width(self.inspector_width, cx)
            });
        }
        cx.notify();
    }

    fn finish_inspector_resize(&mut self, cx: &mut Context<Self>) {
        if self.inspector_resize_origin.take().is_none() {
            return;
        }
        let width = self.inspector_width;
        if let Some(inspector) = &self.inspector {
            inspector.update(cx, |inspector, cx| inspector.set_panel_width(width, cx));
        }
        if let Err(error) = self
            .services
            .store
            .store
            .write()
            .expect("session store lock poisoned")
            .update_preferences(|prefs| prefs.inspector_width = width)
        {
            eprintln!("zeus: could not remember inspector width: {error}");
        }
        cx.notify();
    }

    /// While a resize drag is active, keep pointer motion from reaching the
    /// terminal's selection layer. The drag payload still routes to RootView,
    /// while this transparent hitbox owns everything underneath it.
    fn resize_shield(&self, cx: &mut Context<Self>) -> AnyElement {
        let vertical = self.terminal_resize_origin.is_some();
        deferred(
            div()
                .id("active-resize-shield")
                .absolute()
                .inset_0()
                .cursor(if vertical {
                    CursorStyle::ResizeUpDown
                } else {
                    CursorStyle::ResizeLeftRight
                })
                .occlude()
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.finish_resize(cx);
                        this.finish_terminal_resize(cx);
                        this.finish_inspector_resize(cx);
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.finish_resize(cx);
                        this.finish_terminal_resize(cx);
                        this.finish_inspector_resize(cx);
                    }),
                ),
        )
        .into_any_element()
    }

    /// `layout` is the settled allocation and drives everything the terminal is
    /// *told* -- viewport geometry and resolved panel visibility. The two
    /// `*_seam` widths are what is being painted this frame and drive only the
    /// card's own top corners, so each radius appears the moment its panel
    /// finishes clearing rather than at the start of the slide. Keeping the two
    /// apart is what stops a 260ms slide from firing a PTY resize on every frame.
    fn terminal_card(
        &mut self,
        visible_sidebar: bool,
        layout: HorizontalLayout,
        seam: f32,
        inspector_seam: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let terminal = self.colors();
        let card_width = layout.terminal_width;
        let card_height = f32::from(window.viewport_size().height).max(0.0);
        let mirrored = self.sidebar_on_right();
        let card_x = layout.terminal_x;
        // Corner radii follow the painted seams of whatever panel is on that
        // side this frame, so mirroring only has to swap which seam is which.
        let (leading_seam, trailing_seam) = if mirrored {
            (inspector_seam, seam)
        } else {
            (seam, inspector_seam)
        };
        let chrome = ShellChrome {
            sidebar_visible: visible_sidebar,
            inspector_open: layout.inspector_chrome_open(),
            // Nothing to the card's left means the macOS window buttons land
            // on its own toolbar.
            traffic_light_lane: card_x <= 0.0,
            mirrored,
        };
        let selected = self
            .services
            .store
            .store
            .read()
            .expect("session store lock poisoned")
            .selected_session_id()
            .cloned();
        let split_open = self.auxiliary_terminal.is_some()
            || selected
                .as_ref()
                .is_some_and(|id| self.auxiliary_spawn_parent.as_ref() == Some(id));
        let mut card = div()
            .relative()
            .flex_1()
            .flex()
            .flex_col()
            .h_full()
            .min_w(px(0.0))
            .when(leading_seam <= 0.0, |card| {
                card.rounded_tl(px(Radius::CARD))
            })
            .when(trailing_seam <= 0.0, |card| {
                card.rounded_tr(px(Radius::CARD))
            })
            .when(mirrored, |card| card.rounded_br(px(Radius::CARD)))
            .when(!mirrored, |card| card.rounded_bl(px(Radius::CARD)))
            .bg(terminal.background)
            .overflow_hidden()
            .text_color(terminal.primary);

        // Paint the frame independently from layout. A normal border shrinks
        // the content box, putting this title bar one pixel below the
        // borderless sidebar title bar even though both are 42 points tall.
        let card_outline = div()
            .absolute()
            .inset_0()
            .when(leading_seam <= 0.0, |outline| {
                outline.rounded_tl(px(Radius::CARD))
            })
            .when(trailing_seam <= 0.0, |outline| {
                outline.rounded_tr(px(Radius::CARD))
            })
            .when(mirrored, |outline| outline.rounded_br(px(Radius::CARD)))
            .when(!mirrored, |outline| outline.rounded_bl(px(Radius::CARD)))
            .border_1()
            .border_color(terminal.primary.alpha(0.10));

        if split_open {
            let available_height = (card_height - 1.0).max(0.0);
            self.terminal_available_height = available_height;
            let heights = self.workbench_layout.pane_heights(available_height);
            if let Some(primary) = &self.terminal {
                primary.update(cx, |terminal, cx| {
                    terminal.set_shell_chrome(chrome, cx);
                    terminal.set_viewport(
                        TerminalViewport {
                            x: card_x,
                            y: 0.0,
                            width: card_width,
                            height: heights.primary,
                        },
                        cx,
                    );
                });
                card = card.child(
                    div()
                        .flex_none()
                        .w_full()
                        .h(px(heights.primary))
                        .min_h(px(0.0))
                        .overflow_hidden()
                        .child(primary.clone()),
                );
            }
            card = card.child(self.terminal_resize_handle(cx));

            let mut auxiliary = div()
                .relative()
                .flex_none()
                .w_full()
                .h(px(heights.auxiliary))
                .min_h(px(0.0))
                .overflow_hidden();
            if let Some(terminal) = &self.auxiliary_terminal {
                terminal.update(cx, |terminal, cx| {
                    terminal.set_viewport(
                        TerminalViewport {
                            x: card_x,
                            y: heights.primary + 1.0,
                            width: card_width,
                            height: heights.auxiliary,
                        },
                        cx,
                    );
                });
                auxiliary = auxiliary.child(terminal.clone());
            } else {
                auxiliary = auxiliary.child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(terminal.background)
                        .text_size(px(12.0))
                        .text_color(terminal.secondary)
                        .child("Opening terminal…"),
                );
            }
            if let Some(id) = self.auxiliary_id.clone() {
                let store = Arc::clone(&self.services.store);
                let primary = self.terminal.clone();
                auxiliary = auxiliary.child(
                    div()
                        .id("close-auxiliary-terminal")
                        .absolute()
                        .top(px(12.0))
                        .right(px(12.0))
                        .size(px(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(Radius::BADGE))
                        .cursor_pointer()
                        .text_color(terminal.secondary)
                        .hover(move |button| button.bg(terminal.primary.alpha(0.08)))
                        .child(sf_symbol("xmark", 10.5, terminal.secondary))
                        .on_click(move |_, window, cx| {
                            store
                                .store
                                .write()
                                .expect("session store lock poisoned")
                                .remove_sessions(vec![id.clone()]);
                            if let Some(primary) = &primary {
                                primary.update(cx, |terminal, cx| terminal.focus(window, cx));
                            }
                            cx.stop_propagation();
                        }),
                );
            }
            card = card.child(auxiliary);
        } else if let Some(primary) = &self.terminal {
            self.terminal_available_height = card_height;
            primary.update(cx, |terminal, cx| {
                terminal.set_shell_chrome(chrome, cx);
                terminal.set_viewport(
                    TerminalViewport {
                        x: card_x,
                        y: 0.0,
                        width: card_width,
                        height: card_height,
                    },
                    cx,
                );
            });
            card = card.child(primary.clone());
        }

        card.child(card_outline).into_any_element()
    }

    fn close_confirmation(
        &self,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (title, message) = self.sidebar.read(cx).pending_close_copy()?;
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000055))
                .on_mouse_down(MouseButton::Left, {
                    let sidebar = self.sidebar.clone();
                    move |_, _, cx| {
                        sidebar.update(cx, |sidebar, cx| sidebar.cancel_close(cx));
                        cx.stop_propagation();
                    }
                })
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .child(FloatingSurface::new(
                    colors,
                    div()
                        .w(px(320.0))
                        .p(px(18.0))
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .text_size(px(Typo::DISPLAY_TITLE.size))
                                .font_weight(Typo::DISPLAY_TITLE.weight)
                                .text_color(colors.primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(Typo::ROW.size))
                                .text_color(colors.secondary)
                                .child(message),
                        )
                        .child(
                            div()
                                .mt(px(6.0))
                                .flex()
                                .justify_end()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .id("cancel-close")
                                        .px(px(12.0))
                                        .h(px(30.0))
                                        .flex()
                                        .items_center()
                                        .rounded(px(Radius::ROW))
                                        .cursor_pointer()
                                        .text_size(px(Typo::ROW.size))
                                        .text_color(colors.secondary)
                                        .hover(move |button| button.bg(colors.primary.alpha(0.06)))
                                        .child("Cancel")
                                        .on_click({
                                            let sidebar = self.sidebar.clone();
                                            move |_, _, cx| {
                                                sidebar.update(cx, |sidebar, cx| {
                                                    sidebar.cancel_close(cx)
                                                });
                                            }
                                        }),
                                )
                                .child(
                                    div()
                                        .id("confirm-close")
                                        .px(px(12.0))
                                        .h(px(30.0))
                                        .flex()
                                        .items_center()
                                        .rounded(px(Radius::ROW))
                                        .cursor_pointer()
                                        .bg(zeus_ui::Ink::DANGER.alpha(0.16))
                                        .text_size(px(Typo::ROW.size))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(zeus_ui::Ink::DANGER)
                                        .child("Close")
                                        .on_click({
                                            let sidebar = self.sidebar.clone();
                                            move |_, _, cx| {
                                                sidebar.update(cx, |sidebar, cx| {
                                                    sidebar.confirm_close(cx)
                                                });
                                            }
                                        }),
                                ),
                        ),
                ))
                .into_any_element(),
        )
    }

    fn status_banner(&self, colors: SemanticColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let banner = self.status_banner.as_ref()?;
        Some(
            deferred(
                div()
                    .absolute()
                    .right(px(16.0))
                    .bottom(px(16.0))
                    .w(px(360.0))
                    .p(px(13.0))
                    .flex()
                    .items_start()
                    .gap(px(10.0))
                    .rounded(px(Radius::PANEL))
                    .bg(colors.background)
                    .border_1()
                    .border_color(colors.floating_stroke())
                    .shadow_lg()
                    .occlude()
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .text_size(px(Typo::ROW_EMPHASIZED.size))
                                    .font_weight(Typo::ROW_EMPHASIZED.weight)
                                    .text_color(colors.primary)
                                    .child(banner.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(Typo::META.size))
                                    .text_color(colors.secondary)
                                    .child(banner.body.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("dismiss-status-banner")
                            .size(px(22.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(Radius::CHIP))
                            .cursor_pointer()
                            .text_color(colors.tertiary)
                            .hover(move |button| button.bg(colors.primary.alpha(0.06)))
                            .child(sf_symbol_weighted(
                                "xmark",
                                8.5,
                                SymbolWeight::Bold,
                                colors.tertiary,
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.status_banner_generation =
                                    this.status_banner_generation.wrapping_add(1);
                                this.status_banner = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element(),
        )
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = self.colors();
        let sidebar_visible = self.sidebar.read(cx).is_visible();
        let sidebar_width = self.sidebar.read(cx).width();
        let startup_welcome_visible = self
            .terminal
            .as_ref()
            .is_some_and(|terminal| terminal.read(cx).is_startup_welcome_visible());
        let (has_selected_session, inspector_word_wrap) = {
            let store = self
                .services
                .store
                .store
                .read()
                .expect("session store lock poisoned");
            (
                store.selected_session_id().is_some(),
                store.preferences().inspector_word_wrap,
            )
        };
        if let Some(inspector) = &self.inspector
            && inspector.read(cx).word_wrap_enabled() != inspector_word_wrap
        {
            inspector.update(cx, |inspector, cx| {
                inspector.set_word_wrap(inspector_word_wrap, cx)
            });
        }
        let window_width = f32::from(window.viewport_size().width);
        let mirrored = self.sidebar_on_right();
        let inspector_has_standalone_destination = self
            .inspector
            .as_ref()
            .is_some_and(|inspector| inspector.read(cx).is_code_destination());
        let inspector_available = inspector_has_standalone_destination
            || (!startup_welcome_visible && has_selected_session);
        let requested_inspector_width = self
            .inspector_width
            .clamp(self.inspector_min_width(), MAX_INSPECTOR_WIDTH);
        let layout_input = HorizontalLayoutInput {
            window_width,
            sidebar_visible,
            sidebar_width,
            inspector_visible: self.inspector_open && inspector_available,
            requested_inspector_width,
            inspector_min_width: self.inspector_min_width(),
            terminal_min_width: MIN_TERMINAL_WIDTH,
            mirrored,
        };
        let layout = solve_horizontal_layout(layout_input);
        // Keep the panel's own wrapping/code viewport on the same resolved
        // width as its wrapper. When the user closes it, solve the still-open
        // panel as well so the fixed-width child can slide away without being
        // squeezed by the shrinking seam.
        let inspector_panel_width = if layout.inspector_width > 0.0 {
            layout.inspector_width
        } else if inspector_available {
            // Keep the child at a useful width while the seam clips it, so a
            // close or Narrow collapse slides the panel away instead of
            // squeezing its contents. Re-solving as open recovers Compact's
            // clamped width; Narrow has no visible width, so fall back to the
            // user's requested size.
            let open_width = solve_horizontal_layout(HorizontalLayoutInput {
                inspector_visible: true,
                ..layout_input
            })
            .inspector_width;
            if open_width > 0.0 {
                open_width
            } else {
                requested_inspector_width
            }
        } else {
            0.0
        };
        if inspector_panel_width > 0.0
            && let Some(inspector) = &self.inspector
        {
            inspector.update(cx, |inspector, cx| {
                inspector.set_panel_width(inspector_panel_width, cx)
            });
        }
        let occupied_sidebar_width = layout.sidebar_width;
        let inspector_width = layout.inspector_width;
        let now = Instant::now();
        self.sidebar_seam =
            advance_seam(&mut self.sidebar_slide, occupied_sidebar_width, now, window);
        self.inspector_seam = advance_seam(&mut self.inspector_slide, inspector_width, now, window);
        let seam = self.sidebar_seam;
        let inspector_seam = self.inspector_seam;
        // Each panel keeps its full width and is pinned to the window edge it
        // lives against -- so narrowing a wrapper slides its panel out under
        // the clip instead of squeezing every row's contents down with it.
        let sidebar_wrapper = div()
            .relative()
            .flex_none()
            .h_full()
            .overflow_hidden()
            .w(px(seam))
            .when(seam > 0.0, |wrapper| {
                wrapper.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .when(mirrored, |panel| panel.left(px(0.0)))
                        .when(!mirrored, |panel| panel.right(px(0.0)))
                        .h_full()
                        .w(px(sidebar_width))
                        // A reactive boundary: the sidebar re-renders on its
                        // own notifies, not on the terminal's 60fps repaints.
                        .child(
                            self.sidebar
                                .clone()
                                .cached(StyleRefinement::default().size_full()),
                        ),
                )
            });

        let mut root = div()
            .id("root")
            .size_full()
            // Real SF Pro (registered from SFNS.ttf at startup) for every UI
            // surface; the terminal grid sets its own mono font.
            .font_family(crate::fonts::ui_family())
            .flex()
            // Match the opaque platform window so content behind zeus never
            // participates in compositing. The sidebar keeps its own surface
            // treatment above this base.
            .bg(colors.background)
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::on_key_down))
            .capture_key_up(cx.listener(Self::on_key_up))
            .on_action(cx.listener(Self::close_selected_session))
            .on_action(cx.listener(Self::reopen_last_session))
            .on_action(cx.listener(Self::open_workspace))
            .on_action(cx.listener(Self::open_new_agent))
            .on_modifiers_changed(cx.listener(Self::on_modifiers_changed))
            // Fires for every move once the seam drag starts, wherever the
            // pointer wanders -- unlike hover-gated move listeners.
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<DraggedSidebarEdge>, _, cx| {
                    this.drag_resize(f32::from(event.event.position.x), cx);
                }),
            )
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<DraggedTerminalEdge>, _, cx| {
                    this.drag_terminal_resize(f32::from(event.event.position.y), cx);
                }),
            )
            .on_drag_move(cx.listener(
                |this, event: &DragMoveEvent<DraggedInspectorEdge>, _, cx| {
                    this.drag_inspector_resize(f32::from(event.event.position.x), cx);
                },
            ));
        let inspector_wrapper = (inspector_seam > 0.0)
            .then_some(self.inspector.as_ref())
            .flatten()
            .map(|inspector| {
                div()
                    .relative()
                    .flex_none()
                    .h_full()
                    .w(px(inspector_seam))
                    .overflow_hidden()
                    .when(mirrored, |wrapper| wrapper.border_r_1())
                    .when(!mirrored, |wrapper| wrapper.border_l_1())
                    .border_color(colors.primary.alpha(0.08))
                    .child(
                        div()
                            .absolute()
                            .top(px(0.0))
                            .when(mirrored, |panel| panel.right(px(0.0)))
                            .when(!mirrored, |panel| panel.left(px(0.0)))
                            .h_full()
                            .w(px(inspector_panel_width))
                            .child(
                                inspector
                                    .clone()
                                    .cached(StyleRefinement::default().size_full()),
                            ),
                    )
            });
        // Each seam handle is a zero-width strip centred on the boundary it
        // owns, so it works on either side of its panel; only the order of the
        // row changes when the workbench is mirrored.
        if mirrored {
            if let Some(wrapper) = inspector_wrapper {
                root = root.child(wrapper).child(self.inspector_resize_handle(cx));
            }
            root = root.child(self.terminal_card(
                sidebar_visible,
                layout,
                seam,
                inspector_seam,
                window,
                cx,
            ));
            if seam > 0.0 {
                root = root.child(self.resize_handle(cx));
            }
            root = root.child(sidebar_wrapper);
        } else {
            root = root.child(sidebar_wrapper);
            if seam > 0.0 {
                root = root.child(self.resize_handle(cx));
            }
            root = root.child(self.terminal_card(
                sidebar_visible,
                layout,
                seam,
                inspector_seam,
                window,
                cx,
            ));
            if let Some(wrapper) = inspector_wrapper {
                root = root.child(self.inspector_resize_handle(cx)).child(wrapper);
            }
        }
        if self.resize_origin.is_some()
            || self.terminal_resize_origin.is_some()
            || self.inspector_resize_origin.is_some()
        {
            root = root.child(self.resize_shield(cx));
        }
        if let Some(confirmation) = self.close_confirmation(colors, cx) {
            root = root.child(confirmation);
        }
        // Overlay views are cached reactive boundaries too: each subscribes to
        // store changes itself, so the only thing these wrappers must do is
        // stay out of the root flex row (absolute, zero-size at rest).
        if let Some(surfaces) = &self.session_surfaces {
            root = root.child(cached_window_overlay(surfaces.clone()));
        }
        if let Some(surfaces) = &self.utility_surfaces {
            root = root.child(cached_window_overlay(surfaces.clone()));
        }
        if let Some(navigation) = &self.navigation {
            root = root.child(cached_window_overlay(navigation.clone()));
        }
        if let Some(status) = self.status_banner(colors, cx) {
            root = root.child(status);
        }
        if !self.preview
            && let Some(build) = &self.services.dev_build
        {
            root = root.child(dev_build_marker(build.marker_label(), colors));
        }
        root
    }
}

/// Panel width part-way through a seam drag. A panel pinned to the window's
/// leading edge widens as the pointer travels right; one pinned to the trailing
/// edge widens as it travels left.
fn dragged_panel_width(base_width: f32, travel: f32, grows_rightward: bool) -> f32 {
    if grows_rightward {
        base_width + travel
    } else {
        base_width - travel
    }
}

fn dev_build_marker(label: &str, colors: SemanticColors) -> AnyElement {
    div()
        .absolute()
        .top(px(10.0))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .h(px(22.0))
                .px(px(7.0))
                .flex()
                .items_center()
                .gap(px(5.0))
                .rounded(px(Radius::CHIP))
                .border_1()
                .border_color(Ink::ATTENTION.alpha(0.22))
                .bg(colors.floating_surface())
                .text_size(px(Typo::META.size))
                .font_weight(Typo::META.weight)
                .text_color(colors.secondary)
                .child(
                    div()
                        .size(px(5.0))
                        .rounded_full()
                        .bg(Ink::ATTENTION.alpha(0.88)),
                )
                .child(div().text_color(Ink::ATTENTION.alpha(0.88)).child("DEV"))
                .child("·")
                .child(label.to_owned()),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_modifiers() -> Modifiers {
        Modifiers {
            platform: true,
            ..Modifiers::default()
        }
    }

    /// The mirrored workbench flips which way each seam widens its panel: the
    /// sidebar grows rightward only while it is on the leading edge, and the
    /// inspector only once it is.
    #[test]
    fn seam_drags_widen_panels_away_from_the_edge_they_sit_against() {
        let sidebar_leading = dragged_panel_width(232.0, 40.0, true);
        let sidebar_trailing = dragged_panel_width(232.0, 40.0, false);
        assert_eq!(sidebar_leading, 272.0);
        assert_eq!(sidebar_trailing, 192.0);
        assert_eq!(dragged_panel_width(400.0, -60.0, false), 460.0);
    }

    #[test]
    fn command_t_launches_the_configured_default_agent() {
        assert_eq!(
            new_session_shortcut("t", command_modifiers()),
            Some(NewSessionShortcut::Default)
        );
    }

    #[test]
    fn session_navigation_requires_command_option_arrows() {
        let command = command_modifiers();
        assert_eq!(session_navigation_delta("left", command, false), None);

        let command_option = Modifiers {
            alt: true,
            ..command
        };
        assert_eq!(
            session_navigation_delta("left", command_option, false),
            Some(-1)
        );
        assert_eq!(
            session_navigation_delta("right", command_option, false),
            Some(1)
        );
    }
}
