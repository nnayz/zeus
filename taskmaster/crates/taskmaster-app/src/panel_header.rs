//! Compact panel tabs and the menu for opening additional tools.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    Anchor, AnyElement, Bounds, Context, EventEmitter, FocusHandle, KeyDownEvent, MouseButton,
    Pixels, Render, Role, ScrollHandle, Window, anchored, canvas, deferred, div, point, prelude::*,
    px, rgb,
};
use taskmaster_ui::{Appearance, Icon, IconName, Metrics, SemanticColors, Typo};

use crate::panel_tabs::{PanelTab, PanelTabKind, PanelTabs};

pub(crate) const PANEL_HEADER_HEIGHT: f32 = Metrics::TITLE_BAR;
const PANEL_CONTROL_SIZE: f32 = 32.0;
const PANEL_TAB_HEIGHT: f32 = 28.0;
const NATIVE_WINDOW_LANE: f32 = 82.0;
const MENU_KINDS: [PanelTabKind; 5] = [
    PanelTabKind::Files,
    PanelTabKind::Review,
    PanelTabKind::Details,
    PanelTabKind::Terminal,
    PanelTabKind::Browser,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PanelHeaderEvent {
    Open(PanelTabKind),
    Select(u64),
    Close(u64),
    ClosePanel,
    MenuChanged(bool),
}

pub(crate) struct PanelHeader {
    tabs: PanelTabs,
    appearance: Appearance,
    mirrored: bool,
    menu_open: bool,
    menu_index: usize,
    tab_focus: HashMap<u64, FocusHandle>,
    add_focus: FocusHandle,
    collapse_focus: FocusHandle,
    menu_focus: FocusHandle,
    add_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    scroll: ScrollHandle,
    reveal_after_layout: bool,
}

impl EventEmitter<PanelHeaderEvent> for PanelHeader {}

impl PanelHeader {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            tabs: PanelTabs::empty(),
            appearance: Appearance::Dark,
            mirrored: false,
            menu_open: false,
            menu_index: 0,
            tab_focus: HashMap::new(),
            add_focus: cx.focus_handle(),
            collapse_focus: cx.focus_handle(),
            menu_focus: cx.focus_handle(),
            add_bounds: Rc::new(Cell::new(None)),
            scroll: ScrollHandle::new(),
            reveal_after_layout: false,
        }
    }

    pub(crate) fn set_state(
        &mut self,
        mut tabs: PanelTabs,
        appearance: Appearance,
        mirrored: bool,
        cx: &mut Context<Self>,
    ) {
        tabs.normalize();
        if self.tabs == tabs && self.appearance == appearance && self.mirrored == mirrored {
            return;
        }
        self.tab_focus
            .retain(|id, _| tabs.tabs.iter().any(|tab| tab.id == *id));
        for tab in &tabs.tabs {
            self.tab_focus
                .entry(tab.id)
                .or_insert_with(|| cx.focus_handle());
        }
        if self.tabs.active != tabs.active
            && let Some(index) = tabs.tabs.iter().position(|tab| Some(tab.id) == tabs.active)
        {
            self.scroll.scroll_to_item(index);
            // GPUI initializes the scroll viewport during the first prepaint.
            // Reveal a restored active tab again once that viewport exists.
            self.reveal_after_layout = self.scroll.bounds().size.width <= px(0.0);
        }
        self.tabs = tabs;
        self.appearance = appearance;
        self.mirrored = mirrored;
        cx.notify();
    }

    pub(crate) fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.menu_open {
            self.menu_open = false;
            cx.emit(PanelHeaderEvent::MenuChanged(false));
            cx.notify();
        }
    }

    pub(crate) fn is_menu_open(&self) -> bool {
        self.menu_open
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu_open {
            window.focus(&self.menu_focus, cx);
            return;
        }
        let focus = self
            .tabs
            .active
            .and_then(|id| self.tab_focus.get(&id))
            .unwrap_or(&self.add_focus);
        window.focus(focus, cx);
    }

    pub(crate) fn open_add_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.menu_open {
            self.toggle_menu(window, cx);
        }
    }

    pub(crate) fn is_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.add_focus.contains_focused(window, cx)
            || self.collapse_focus.contains_focused(window, cx)
            || self.menu_focus.contains_focused(window, cx)
            || self
                .tab_focus
                .values()
                .any(|focus| focus.contains_focused(window, cx))
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss(cx);
        window.focus(&self.add_focus, cx);
    }

    fn toggle_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu_open {
            self.close_menu(window, cx);
        } else {
            self.menu_open = true;
            self.menu_index = 0;
            window.focus(&self.menu_focus, cx);
            cx.emit(PanelHeaderEvent::MenuChanged(true));
            cx.notify();
        }
    }

    fn open_kind(&mut self, kind: PanelTabKind, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(window, cx);
        cx.emit(PanelHeaderEvent::Open(kind));
    }

    fn select_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss(cx);
        if let Some(focus) = self.tab_focus.get(&id) {
            window.focus(focus, cx);
        }
        if self.tabs.select(id) {
            if let Some(index) = self.tabs.tabs.iter().position(|tab| tab.id == id) {
                self.scroll.scroll_to_item(index);
            }
            cx.emit(PanelHeaderEvent::Select(id));
            cx.notify();
        }
    }

    fn close_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self
            .tab_focus
            .get(&id)
            .is_some_and(|focus| focus.contains_focused(window, cx));
        let closed = if self.tabs.active == Some(id) {
            self.tabs.close_active()
        } else {
            self.tabs.close(id)
        };
        if !closed {
            return;
        }
        self.tab_focus.remove(&id);
        if focused {
            let focus = self
                .tabs
                .active
                .and_then(|id| self.tab_focus.get(&id))
                .unwrap_or(&self.add_focus);
            window.focus(focus, cx);
        }
        cx.emit(PanelHeaderEvent::Close(id));
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        let focused_tab = self
            .tab_focus
            .iter()
            .find(|(_, focus)| focus.contains_focused(window, cx))
            .map(|(id, _)| *id);
        if (modifiers.platform || modifiers.control)
            && !modifiers.alt
            && key == "w"
            && let Some(id) = focused_tab
        {
            self.close_tab(id, window, cx);
        } else if modifiers.platform || modifiers.control || modifiers.alt {
            return;
        } else if self.menu_open {
            match key {
                "enter" | "space" => self.open_kind(MENU_KINDS[self.menu_index], window, cx),
                "escape" => self.close_menu(window, cx),
                "up" | "down" | "tab" => {
                    let backwards = key == "up" || key == "tab" && modifiers.shift;
                    self.menu_index = if backwards {
                        (self.menu_index + MENU_KINDS.len() - 1) % MENU_KINDS.len()
                    } else {
                        (self.menu_index + 1) % MENU_KINDS.len()
                    };
                    cx.notify();
                }
                "home" => {
                    self.menu_index = 0;
                    cx.notify();
                }
                "end" => {
                    self.menu_index = MENU_KINDS.len() - 1;
                    cx.notify();
                }
                _ => {}
            }
        } else {
            match key {
                "enter" | "space" if self.add_focus.contains_focused(window, cx) => {
                    self.toggle_menu(window, cx)
                }
                "enter" | "space" if self.collapse_focus.contains_focused(window, cx) => {
                    cx.emit(PanelHeaderEvent::ClosePanel)
                }
                "enter" | "space" => {
                    if let Some(id) = focused_tab {
                        self.select_tab(id, window, cx);
                    }
                }
                "left" | "right" | "home" | "end" if focused_tab.is_some() => {
                    let id = match key {
                        "left" | "right" => {
                            // Resolve relative to keyboard focus, then let
                            // select_tab publish the resulting selection.
                            let previous = self.tabs.active;
                            self.tabs.active = focused_tab;
                            self.tabs
                                .select_relative(if key == "left" { -1 } else { 1 });
                            let id = self.tabs.active.unwrap();
                            self.tabs.active = previous;
                            id
                        }
                        "home" => self.tabs.tabs.first().unwrap().id,
                        _ => self.tabs.tabs.last().unwrap().id,
                    };
                    self.select_tab(id, window, cx);
                }
                "delete" | "backspace" if focused_tab.is_some() => {
                    self.close_tab(focused_tab.unwrap(), window, cx)
                }
                "tab" => return,
                _ => {}
            }
        }
        // Consume before GPUI's Enter/Space synthesis and before terminal listeners.
        window.prevent_default();
        cx.stop_propagation();
    }

    fn tab(&self, tab: &PanelTab, colors: SemanticColors, cx: &mut Context<Self>) -> AnyElement {
        let id = tab.id;
        let selected = self.tabs.active == Some(id);
        let focus = self
            .tab_focus
            .get(&id)
            .expect("panel tab focus initialized");
        let appearance = self.appearance;
        div()
            .id(("panel-tab", id))
            .debug_selector(move || format!("PANEL_TAB_{id}"))
            .role(Role::Tab)
            .aria_label(tab.label().to_owned())
            .aria_selected(selected)
            .track_focus(focus)
            .tab_stop(selected)
            .flex_none()
            .h(px(PANEL_TAB_HEIGHT))
            .max_w(px(160.0))
            .pl(px(8.0))
            .pr(px(4.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .rounded(px(6.0))
            .border_2()
            .border_color(colors.primary.alpha(0.0))
            .focus_visible(move |tab| tab.border_color(focus_color(appearance)))
            .bg(colors.primary.alpha(if selected { 0.08 } else { 0.0 }))
            .hover(move |tab| tab.bg(colors.primary.alpha(if selected { 0.10 } else { 0.05 })))
            .text_color(if selected {
                colors.primary
            } else {
                colors.secondary
            })
            .text_size(px(Typo::ROW.size))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_tab(id, window, cx);
                cx.stop_propagation();
            }))
            .child(
                Icon::from_system_name(tab.icon(), 14.0, colors.secondary).expect("panel tab icon"),
            )
            .child(
                div()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(tab.label().to_owned()),
            )
            .child(
                div()
                    .id(("panel-tab-close", id))
                    .debug_selector(move || format!("PANEL_TAB_CLOSE_{id}"))
                    .role(Role::Button)
                    .aria_label(format!("Close {}", tab.label()))
                    .flex_none()
                    .w(px(22.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(move |button| button.bg(colors.primary.alpha(0.08)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_tab(id, window, cx);
                        cx.stop_propagation();
                    }))
                    .child(Icon::new(IconName::Close, 12.0, colors.secondary)),
            )
            .into_any_element()
    }

    fn menu(&self, window: &Window, colors: SemanticColors, cx: &mut Context<Self>) -> AnyElement {
        let bounds = self.add_bounds.get();
        let position = bounds.map_or(point(px(168.0), px(PANEL_HEADER_HEIGHT + 4.0)), |bounds| {
            point(bounds.right(), bounds.bottom() + px(4.0))
        });
        let mut rows = div().py(px(4.0));
        for (index, kind) in MENU_KINDS.into_iter().enumerate() {
            let selected = index == self.menu_index;
            rows = rows.child(
                div()
                    .id(("panel-open-kind", index))
                    .debug_selector(move || format!("PANEL_OPEN_{}", kind.label().to_uppercase()))
                    .role(Role::MenuItem)
                    .aria_label(format!("Open {} tab", kind.label()))
                    .aria_selected(selected)
                    .when(selected, |row| row.aria_active_descendant())
                    .mx(px(4.0))
                    .h(px(28.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(px(6.0))
                    .border_2()
                    .border_color(colors.primary.alpha(if selected { 0.28 } else { 0.0 }))
                    .bg(colors.primary.alpha(if selected { 0.08 } else { 0.0 }))
                    .hover(move |row| row.bg(colors.primary.alpha(0.06)))
                    .text_size(px(Typo::ROW.size))
                    .text_color(colors.primary)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_kind(kind, window, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        Icon::from_system_name(kind.icon(), 14.0, colors.secondary)
                            .expect("panel menu icon"),
                    )
                    .child(kind.label()),
            );
        }
        deferred(
            anchored()
                .position(position)
                .anchor(Anchor::TopRight)
                .snap_to_window_with_margin(px(8.0))
                .child(
                    div()
                        .id("panel-open-menu")
                        .debug_selector(|| "PANEL_OPEN_MENU".to_owned())
                        .role(Role::Menu)
                        .aria_label("Open panel tab")
                        .track_focus(&self.menu_focus)
                        .capture_key_down(cx.listener(Self::key_down))
                        .w(px(168.0))
                        .max_h(px((f32::from(window.viewport_size().height)
                            - PANEL_HEADER_HEIGHT
                            - 16.0)
                            .max(1.0)))
                        .overflow_y_scroll()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(colors.primary.alpha(0.10))
                        .bg(colors.floating_surface())
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down_out(
                            cx.listener(|this, _, window, cx| this.close_menu(window, cx)),
                        )
                        .child(rows),
                ),
        )
        .with_priority(2)
        .into_any_element()
    }
}

impl Render for PanelHeader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.reveal_after_layout) {
            cx.on_next_frame(window, |this, _, cx| {
                if let Some(index) = this
                    .tabs
                    .tabs
                    .iter()
                    .position(|tab| Some(tab.id) == this.tabs.active)
                {
                    this.scroll.scroll_to_item(index);
                    cx.notify();
                }
            });
        }
        let colors = colors(self.appearance);
        let appearance = self.appearance;
        let add_bounds = Rc::clone(&self.add_bounds);
        let mut strip = div()
            .id("panel-tab-strip")
            .debug_selector(|| "PANEL_TAB_STRIP".to_owned())
            .role(Role::TabList)
            .aria_label("Panel tabs")
            .min_w(px(0.0))
            .h_full()
            .flex_1()
            .flex()
            .items_center()
            .gap(px(4.0))
            .overflow_x_scroll()
            .track_scroll(&self.scroll);
        for tab in &self.tabs.tabs {
            strip = strip.child(self.tab(tab, colors, cx));
        }
        div()
            .id("panel-header")
            .debug_selector(|| "PANEL_HEADER".to_owned())
            .w_full()
            .h(px(PANEL_HEADER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(if self.mirrored {
                NATIVE_WINDOW_LANE
            } else {
                8.0
            }))
            .pr(px(8.0))
            .gap(px(4.0))
            .capture_key_down(cx.listener(Self::key_down))
            .child(strip)
            .child(
                div()
                    .id("panel-add-tab")
                    .debug_selector(|| "PANEL_ADD_TAB".to_owned())
                    .relative()
                    .role(Role::Button)
                    .aria_label("Open panel tab")
                    .aria_expanded(self.menu_open)
                    .track_focus(&self.add_focus)
                    .tab_stop(true)
                    .flex_none()
                    .w(px(PANEL_CONTROL_SIZE))
                    .h(px(PANEL_TAB_HEIGHT))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .border_2()
                    .border_color(colors.primary.alpha(0.0))
                    .focus_visible(move |button| button.border_color(focus_color(appearance)))
                    .bg(colors
                        .primary
                        .alpha(if self.menu_open { 0.08 } else { 0.0 }))
                    .hover(move |button| button.bg(colors.primary.alpha(0.05)))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_menu(window, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        canvas(
                            move |bounds, _, _| add_bounds.set(Some(bounds)),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(Icon::new(IconName::Plus, 14.0, colors.secondary)),
            )
            .child(
                div()
                    .id("panel-collapse")
                    .debug_selector(|| "PANEL_COLLAPSE".to_owned())
                    .role(Role::Button)
                    .aria_label("Hide panel")
                    .track_focus(&self.collapse_focus)
                    .tab_stop(true)
                    .flex_none()
                    .w(px(PANEL_CONTROL_SIZE))
                    .h(px(PANEL_TAB_HEIGHT))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .border_2()
                    .border_color(colors.primary.alpha(0.0))
                    .focus_visible(move |button| button.border_color(focus_color(appearance)))
                    .hover(move |button| button.bg(colors.primary.alpha(0.05)))
                    .cursor_pointer()
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(PanelHeaderEvent::ClosePanel);
                        cx.stop_propagation();
                    }))
                    .child(Icon::new(
                        if self.mirrored {
                            IconName::Sidebar
                        } else {
                            IconName::SidebarRight
                        },
                        14.0,
                        colors.secondary,
                    )),
            )
            .when(self.menu_open, |header| {
                header.child(self.menu(window, colors, cx))
            })
    }
}

