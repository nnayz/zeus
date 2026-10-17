//! Sidebar account control, usage, updates, and application destinations.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Anchor, AnyElement, Bounds, Context, EventEmitter, FocusHandle, FontWeight, KeyDownEvent,
    MouseButton, Pixels, Render, Role, Task, Window, anchored, canvas, deferred, div, point,
    prelude::*, px, rgb,
};
use taskmaster_ui::{Appearance, Metrics, Radius, SemanticColors, Typo};

use crate::macos::sf_symbols::{SymbolWeight, sf_symbol, sf_symbol_weighted};
use crate::store::StoreRuntime;
use crate::updates::{UpdateCommand, UpdatePhase, UpdateState};
use crate::usage::{UsageFormat, UsageSnapshot};

const ACCOUNT_MENU_WIDTH: f32 = 244.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AccountAction {
    Update,
    AddRemoteHost,
    Settings,
    Documentation,
    DeveloperGuide,
    Done,
}

#[derive(Clone, Debug)]
pub(crate) enum AccountMenuEvent {
    OpenSettings,
    AddRemoteHost,
    Update(UpdateCommand),
}

pub(crate) struct AccountMenu {
    runtime: Arc<StoreRuntime>,
    preview: bool,
    profile_name: String,
    open: bool,
    focus: FocusHandle,
    menu_focus: FocusHandle,
    focused_action: Option<AccountAction>,
    control_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    usage: Option<UsageSnapshot>,
    update: UpdateState,
    _store_changes: Task<()>,
}

impl EventEmitter<AccountMenuEvent> for AccountMenu {}

