//! One independently retained Browser tab, with a compact GPUI address bar.

#[cfg(target_os = "macos")]
use std::cell::RefCell;
#[cfg(target_os = "macos")]
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, HighlightStyle, KeyDownEvent,
    Render, Role, SharedString, Task, Window, canvas, div, prelude::*, px,
};
use taskmaster_ui::{Appearance, Icon, IconName, Typo};

use crate::query_editor::{self, ClipboardEdit, Edit, QueryEditor};
use crate::workspace_colors::WorkspaceColors;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BrowserState {
    pub(crate) url: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) loading: bool,
    pub(crate) can_go_back: bool,
    pub(crate) can_go_forward: bool,
    pub(crate) error: Option<String>,
}

pub(crate) struct BrowserPanel {
    preview: bool,
    native_preview: bool,
    appearance: Option<Appearance>,
    visible: bool,
    focus: FocusHandle,
    page_focus: FocusHandle,
    address: QueryEditor,
    target: Option<String>,
    state: BrowserState,
    #[cfg(target_os = "macos")]
    sender: tokio::sync::mpsc::UnboundedSender<BrowserState>,
    #[cfg(target_os = "macos")]
    native: Option<Rc<RefCell<crate::macos::browser::NativeBrowser>>>,
    _events: Task<()>,
}

pub(crate) enum BrowserPanelEvent {
    Changed,
}

impl EventEmitter<BrowserPanelEvent> for BrowserPanel {}