fn focus_color(appearance: Appearance) -> gpui::Rgba {
    rgb(match appearance {
        Appearance::Light => 0x006bd6,
        Appearance::Dark => 0x2997ff,
    })
}

fn colors(appearance: Appearance) -> SemanticColors {
    let (surface, raised, foreground, secondary, muted) = match appearance {
        Appearance::Light => (0xfcfcfc, 0xffffff, 0x191919, 0x6e6e6e, 0x737373),
        Appearance::Dark => (0x0d0d0d, 0x161616, 0xe6e6e6, 0xa0a0a0, 0x8c8c8c),
    };
    let mut colors = SemanticColors::themed(
        appearance,
        rgb(surface),
        rgb(foreground),
        rgb(surface),
        rgb(raised),
        true,
    );
    colors.secondary = rgb(secondary);
    colors.tertiary = rgb(muted);
    colors
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui::{Entity, TestAppContext};

    use super::*;

    struct HeaderHarness {
        header: Entity<PanelHeader>,
        background_keys: Rc<Cell<usize>>,
    }

    impl Render for HeaderHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let keys = Rc::clone(&self.background_keys);
            div()
                .size_full()
                .on_key_down(move |_, _, _| keys.set(keys.get() + 1))
                .child(div().w(px(320.0)).child(self.header.clone()))
        }
    }

    #[gpui::test]
    fn add_menu_keyboard_dispatches_once_and_never_reaches_background(cx: &mut TestAppContext) {
        let events = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&events);
        let (harness, cx) = cx.add_window_view(move |_, cx| {
            let header = cx.new(PanelHeader::new);
            cx.subscribe(&header, move |_, _, event, _| {
                recorded.borrow_mut().push(*event)
            })
            .detach();
            HeaderHarness {
                header,
                background_keys: Rc::new(Cell::new(0)),
            }
        });
        let header = harness.read_with(cx, |harness, _| harness.header.clone());
        cx.update(|window, cx| window.focus(&header.read(cx).add_focus.clone(), cx));
        cx.simulate_keystrokes("enter down x enter");
        assert_eq!(
            *events.borrow(),
            vec![
                PanelHeaderEvent::MenuChanged(true),
                PanelHeaderEvent::MenuChanged(false),
                PanelHeaderEvent::Open(PanelTabKind::Review),
            ]
        );
        assert!(cx.debug_bounds("PANEL_OPEN_MENU").is_none());
        assert!(cx.update(|window, cx| header.read(cx).add_focus.is_focused(window)));
        cx.simulate_keystrokes("space escape");
        assert!(cx.debug_bounds("PANEL_OPEN_MENU").is_none());
        assert_eq!(
            harness.read_with(cx, |harness, _| harness.background_keys.get()),
            0
        );
    }

    #[gpui::test]
    fn tab_keyboard_selects_and_closes_neighbor_then_empty(cx: &mut TestAppContext) {
        let events = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&events);
        let (harness, cx) = cx.add_window_view(move |_, cx| {
            let header = cx.new(|cx| {
                let mut header = PanelHeader::new(cx);
                let mut tabs = PanelTabs::default();
                tabs.open(PanelTabKind::Review);
                tabs.open(PanelTabKind::Files);
                tabs.select(1);
                header.set_state(tabs, Appearance::Dark, false, cx);
                header
            });
            cx.subscribe(&header, move |_, _, event, _| {
                recorded.borrow_mut().push(*event)
            })
            .detach();
            HeaderHarness {
                header,
                background_keys: Rc::new(Cell::new(0)),
            }
        });
        let header = harness.read_with(cx, |harness, _| harness.header.clone());
        cx.update(|window, cx| window.focus(&header.read(cx).tab_focus[&1].clone(), cx));
        cx.simulate_keystrokes("right delete");
        assert_eq!(
            *events.borrow(),
            vec![PanelHeaderEvent::Select(2), PanelHeaderEvent::Close(2)]
        );
        assert_eq!(
            header.read_with(cx, |header, _| header.tabs.active),
            Some(3)
        );
        assert!(cx.update(|window, cx| header.read(cx).tab_focus[&3].is_focused(window)));
        cx.simulate_keystrokes("delete delete");
        assert!(header.read_with(cx, |header, _| header.tabs.tabs.is_empty()));
        assert!(cx.update(|window, cx| header.read(cx).add_focus.is_focused(window)));
        assert!(cx.debug_bounds("PANEL_ADD_TAB").is_some());
        assert_eq!(
            harness.read_with(cx, |harness, _| harness.background_keys.get()),
            0
        );
    }

    #[gpui::test]
    fn restored_active_tab_scrolls_into_view_after_layout(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, cx| {
            let header = cx.new(|cx| {
                let mut header = PanelHeader::new(cx);
                let mut tabs = PanelTabs::default();
                for _ in 0..12 {
                    tabs.open(PanelTabKind::Terminal);
                }
                header.set_state(tabs, Appearance::Dark, false, cx);
                header
            });
            HeaderHarness {
                header,
                background_keys: Rc::new(Cell::new(0)),
            }
        });
        cx.update(|window, cx| window.simulate_next_frame(cx));
        let strip = cx.debug_bounds("PANEL_TAB_STRIP").unwrap();
        let last = cx.debug_bounds("PANEL_TAB_13").unwrap();
        assert!(last.left() >= strip.left());
        assert!(last.right() <= strip.right());
    }

    #[gpui::test]
    fn mirrored_header_keeps_native_lane_and_pinned_controls_with_overflow(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = cx.add_window_view(|_, cx| {
            let header = cx.new(|cx| {
                let mut header = PanelHeader::new(cx);
                let mut tabs = PanelTabs::default();
                for _ in 0..12 {
                    tabs.open(PanelTabKind::Terminal);
                }
                tabs.select(1);
                header.set_state(tabs, Appearance::Light, true, cx);
                header
            });
            HeaderHarness {
                header,
                background_keys: Rc::new(Cell::new(0)),
            }
        });
        let header = harness.read_with(cx, |harness, _| harness.header.clone());
        let bounds = cx.debug_bounds("PANEL_HEADER").unwrap();
        let strip = cx.debug_bounds("PANEL_TAB_STRIP").unwrap();
        let add = cx.debug_bounds("PANEL_ADD_TAB").unwrap();
        let collapse = cx.debug_bounds("PANEL_COLLAPSE").unwrap();
        assert_eq!(bounds.size.height, px(PANEL_HEADER_HEIGHT));
        assert_eq!(strip.left(), bounds.left() + px(NATIVE_WINDOW_LANE));
        assert!(strip.right() <= add.left());
        assert_eq!(add.size.width, px(PANEL_CONTROL_SIZE));
        assert_eq!(collapse.size.width, px(PANEL_CONTROL_SIZE));
        assert_eq!(collapse.right(), bounds.right() - px(8.0));
        cx.update(|window, cx| header.update(cx, |header, cx| header.focus(window, cx)));
        cx.simulate_keystrokes("end");
        let last = cx.debug_bounds("PANEL_TAB_13").unwrap();
        assert!(last.left() >= strip.left());
        assert!(last.right() <= strip.right());
        cx.simulate_click(add.center(), Default::default());
        let menu = cx.debug_bounds("PANEL_OPEN_MENU").unwrap();
        assert!(menu.top() >= add.bottom());
        assert!(menu.right() <= add.right() + px(1.0));
        header.update(cx, |header, cx| {
            header.dismiss(cx);
            header.set_state(PanelTabs::empty(), Appearance::Dark, false, cx);
        });
        assert!(cx.debug_bounds("PANEL_ADD_TAB").is_some());
        assert!(cx.debug_bounds("PANEL_COLLAPSE").is_some());
    }
}