impl AccountMenu {
    pub(crate) fn new(runtime: Arc<StoreRuntime>, preview: bool, cx: &mut Context<Self>) -> Self {
        let mut changes = runtime.changes();
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
            runtime,
            preview,
            profile_name: crate::macos::local_profile_name(),
            open: false,
            focus: cx.focus_handle(),
            menu_focus: cx.focus_handle(),
            focused_action: None,
            control_bounds: Rc::new(Cell::new(None)),
            usage: None,
            update: UpdateState::default(),
            _store_changes: store_changes,
        }
    }

    pub(crate) fn set_usage(&mut self, snapshot: UsageSnapshot, cx: &mut Context<Self>) {
        self.usage = Some(snapshot);
        cx.notify();
    }

    pub(crate) fn set_update(&mut self, state: UpdateState, cx: &mut Context<Self>) {
        self.update = state;
        if self.open
            && self
                .focused_action
                .is_some_and(|action| !self.actions().contains(&action))
        {
            self.focused_action = self.actions().first().copied();
        }
        cx.notify();
    }

    pub(crate) fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            self.focused_action = None;
            cx.notify();
        }
    }

    pub(crate) fn is_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.focus.contains_focused(window, cx) || self.menu_focus.contains_focused(window, cx)
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    fn update_command(&self) -> Option<UpdateCommand> {
        match &self.update.phase {
            UpdatePhase::Available(_) => Some(UpdateCommand::Download),
            UpdatePhase::Ready(_) => Some(UpdateCommand::Install),
            UpdatePhase::Checking
            | UpdatePhase::Downloading { .. }
            | UpdatePhase::Installing
            | UpdatePhase::Unsupported(_) => None,
            _ => Some(UpdateCommand::Check {
                user_initiated: true,
            }),
        }
    }

    fn actions(&self) -> Vec<AccountAction> {
        self.update_command()
            .map(|_| AccountAction::Update)
            .into_iter()
            .chain([
                AccountAction::AddRemoteHost,
                AccountAction::Settings,
                AccountAction::Documentation,
                AccountAction::DeveloperGuide,
                AccountAction::Done,
            ])
            .collect()
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.dismiss(cx);
            window.focus(&self.focus, cx);
        } else {
            self.open = true;
            self.focused_action = self.actions().first().copied();
            window.focus(&self.menu_focus, cx);
            cx.notify();
        }
    }

    fn activate(&mut self, action: AccountAction, window: &mut Window, cx: &mut Context<Self>) {
        let update = self.update_command();
        self.dismiss(cx);
        window.focus(&self.focus, cx);
        match action {
            AccountAction::Update => {
                if let Some(command) = update {
                    cx.emit(AccountMenuEvent::Update(command));
                }
            }
            AccountAction::AddRemoteHost => cx.emit(AccountMenuEvent::AddRemoteHost),
            AccountAction::Settings => cx.emit(AccountMenuEvent::OpenSettings),
            AccountAction::Documentation => cx.open_url(crate::settings::DOCS_URL),
            AccountAction::DeveloperGuide => cx.open_url(crate::settings::DEVELOPER_DOCS_URL),
            AccountAction::Done => {}
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.alt {
            return;
        }
        let actions = self.actions();
        let key = event.keystroke.key.as_str();
        match key {
            "enter" | "space" if !self.open => self.toggle(window, cx),
            "enter" | "space" => {
                if let Some(action) = self.focused_action {
                    self.activate(action, window, cx);
                }
            }
            "escape" if self.open => {
                self.dismiss(cx);
                window.focus(&self.focus, cx);
            }
            "up" | "down" | "tab" if self.open => {
                let current = self
                    .focused_action
                    .and_then(|action| actions.iter().position(|candidate| *candidate == action))
                    .unwrap_or(0);
                let previous = key == "up" || key == "tab" && modifiers.shift;
                self.focused_action = Some(
                    actions[if previous {
                        (current + actions.len() - 1) % actions.len()
                    } else {
                        (current + 1) % actions.len()
                    }],
                );
                cx.notify();
            }
            "home" if self.open => {
                self.focused_action = actions.first().copied();
                cx.notify();
            }
            "end" if self.open => {
                self.focused_action = actions.last().copied();
                cx.notify();
            }
            _ if self.open => {}
            _ => return,
        }
        // Capture before GPUI synthesizes an Enter/Space click on key-up.
        // Menu navigation and printable keys must not reach terminal input.
        window.prevent_default();
        cx.stop_propagation();
    }

    fn action_focused(&self, action: AccountAction) -> bool {
        self.open && self.focused_action == Some(action)
    }

    fn action_row(
        &self,
        id: &'static str,
        action: AccountAction,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let focused = self.action_focused(action);
        div()
            .id(id)
            .role(Role::MenuItem)
            .aria_selected(focused)
            .when(focused, |row| row.aria_active_descendant())
            .mx(px(6.0))
            .px(px(8.0))
            .h(px(30.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(px(Radius::ROW))
            .border_2()
            .border_color(colors.primary.alpha(if focused { 0.28 } else { 0.0 }))
            .bg(colors.primary.alpha(if focused { 0.08 } else { 0.0 }))
            .cursor_pointer()
            .hover(move |element| element.bg(colors.primary.alpha(0.06)))
            .text_size(px(Typo::ROW.size))
            .text_color(colors.primary)
            .on_click(cx.listener(move |this, _, window, cx| this.activate(action, window, cx)))
    }

    fn account_update_row(&self, colors: SemanticColors, cx: &mut Context<Self>) -> AnyElement {
        let unsupported = matches!(self.update.phase, UpdatePhase::Unsupported(_));
        let command = self.update_command();
        let label = if self.preview {
            format!("taskmaster {}", crate::updates::CURRENT_VERSION)
        } else {
            self.update.summary()
        };
        let mut row = div()
            .id("account-version")
            .debug_selector(|| "ACCOUNT_UPDATE".to_owned())
            .role(Role::MenuItem)
            .aria_selected(self.action_focused(AccountAction::Update))
            .when(self.action_focused(AccountAction::Update), |row| {
                row.aria_active_descendant()
            })
            .bg(colors
                .primary
                .alpha(if self.action_focused(AccountAction::Update) {
                    0.08
                } else {
                    0.0
                }))
            .border_2()
            .border_color(
                colors
                    .primary
                    .alpha(if self.action_focused(AccountAction::Update) {
                        0.28
                    } else {
                        0.0
                    }),
            )
            .mx(px(6.0))
            .px(px(8.0))
            .h(px(28.0))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .rounded(px(Radius::ROW))
            .text_size(px(Typo::ROW.size))
            .text_color(if unsupported {
                colors.tertiary
            } else {
                colors.primary
            })
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(label),
            );
        if let Some(command) = command {
            let action = match command {
                UpdateCommand::Download => "Download",
                UpdateCommand::Install => "Restart",
                _ => "Check",
            };
            row = row
                .cursor_pointer()
                .hover(move |element| element.bg(colors.primary.alpha(0.06)))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(Typo::META.size))
                        .text_color(colors.secondary)
                        .child(action),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.activate(AccountAction::Update, window, cx);
                }));
        }
        row.into_any_element()
    }

    fn account_popover(
        &self,
        colors: SemanticColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut usage = div()
            .flex()
            .flex_col()
            .child(account_section_label("Usage", colors));
        if self.preview {
            usage = usage
                .child(account_usage_row(
                    "Session",
                    "resets in 2h 14m",
                    "$2.31",
                    colors,
                ))
                .child(account_usage_row("Today", "1.8M tokens", "$4.82", colors))
                .child(account_usage_row("This month", "", "$86.40", colors));
        } else if let Some(snapshot) = self.usage {
            usage = usage
                .child(account_usage_row(
                    "Session",
                    snapshot
                        .session_remaining_seconds
                        .map(|seconds| format!("resets in {}", compact_duration(seconds)))
                        .as_deref()
                        .unwrap_or("idle"),
                    &snapshot
                        .session_cost
                        .map(UsageFormat::money)
                        .unwrap_or_else(|| "—".into()),
                    colors,
                ))
                .child(account_usage_row(
                    "Today",
                    &format!(
                        "{} tokens",
                        UsageFormat::tokens(snapshot.today().total_tokens())
                    ),
                    &UsageFormat::money(snapshot.today().cost),
                    colors,
                ))
                .child(account_usage_row(
                    "This month",
                    "",
                    &UsageFormat::money(snapshot.month().cost),
                    colors,
                ));
        } else {
            usage = usage.child(
                div()
                    .px(px(14.0))
                    .py(px(6.0))
                    .text_size(px(Typo::ROW.size))
                    .text_color(colors.tertiary)
                    .child("Measuring…"),
            );
        }

        let content = div()
            .flex()
            .flex_col()
            .child(usage)
            .child(div().mt(px(8.0)).h(px(1.0)).bg(colors.primary.alpha(0.06)))
            .child(account_section_label("Version", colors))
            .child(self.account_update_row(colors, cx))
            .child(div().mt(px(8.0)).h(px(1.0)).bg(colors.primary.alpha(0.06)))
            .child(account_section_label("Remote", colors))
            .child(
                self.action_row(
                    "account-add-remote-host",
                    AccountAction::AddRemoteHost,
                    colors,
                    cx,
                )
                .debug_selector(|| "ACCOUNT_ADD_REMOTE_HOST".to_owned())
                .child(sf_symbol("plus", 11.0, colors.secondary))
                .child(div().flex_1().child("Add remote host"))
                .child(
                    div()
                        .text_size(px(Typo::META.size))
                        .text_color(colors.tertiary)
                        .child("SSH"),
                ),
            )
            .child(div().mt(px(8.0)).h(px(1.0)).bg(colors.primary.alpha(0.06)))
            .child(account_section_label("Account", colors))
            .child(
                self.action_row("account-open-settings", AccountAction::Settings, colors, cx)
                    .debug_selector(|| "ACCOUNT_OPEN_SETTINGS".to_owned())
                    .child(sf_symbol("gearshape", 11.0, colors.secondary))
                    .child(div().flex_1().child("Settings"))
                    .child(
                        div()
                            .text_size(px(Typo::META.size))
                            .text_color(colors.tertiary)
                            .child("⌘,"),
                    ),
            )
            .child(
                div()
                    .id("account-active")
                    .mx(px(6.0))
                    .px(px(8.0))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(px(Radius::ROW))
                    .text_size(px(Typo::ROW.size))
                    .text_color(colors.primary)
                    .child(sf_symbol_weighted(
                        "checkmark",
                        10.0,
                        SymbolWeight::Semibold,
                        colors.secondary,
                    ))
                    .child(if self.preview {
                        "preview@taskmaster.local"
                    } else {
                        "Local agents"
                    }),
            )
            .child(div().mt(px(8.0)).h(px(1.0)).bg(colors.primary.alpha(0.06)))
            .child(account_section_label("Help", colors))
            .child(
                self.action_row(
                    "account-open-documentation",
                    AccountAction::Documentation,
                    colors,
                    cx,
                )
                .debug_selector(|| "ACCOUNT_DOCUMENTATION".to_owned())
                .child(sf_symbol("link", 11.0, colors.secondary))
                .child(div().flex_1().child("User Guide")),
            )
            .child(
                self.action_row(
                    "account-open-developer-guide",
                    AccountAction::DeveloperGuide,
                    colors,
                    cx,
                )
                .debug_selector(|| "ACCOUNT_DEVELOPER_GUIDE".to_owned())
                .child(sf_symbol("link", 11.0, colors.secondary))
                .child(div().flex_1().child("Developer Guide")),
            )
            .child(
                self.action_row("account-dismiss", AccountAction::Done, colors, cx)
                    .debug_selector(|| "ACCOUNT_DONE".to_owned())
                    .my(px(6.0))
                    .h(px(28.0))
                    .text_color(colors.secondary)
                    .child("Done"),
            );

        let mirrored = self
            .runtime
            .store
            .read()
            .expect("session store lock poisoned")
            .preferences()
            .sidebar_on_right;
        let bounds = self.control_bounds.get();
        let x = bounds.map_or_else(
            || {
                if mirrored {
                    f32::from(window.viewport_size().width) - 8.0
                } else {
                    8.0
                }
            },
            |bounds| {
                f32::from(if mirrored {
                    bounds.right()
                } else {
                    bounds.left()
                })
            },
        );
        let y = bounds.map_or(f32::from(window.viewport_size().height) - 52.0, |bounds| {
            f32::from(bounds.top()) - 4.0
        });
        let position = point(px(x), px(y));
        let anchor = if mirrored {
            Anchor::BottomRight
        } else {
            Anchor::BottomLeft
        };
        deferred(
            anchored()
                .position(position)
                .anchor(anchor)
                .snap_to_window_with_margin(px(8.0))
                .child(
                    div()
                        .id("account-menu")
                        .debug_selector(|| "ACCOUNT_MENU".to_owned())
                        .role(Role::Menu)
                        .aria_label("Account")
                        .track_focus(&self.menu_focus)
                        .capture_key_down(cx.listener(Self::key_down))
                        .w(px(ACCOUNT_MENU_WIDTH))
                        .max_h(px(
                            (f32::from(window.viewport_size().height) - 64.0).max(1.0)
                        ))
                        .overflow_y_scroll()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(colors.primary.alpha(0.10))
                        .bg(colors.floating_surface())
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.dismiss(cx)))
                        .child(div().py(px(4.0)).child(content)),
                ),
        )
        .with_priority(2)
        .into_any_element()
    }
}