impl Focusable for BrowserPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl BrowserPanel {
    pub(crate) fn new(preview: bool, cx: &mut Context<Self>) -> Self {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<BrowserState>();
        #[cfg(not(target_os = "macos"))]
        let _ = sender;
        let events = cx.spawn(async move |this, cx| {
            while let Some(state) = receiver.recv().await {
                if this
                    .update(cx, |this, cx| {
                        // A cancelled old navigation cannot reopen a blank
                        // tab or override an ordinary offline preview.
                        if this.target.is_none() || (this.preview && !this.native_preview) {
                            return;
                        }
                        if !state.loading
                            && state.error.is_none()
                            && let Some(url) = &state.url
                        {
                            let unchanged =
                                this.address.text() == this.target.as_deref().unwrap_or("");
                            this.target = Some(url.clone());
                            // Preserve an address being edited while a page loads.
                            if unchanged {
                                this.address.reset(url.clone(), url.len());
                            }
                        }
                        this.state = state;
                        #[cfg(target_os = "macos")]
                        if this.state.error.is_some()
                            && let Some(native) = &this.native
                        {
                            native.borrow_mut().hide();
                        }
                        cx.emit(BrowserPanelEvent::Changed);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            preview,
            native_preview: false,
            appearance: None,
            visible: false,
            focus: cx.focus_handle(),
            page_focus: cx.focus_handle(),
            address: QueryEditor::default(),
            target: None,
            state: BrowserState::default(),
            #[cfg(target_os = "macos")]
            sender,
            #[cfg(target_os = "macos")]
            native: None,
            _events: events,
        }
    }

    /// Call whenever this tab or the containing panel is hidden, and while a
    /// GPUI overlay covers the page. Native NSViews sit above GPUI's drawing.
    pub(crate) fn set_visible(
        &mut self,
        visible: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.visible == visible {
            return;
        }
        if !visible {
            self.deactivate(cx);
            return;
        }
        self.visible = visible;
        cx.notify();
    }

    pub(crate) fn deactivate(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        #[cfg(target_os = "macos")]
        if let Some(native) = &self.native {
            native.borrow_mut().hide();
        }
        cx.notify();
    }

    pub(crate) fn set_appearance(&mut self, appearance: Appearance, cx: &mut Context<Self>) {
        if self.appearance == Some(appearance) {
            return;
        }
        self.appearance = Some(appearance);
        #[cfg(target_os = "macos")]
        if let Some(native) = &self.native {
            native.borrow().set_appearance(appearance);
        }
        cx.notify();
    }

    pub(crate) fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if let Some(native) = &self.native {
            native.borrow().return_focus();
        }
        self.address.select_all();
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(crate) fn is_focused(&self, window: &Window, _cx: &App) -> bool {
        if self.focus.is_focused(window) || self.page_focus.is_focused(window) {
            return true;
        }
        #[cfg(target_os = "macos")]
        if let Some(native) = &self.native {
            return native.borrow().is_focused();
        }
        false
    }

    pub(crate) fn navigate(&mut self, address: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.native_preview = false;
        let target = match normalize_url(address) {
            Ok(target) => target,
            Err(error) => {
                self.state.error = Some(error);
                cx.notify();
                return;
            }
        };
        let Some(target) = target else {
            self.target = None;
            self.state = BrowserState::default();
            self.address.clear();
            #[cfg(target_os = "macos")]
            if let Some(native) = &self.native {
                let mut native = native.borrow_mut();
                native.hide();
                native.stop();
            }
            cx.emit(BrowserPanelEvent::Changed);
            cx.notify();
            return;
        };
        self.address.reset(target.clone(), target.len());
        self.target = Some(target.clone());
        self.state.error = None;
        if self.preview {
            self.state.error =
                Some("Web pages open in the running app. Preview mode stays offline.".into());
            cx.notify();
            return;
        }
        #[cfg(target_os = "macos")]
        {
            if self.native.is_none() {
                match crate::macos::browser::NativeBrowser::new(self.sender.clone()) {
                    Ok(native) => {
                        if let Some(appearance) = self.appearance {
                            native.set_appearance(appearance);
                        }
                        self.native = Some(Rc::new(RefCell::new(native)));
                    }
                    Err(error) => {
                        self.state.error = Some(error);
                        cx.notify();
                        return;
                    }
                }
            }
            if let Some(native) = &self.native {
                match native.borrow().navigate(&target) {
                    Ok(()) => self.state.loading = true,
                    Err(error) => self.state.error = Some(error),
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.state.error = Some("Embedded browsing is available on macOS.".into());
        }
        // Keep a GPUI focus path for window actions while WebKit takes the
        // native first responder. Blurring GPUI would disable app shortcuts.
        window.focus(&self.page_focus, cx);
        cx.notify();
    }

    /// Explicit, offline native smoke preview. The caller opts into this hook;
    /// ordinary fixture previews never create a WebKit process or open a URL.
    pub(crate) fn preview_content(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.preview {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            if self.native.is_none() {
                match crate::macos::browser::NativeBrowser::new(self.sender.clone()) {
                    Ok(native) => {
                        if let Some(appearance) = self.appearance {
                            native.set_appearance(appearance);
                        }
                        self.native = Some(Rc::new(RefCell::new(native)));
                    }
                    Err(error) => {
                        self.state.error = Some(error);
                        cx.notify();
                        return;
                    }
                }
            }
            if let Some(native) = &self.native {
                self.native_preview = true;
                self.target = Some("about:blank".into());
                self.state.error = None;
                self.state.loading = true;
                native.borrow().load_preview_html();
                cx.notify();
            }
        }
    }

    pub(crate) fn target_url(&self) -> Option<&str> {
        self.target.as_deref()
    }

    /// Native fixture smoke only: verify the offline page is mounted, then
    /// send Command-O through AppKit while the page owns first responder.
    pub(crate) fn native_smoke(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.preview || !self.native_preview {
            return Err("Native browser smoke requires the offline preview.".into());
        }
        #[cfg(target_os = "macos")]
        {
            let native = self
                .native
                .as_ref()
                .ok_or("The browser preview has not initialized.")?;
            window.focus(&self.page_focus, cx);
            native.borrow().queue_preview_command_o()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (window, cx);
            Err("Native browser smoke requires macOS.".into())
        }
    }

    pub(crate) fn title(&self) -> &str {
        self.state
            .title
            .as_deref()
            .filter(|title| !title.is_empty())
            .unwrap_or("Browser")
    }

    pub(crate) fn check_native_preview_visibility(&self, visible: bool) -> Result<(), String> {
        if !self.preview || !self.native_preview {
            return Err("Native visibility checks require the offline preview.".into());
        }
        #[cfg(target_os = "macos")]
        {
            self.native
                .as_ref()
                .ok_or("The browser preview has not initialized.")?
                .borrow()
                .check_preview_visibility(visible)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = visible;
            Err("Native visibility checks require macOS.".into())
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        if event.keystroke.key == "enter" {
            let address = self.address.text().to_owned();
            self.navigate(&address, window, cx);
            cx.stop_propagation();
            return;
        }
        if event.keystroke.key == "escape" {
            let target = self.target.clone().unwrap_or_default();
            self.address.reset(target.clone(), target.len());
            window.focus(&self.page_focus, cx);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let Some(edit) = query_editor::edit_for(&event.keystroke) else {
            return;
        };
        match edit {
            Edit::Local(edit) => {
                self.address.apply(edit);
            }
            Edit::Clipboard(ClipboardEdit::Copy) => query_editor::copy_selection(&self.address, cx),
            Edit::Clipboard(ClipboardEdit::Cut) => {
                query_editor::cut_selection(&mut self.address, cx);
            }
            Edit::Clipboard(ClipboardEdit::Paste) => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.address.insert(&text);
                }
            }
        }
        cx.notify();
        cx.stop_propagation();
    }

    fn history(&mut self, backwards: bool, cx: &mut Context<Self>) {
        #[cfg(not(target_os = "macos"))]
        let _ = backwards;
        #[cfg(target_os = "macos")]
        if let Some(native) = &self.native {
            if backwards {
                native.borrow().back();
            } else {
                native.borrow().forward();
            }
        }
        cx.notify();
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if self.native.is_none() {
            if let Some(target) = self.target.clone() {
                self.navigate(&target, window, cx);
            }
            return;
        }
        #[cfg(not(target_os = "macos"))]
        let _ = window;
        #[cfg(target_os = "macos")]
        let retry = self.state.error.is_some();
        self.state.error = None;
        #[cfg(target_os = "macos")]
        if let Some(native) = &self.native {
            if self.state.loading {
                native.borrow().stop();
                self.state.loading = false;
            } else {
                if retry && let Some(target) = &self.target {
                    if let Err(error) = native.borrow().navigate(target) {
                        self.state.error = Some(error);
                    }
                } else {
                    native.borrow().reload();
                }
                self.state.loading = self.state.error.is_none();
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.state.error = Some("Embedded browsing is available on macOS.".into());
        }
        cx.notify();
    }

    fn control(
        &self,
        id: &'static str,
        label: &'static str,
        icon: AnyElement,
        enabled: bool,
        colors: WorkspaceColors,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .focusable()
            .tab_stop(enabled)
            .role(Role::Button)
            .aria_label(label)
            .w(px(32.0))
            .h(px(32.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.0))
            .border_2()
            .border_color(gpui::transparent_black())
            .text_color(if enabled {
                colors.secondary
            } else {
                colors.secondary.alpha(0.45)
            })
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(move |style| style.bg(colors.hover))
            })
            .focus(move |style| style.border_color(colors.focus))
            .child(icon)
    }
}

impl Render for BrowserPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = WorkspaceColors::new(
            self.appearance
                .unwrap_or_else(|| Appearance::from_window(window.appearance())),
        );
        let focused = self.focus.is_focused(window);
        let (text, selection) = self.address.display(if focused { "│" } else { "" });
        let address = if self.address.is_empty() && !focused {
            div()
                .text_color(colors.secondary)
                .child("Address or localhost:port")
                .into_any_element()
        } else if let Some(selection) = selection {
            gpui::StyledText::new(SharedString::from(text))
                .with_highlights([(
                    selection,
                    HighlightStyle {
                        background_color: Some(colors.focus.alpha(0.18).into()),
                        ..HighlightStyle::default()
                    },
                )])
                .into_any_element()
        } else {
            SharedString::from(text).into_any_element()
        };
        let back = self
            .control(
                "browser-back",
                "Back",
                div().text_size(px(16.0)).child("←").into_any_element(),
                self.state.can_go_back,
                colors,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if this.state.can_go_back {
                    this.history(true, cx);
                }
            }));
        let forward = self
            .control(
                "browser-forward",
                "Forward",
                div().text_size(px(16.0)).child("→").into_any_element(),
                self.state.can_go_forward,
                colors,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if this.state.can_go_forward {
                    this.history(false, cx);
                }
            }));
        let reload = self
            .control(
                "browser-reload",
                if self.state.loading {
                    "Stop loading"
                } else {
                    "Reload"
                },
                Icon::new(
                    if self.state.loading {
                        IconName::Close
                    } else {
                        IconName::Refresh
                    },
                    14.0,
                    colors.secondary,
                )
                .into_any_element(),
                self.target.is_some() && !self.preview,
                colors,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                if this.target.is_some() && !this.preview {
                    this.reload(window, cx);
                }
            }));
        let external = self
            .control(
                "browser-external",
                "Open in browser",
                Icon::new(IconName::ExternalLink, 14.0, colors.secondary).into_any_element(),
                self.target.is_some() && !self.preview,
                colors,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if !this.preview
                    && let Some(url) = &this.target
                {
                    cx.open_url(url);
                }
            }));
        let mut page = div()
            .id("browser-page")
            .track_focus(&self.page_focus)
            .relative()
            .min_h(px(0.0))
            .flex_1()
            .w_full()
            .bg(colors.surface);
        #[cfg(target_os = "macos")]
        let display_native = self.visible
            && self.target.is_some()
            && self.state.error.is_none()
            && (!self.preview || self.native_preview);
        #[cfg(target_os = "macos")]
        if !display_native && let Some(native) = &self.native {
            native.borrow_mut().hide();
        }
        #[cfg(target_os = "macos")]
        if display_native && let Some(native) = &self.native {
            let native = Rc::clone(native);
            let entity = cx.entity().downgrade();
            page = page.child(
                canvas(
                    |bounds, _, _| bounds,
                    move |bounds, _, window, cx| {
                        let clipped = bounds.intersect(&window.content_mask().bounds);
                        if clipped.size.width <= px(0.0) || clipped.size.height <= px(0.0) {
                            native.borrow_mut().hide();
                        } else if let Err(error) = native.borrow_mut().mount(window, clipped) {
                            let _ = entity.update(cx, |this, cx| {
                                this.state.error = Some(error);
                                cx.notify();
                            });
                        }
                    },
                )
                .size_full(),
            );
        }
        if self.target.is_none()
            || self.state.error.is_some()
            || (self.preview && !self.native_preview)
        {
            let detail =
                self.state.error.clone().unwrap_or_else(|| {
                    "Enter a website or a local development server above.".into()
                });
            page = page.child(
                div()
                    .size_full()
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(Typo::DISPLAY_TITLE.size))
                            .text_color(colors.primary)
                            .child(if self.state.error.is_some() {
                                "Unable to open page"
                            } else {
                                "Browser"
                            }),
                    )
                    .child(
                        div()
                            .max_w(px(360.0))
                            .text_size(px(Typo::ROW.size))
                            .text_color(colors.secondary)
                            .child(detail),
                    ),
            );
        }
        div()
            .id("browser-panel")
            .debug_selector(|| "BROWSER_PANEL".into())
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(colors.surface)
            .text_color(colors.primary)
            .text_size(px(Typo::ROW.size))
            .child(
                div()
                    .h(px(36.0))
                    .flex_shrink_0()
                    .px(px(4.0))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(colors.outline)
                    .child(back)
                    .child(forward)
                    .child(reload)
                    .child(
                        div()
                            .id("browser-address")
                            .debug_selector(|| "BROWSER_ADDRESS".into())
                            .track_focus(&self.focus)
                            .tab_stop(true)
                            .role(Role::TextInput)
                            .aria_label("Website address")
                            .min_w(px(40.0))
                            .flex_1()
                            .h(px(28.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .rounded(px(8.0))
                            .bg(colors.raised)
                            .border_2()
                            .border_color(if focused {
                                colors.focus
                            } else {
                                colors.outline
                            })
                            .cursor(gpui::CursorStyle::IBeam)
                            .on_click(cx.listener(|this, _, window, cx| this.focus(window, cx)))
                            .on_key_down(cx.listener(Self::key_down))
                            .child(address),
                    )
                    .child(external),
            )
            .child(page)
    }
}