impl Render for AccountMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let appearance = {
            let store = self
                .runtime
                .store
                .read()
                .expect("session store lock poisoned");
            crate::app_theme::colors(&store.preferences().terminal_theme).appearance
        };
        let colors = menu_colors(appearance);
        let open = self.open;
        let control_bounds = Rc::clone(&self.control_bounds);
        div()
            .id("sidebar-account")
            .relative()
            .w_full()
            .h(px(32.0))
            .flex_none()
            .capture_key_down(cx.listener(Self::key_down))
            .child(
                canvas(
                    move |bounds, _, _| control_bounds.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .id("sidebar-account-control")
                    .debug_selector(|| "SIDEBAR_ACCOUNT_CONTROL".to_owned())
                    .track_focus(&self.focus)
                    .tab_stop(true)
                    .role(Role::Button)
                    .aria_label(format!("{} account", self.profile_name))
                    .aria_expanded(self.open)
                    .w_full()
                    .h(px(32.0))
                    .px(px(6.0))
                    .rounded(px(8.0))
                    .border_2()
                    .border_color(gpui::transparent_black())
                    .focus_visible(move |button| {
                        button.border_color(match appearance {
                            Appearance::Light => rgb(0x006bd6),
                            Appearance::Dark => rgb(0x2997ff),
                        })
                    })
                    .bg(if open {
                        colors.primary.alpha(0.06)
                    } else {
                        colors.primary.alpha(0.0)
                    })
                    .hover(move |button| button.bg(colors.primary.alpha(0.06)))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle(window, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        div()
                            .relative()
                            .size(px(24.0))
                            .flex_none()
                            .rounded_full()
                            .bg(colors.primary.alpha(0.08))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(Typo::META.size))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.primary)
                            .child(
                                self.profile_name
                                    .chars()
                                    .next()
                                    .map(|character| character.to_uppercase().collect::<String>())
                                    .unwrap_or_else(|| "U".to_owned()),
                            )
                            .when(self.update.is_noteworthy(), |avatar| {
                                avatar.child(
                                    div()
                                        .absolute()
                                        .top(px(1.0))
                                        .right(px(1.0))
                                        .size(px(6.0))
                                        .rounded_full()
                                        .bg(taskmaster_ui::Ink::FRESH),
                                )
                            }),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(px(Typo::ROW.size))
                            .text_color(colors.primary)
                            .child(self.profile_name.clone()),
                    )
                    .child(sf_symbol("chevron.up.chevron.down", 10.0, colors.secondary)),
            )
            .when(self.open, |footer| {
                footer.child(self.account_popover(colors, window, cx))
            })
    }
}

fn menu_colors(appearance: Appearance) -> SemanticColors {
    let (surface, raised, primary, secondary, muted) = match appearance {
        Appearance::Light => (0xfcfcfc, 0xffffff, 0x191919, 0x6e6e6e, 0x737373),
        Appearance::Dark => (0x0d0d0d, 0x161616, 0xe6e6e6, 0xa0a0a0, 0x8c8c8c),
    };
    let mut colors = SemanticColors::themed(
        appearance,
        rgb(surface),
        rgb(primary),
        rgb(surface),
        rgb(raised),
        true,
    );
    colors.secondary = rgb(secondary);
    colors.tertiary = rgb(muted);
    colors
}

fn account_section_label(label: &'static str, colors: SemanticColors) -> AnyElement {
    div()
        .px(px(12.0))
        .pt(px(6.0))
        .pb(px(2.0))
        .text_size(px(Typo::SECTION_HEADER.size))
        .font_weight(Typo::SECTION_HEADER.weight)
        .text_color(colors.tertiary)
        .child(label)
        .into_any_element()
}

fn account_usage_row(label: &str, detail: &str, value: &str, colors: SemanticColors) -> AnyElement {
    div()
        .px(px(12.0))
        .h(px(Metrics::MENU_ROW_HEIGHT))
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_size(px(Typo::ROW.size))
        .text_color(colors.text(taskmaster_ui::TextTone::Label))
        .child(label.to_owned())
        .child(div().flex_1())
        .when(!detail.is_empty(), |row| {
            row.child(
                div()
                    .text_size(px(Typo::META.size))
                    .text_color(colors.tertiary)
                    .child(detail.to_owned()),
            )
        })
        .child(
            div()
                .font_family(crate::fonts::mono_family())
                .text_size(px(Typo::META_MONO.size))
                .text_color(colors.secondary)
                .child(value.to_owned()),
        )
        .into_any_element()
}