/// Normalize ordinary browser addresses without loading local files, scripts,
/// other URL schemes, or credentials supplied in the address itself.
pub(crate) fn normalize_url(input: &str) -> Result<Option<String>, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    if input.len() > 8192 || input.chars().any(char::is_control) {
        return Err("Enter a valid website address.".into());
    }
    let candidate = if input.contains("://") {
        input.to_owned()
    } else {
        let authority = input.split('/').next().unwrap_or(input);
        // A host:port is an address; other explicit schemes are unsupported.
        if !authority.starts_with('[')
            && let Some((_, tail)) = authority.split_once(':')
            && !tail.chars().all(|c| c.is_ascii_digit())
        {
            return Err("Use an http or https website address.".into());
        }
        let host = authority.split(':').next().unwrap_or(authority);
        let local = host.eq_ignore_ascii_case("localhost")
            || host == "127.0.0.1"
            || authority.starts_with("[::1]");
        format!("{}://{input}", if local { "http" } else { "https" })
    };
    let parsed = url::Url::parse(&candidate).map_err(|_| "Enter a valid website address.")?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("Use an http or https website address.".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Enter the address without a username or password.".into());
    }
    Ok(Some(parsed.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn website_addresses_and_local_servers_are_normalized() {
        for (input, expected) in [
            ("example.com", "https://example.com/"),
            (
                "https://example.com/path?q=test",
                "https://example.com/path?q=test",
            ),
            ("localhost:5173", "http://localhost:5173/"),
            ("127.0.0.1:3000/app", "http://127.0.0.1:3000/app"),
            ("[::1]:3000", "http://[::1]:3000/"),
            ("http://example.com", "http://example.com/"),
        ] {
            assert_eq!(normalize_url(input).unwrap().as_deref(), Some(expected));
        }
        assert_eq!(normalize_url("  ").unwrap(), None);
    }

    #[test]
    fn only_web_addresses_without_embedded_credentials_are_accepted() {
        for input in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,hello",
            "https://user:password@example.com",
            "https://example.com\nsecret",
            "https://",
            "bad host",
            "localhost:wrong",
        ] {
            assert!(normalize_url(input).is_err(), "accepted {input:?}");
        }
    }

    #[gpui::test]
    fn ordinary_preview_navigation_never_creates_a_native_browser(cx: &mut gpui::TestAppContext) {
        let (browser, cx) = cx.add_window_view(|_, cx| BrowserPanel::new(true, cx));
        cx.update(|window, cx| {
            browser.update(cx, |browser, cx| {
                browser.set_visible(true, window, cx);
                browser.focus(window, cx);
            })
        });
        cx.simulate_keystrokes("l o c a l h o s t : 3 0 0 0 enter");
        browser.read_with(cx, |browser, _| {
            assert_eq!(browser.target_url(), Some("http://localhost:3000/"));
            assert!(
                browser
                    .state
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("offline"))
            );
            assert!(!browser.state.loading);
            #[cfg(target_os = "macos")]
            assert!(browser.native.is_none());
        });
        cx.update(|window, cx| {
            browser.update(cx, |browser, cx| {
                browser.set_visible(false, window, cx);
                browser.navigate("", window, cx);
            })
        });
        browser.read_with(cx, |browser, _| {
            assert_eq!(browser.target_url(), None);
            assert_eq!(browser.title(), "Browser");
            assert!(browser.state.error.is_none());
        });
        cx.update(|window, cx| browser.update(cx, |browser, cx| browser.focus(window, cx)));
        cx.simulate_keystrokes("escape");
        assert!(cx.update(|window, cx| browser.read(cx).page_focus.is_focused(window)));
    }

    #[cfg(target_os = "macos")]
    #[gpui::test]
    fn late_navigation_events_cannot_reopen_a_cleared_page(cx: &mut gpui::TestAppContext) {
        let (browser, cx) = cx.add_window_view(|_, cx| BrowserPanel::new(false, cx));
        let sender = browser.read_with(cx, |browser, _| browser.sender.clone());
        cx.update(|window, cx| {
            browser.update(cx, |browser, cx| {
                // Stand in for an in-flight WebKit request without network access.
                browser.target = Some("https://example.com/".into());
                browser.state.loading = true;
                browser.navigate("", window, cx);
            })
        });
        sender
            .send(BrowserState {
                url: Some("https://example.com/".into()),
                title: Some("Old page".into()),
                ..BrowserState::default()
            })
            .unwrap();
        cx.run_until_parked();
        browser.read_with(cx, |browser, _| {
            assert_eq!(browser.target_url(), None);
            assert_eq!(browser.title(), "Browser");
            assert!(!browser.state.loading);
        });
    }
}