fn compact_duration(seconds: i64) -> String {
    let minutes = (seconds / 60).max(0);
    if minutes >= 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui::{Entity, TestAppContext};

    use super::*;

    struct MenuHarness {
        menu: Entity<AccountMenu>,
        background_keys: Rc<Cell<usize>>,
    }

    impl Render for MenuHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let background_keys = Rc::clone(&self.background_keys);
            div()
                .size_full()
                .flex()
                .flex_col()
                .justify_end()
                .on_key_down(move |_, _, _| background_keys.set(background_keys.get() + 1))
                .child(
                    div()
                        .w(px(184.0))
                        .px(px(8.0))
                        .pb(px(8.0))
                        .child(self.menu.clone()),
                )
        }
    }

    #[gpui::test]
    fn keyboard_opens_navigates_and_dismisses_without_reaching_background(cx: &mut TestAppContext) {
        let background_keys = Rc::new(Cell::new(0));
        let events = Rc::new(RefCell::new(Vec::new()));
        let test_keys = Rc::clone(&background_keys);
        let recorded_events = Rc::clone(&events);
        let (harness, cx) = cx.add_window_view(move |_, cx| {
            let menu = cx.new(|cx| AccountMenu::new(Arc::new(StoreRuntime::inert()), true, cx));
            cx.subscribe(&menu, move |_, _, event, _| {
                recorded_events.borrow_mut().push(event.clone())
            })
            .detach();
            MenuHarness {
                menu,
                background_keys: test_keys,
            }
        });
        let menu = harness.read_with(cx, |harness, _| harness.menu.clone());
        cx.update(|window, cx| {
            let focus = menu.read(cx).focus.clone();
            window.focus(&focus, cx);
        });

        cx.simulate_keystrokes("enter");
        assert!(cx.debug_bounds("ACCOUNT_MENU").is_some());
        assert_eq!(
            menu.read_with(cx, |menu, _| menu.focused_action),
            Some(AccountAction::Update)
        );
        cx.simulate_keystrokes("down down");
        assert_eq!(
            menu.read_with(cx, |menu, _| menu.focused_action),
            Some(AccountAction::Settings)
        );
        cx.simulate_keystrokes("x enter");
        assert!(cx.debug_bounds("ACCOUNT_MENU").is_none());
        assert!(matches!(
            events.borrow().as_slice(),
            [AccountMenuEvent::OpenSettings]
        ));
        assert!(cx.update(|window, cx| menu.read(cx).is_focused(window, cx)));
        assert_eq!(background_keys.get(), 0);

        cx.simulate_keystrokes("space escape");
        assert!(cx.debug_bounds("ACCOUNT_MENU").is_none());
        assert!(cx.update(|window, cx| menu.read(cx).focus.is_focused(window)));
        cx.simulate_keystrokes("space");
        assert!(cx.debug_bounds("ACCOUNT_MENU").is_some());
        cx.simulate_keystrokes("end enter");
        assert!(cx.debug_bounds("ACCOUNT_MENU").is_none());
        assert_eq!(events.borrow().len(), 1);
        assert_eq!(background_keys.get(), 0);
    }

    #[gpui::test]
    fn keyboard_dispatches_update_remote_and_documentation_actions(cx: &mut TestAppContext) {
        let events = Rc::new(RefCell::new(Vec::new()));
        let recorded_events = Rc::clone(&events);
        let (harness, cx) = cx.add_window_view(move |_, cx| {
            let menu = cx.new(|cx| AccountMenu::new(Arc::new(StoreRuntime::inert()), true, cx));
            cx.subscribe(&menu, move |_, _, event, _| {
                recorded_events.borrow_mut().push(event.clone())
            })
            .detach();
            MenuHarness {
                menu,
                background_keys: Rc::new(Cell::new(0)),
            }
        });
        let menu = harness.read_with(cx, |harness, _| harness.menu.clone());
        cx.update(|window, cx| {
            let focus = menu.read(cx).focus.clone();
            window.focus(&focus, cx);
        });

        cx.simulate_keystrokes("enter enter");
        assert!(matches!(
            events.borrow().as_slice(),
            [AccountMenuEvent::Update(UpdateCommand::Check {
                user_initiated: true
            })]
        ));
        assert!(cx.debug_bounds("ACCOUNT_MENU").is_none());
        cx.simulate_keystrokes("space down enter");
        assert!(matches!(
            events.borrow().last(),
            Some(AccountMenuEvent::AddRemoteHost)
        ));
        cx.simulate_keystrokes("space up up up enter");
        assert_eq!(cx.opened_url().as_deref(), Some(crate::settings::DOCS_URL));
        cx.simulate_keystrokes("space up up enter");
        assert_eq!(
            cx.opened_url().as_deref(),
            Some(crate::settings::DEVELOPER_DOCS_URL)
        );
        assert!(cx.debug_bounds("ACCOUNT_MENU").is_none());
    }

    #[gpui::test]
    fn keyboard_skips_unavailable_updates_and_retains_focus_when_update_changes(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = cx.add_window_view(|_, cx| {
            let menu = cx.new(|cx| {
                let mut menu = AccountMenu::new(Arc::new(StoreRuntime::inert()), true, cx);
                menu.set_update(
                    UpdateState {
                        phase: UpdatePhase::Unsupported("development build".into()),
                        ..UpdateState::default()
                    },
                    cx,
                );
                menu
            });
            MenuHarness {
                menu,
                background_keys: Rc::new(Cell::new(0)),
            }
        });
        let menu = harness.read_with(cx, |harness, _| harness.menu.clone());
        cx.update(|window, cx| {
            let focus = menu.read(cx).focus.clone();
            window.focus(&focus, cx);
        });

        cx.simulate_keystrokes("space");
        assert_eq!(
            menu.read_with(cx, |menu, _| menu.focused_action),
            Some(AccountAction::AddRemoteHost)
        );
        cx.simulate_keystrokes("down");
        assert_eq!(
            menu.read_with(cx, |menu, _| menu.focused_action),
            Some(AccountAction::Settings)
        );
        cx.simulate_keystrokes("shift-tab");
        assert_eq!(
            menu.read_with(cx, |menu, _| menu.focused_action),
            Some(AccountAction::AddRemoteHost)
        );
        menu.update(cx, |menu, cx| menu.set_update(UpdateState::default(), cx));
        cx.simulate_keystrokes("home");
        assert_eq!(
            menu.read_with(cx, |menu, _| menu.focused_action),
            Some(AccountAction::Update)
        );
        menu.update(cx, |menu, cx| {
            menu.set_update(
                UpdateState {
                    phase: UpdatePhase::Checking,
                    ..UpdateState::default()
                },
                cx,
            )
        });
        assert_eq!(
            menu.read_with(cx, |menu, _| menu.focused_action),
            Some(AccountAction::AddRemoteHost)
        );
        assert!(cx.debug_bounds("ACCOUNT_MENU").is_some());
        cx.simulate_keystrokes("escape");
        assert!(cx.debug_bounds("ACCOUNT_MENU").is_none());
    }
}
