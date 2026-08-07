//! Native menu bar surface using the same visual grammar as the GPUI sidebar.

use std::cell::RefCell;
use std::sync::{Arc, RwLock};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBackingStoreType, NSBezelStyle, NSBox, NSBoxType,
    NSButton, NSCellImagePosition, NSColor, NSFloatingWindowLevel, NSFocusRingType, NSFont,
    NSFontWeightMedium, NSFontWeightSemibold, NSImage, NSImageView, NSLineBreakMode, NSPanel,
    NSSquareStatusItemLength, NSStatusBar, NSStatusBarButton, NSStatusItem, NSTextAlignment,
    NSTextField, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView, NSWindowAnimationBehavior, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use diri_proto::{AgentKind, AttentionLevel, RiskHint, SessionId, SessionRecord, SessionStatus};

use crate::store::{SessionStore, StoreSnapshot};

const POPUP_WIDTH: f64 = 344.0;
const HEADER_HEIGHT: f64 = 46.0;
const FOOTER_HEIGHT: f64 = 44.0;
const ROW_HEIGHT: f64 = 28.0;
const BODY_PADDING: f64 = 6.0;
const EMPTY_BODY_HEIGHT: f64 = 92.0;
const MAX_BODY_ROWS: usize = 11;

pub struct NativeMenuBar {
    _status_item: Retained<NSStatusItem>,
    button: Retained<NSStatusBarButton>,
    panel: Retained<NSPanel>,
    surface: Retained<NSVisualEffectView>,
    header_icon: Retained<NSImageView>,
    title_label: Retained<NSTextField>,
    activity_label: Retained<NSTextField>,
    header_divider: Retained<NSBox>,
    body: Retained<NSView>,
    // NSControl target is not retained.
    _target: Retained<MenuBarTarget>,
    /// Hash of the last-rendered panel content. Rebuilding tears down and
    /// recreates every NSView in the body, so identical publishes are skipped.
    last_fingerprint: Option<u64>,
}

impl NativeMenuBar {
    #[must_use]
    pub fn new(mtm: MainThreadMarker, store: Arc<RwLock<SessionStore>>) -> Option<Self> {
        let status_item =
            NSStatusBar::systemStatusBar().statusItemWithLength(NSSquareStatusItemLength);
        let button = status_item.button(mtm)?;
        button.setToolTip(Some(&NSString::from_str("diri")));

        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            rect(0.0, 0.0, POPUP_WIDTH, 140.0),
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        panel.setFloatingPanel(true);
        panel.setLevel(NSFloatingWindowLevel);
        panel.setHasShadow(true);
        panel.setHidesOnDeactivate(true);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setAnimationBehavior(NSWindowAnimationBehavior::UtilityWindow);

        // A real sidebar material is the strongest connection to the main
        // window. The old opaque NSTextField panel looked like a debug dump.
        let surface = NSVisualEffectView::initWithFrame(
            NSVisualEffectView::alloc(mtm),
            rect(0.0, 0.0, POPUP_WIDTH, 140.0),
        );
        surface.setMaterial(NSVisualEffectMaterial::Sidebar);
        surface.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        surface.setState(NSVisualEffectState::Active);
        surface.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        surface.setWantsLayer(true);
        // NSView exposes CALayer only when objc2-quartz-core is linked. Keep
        // this tiny styling call dynamic so the AppKit surface does not pull a
        // second graphics binding into the app just for two properties.
        unsafe {
            let layer: *mut AnyObject = msg_send![&*surface, layer];
            if !layer.is_null() {
                let _: () = msg_send![layer, setCornerRadius: 12.0_f64];
                let _: () = msg_send![layer, setMasksToBounds: true];
            }
        }
        panel.setContentView(Some(&surface));

        let header_icon = symbol_view(
            "waveform.circle",
            "waveform",
            &NSColor::secondaryLabelColor(),
            rect(14.0, 0.0, 20.0, 20.0),
            mtm,
        );
        surface.addSubview(&header_icon);

        let title_label = label(
            "diri",
            13.0,
            FontStyle::Semibold,
            &NSColor::labelColor(),
            rect(42.0, 0.0, 80.0, 18.0),
            mtm,
        );
        surface.addSubview(&title_label);

        let activity_label = label(
            "No active sessions",
            11.0,
            FontStyle::Medium,
            &NSColor::secondaryLabelColor(),
            rect(148.0, 0.0, POPUP_WIDTH - 162.0, 18.0),
            mtm,
        );
        activity_label.setAlignment(NSTextAlignment::Right);
        surface.addSubview(&activity_label);

        let header_divider = separator(rect(10.0, 0.0, POPUP_WIDTH - 20.0, 1.0), mtm);
        surface.addSubview(&header_divider);

        let body = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0.0, FOOTER_HEIGHT, POPUP_WIDTH, EMPTY_BODY_HEIGHT),
        );
        surface.addSubview(&body);

        let footer_divider = separator(rect(10.0, FOOTER_HEIGHT, POPUP_WIDTH - 20.0, 1.0), mtm);
        surface.addSubview(&footer_divider);

        let target = MenuBarTarget::new(mtm, panel.clone(), button.clone(), store);
        let open_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Open diri"),
                Some(&*target as &AnyObject),
                Some(sel!(openDiri:)),
                mtm,
            )
        };
        style_footer_button(&open_button, true);
        open_button.setImage(Some(&symbol_image("arrow.up.forward.app", "macwindow")));
        open_button.setImagePosition(NSCellImagePosition::ImageLeading);
        open_button.setImageHugsTitle(true);
        open_button.setFrame(rect(8.0, 8.0, 112.0, 28.0));
        surface.addSubview(&open_button);

        let quit_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Quit"),
                Some(&*target as &AnyObject),
                Some(sel!(quitDiri:)),
                mtm,
            )
        };
        style_footer_button(&quit_button, false);
        quit_button.setFrame(rect(POPUP_WIDTH - 66.0, 8.0, 58.0, 28.0));
        surface.addSubview(&quit_button);

        unsafe {
            button.setTarget(Some(&*target as &AnyObject));
            button.setAction(Some(sel!(toggleDiriMenu:)));
        }

        let mut menu_bar = Self {
            _status_item: status_item,
            button,
            panel,
            surface,
            header_icon,
            title_label,
            activity_label,
            header_divider,
            body,
            _target: target,
            last_fingerprint: None,
        };
        menu_bar.set_attention(AttentionLevel::None);
        Some(menu_bar)
    }

    pub fn update(&mut self, snapshot: &StoreSnapshot) {
        self.set_attention(snapshot.global_attention);
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };

        // A hidden panel needs only the status-item glyph above. Opening the
        // panel requests a fresh snapshot publish (see `toggle_menu`), which
        // re-enters here with the panel visible.
        if !self.panel.isVisible() {
            self.last_fingerprint = None;
            return;
        }

        let rows = menu_rows(snapshot);
        let visible_sessions = rows
            .iter()
            .filter(|row| matches!(row, MenuBodyRow::Session(_)))
            .count();
        let total_sessions = active_session_count(snapshot);
        let remaining = total_sessions.saturating_sub(visible_sessions);
        let fingerprint = panel_fingerprint(
            &rows,
            remaining,
            total_sessions,
            snapshot.selected_session_id.as_ref(),
            snapshot.global_attention,
        );
        if self.last_fingerprint == Some(fingerprint) {
            return;
        }
        self.last_fingerprint = Some(fingerprint);
        let body_height = if rows.is_empty() {
            EMPTY_BODY_HEIGHT
        } else {
            BODY_PADDING * 2.0
                + (rows.len() as f64 + usize::from(remaining > 0) as f64) * ROW_HEIGHT
        };
        let height = HEADER_HEIGHT + body_height + FOOTER_HEIGHT;

        let old_top_left = self.panel.isVisible().then(|| {
            let frame = self.panel.frame();
            NSPoint::new(frame.origin.x, frame.origin.y + frame.size.height)
        });
        self.panel.setContentSize(NSSize::new(POPUP_WIDTH, height));
        if let Some(top_left) = old_top_left {
            self.panel.setFrameTopLeftPoint(top_left);
        }
        self.surface.setFrame(rect(0.0, 0.0, POPUP_WIDTH, height));

        let header_y = FOOTER_HEIGHT + body_height;
        self.header_icon
            .setFrame(rect(14.0, header_y + 13.0, 20.0, 20.0));
        self.title_label
            .setFrame(rect(42.0, header_y + 14.0, 80.0, 18.0));
        self.activity_label
            .setFrame(rect(148.0, header_y + 14.0, POPUP_WIDTH - 162.0, 18.0));
        self.header_divider
            .setFrame(rect(10.0, header_y, POPUP_WIDTH - 20.0, 1.0));
        self.body
            .setFrame(rect(0.0, FOOTER_HEIGHT, POPUP_WIDTH, body_height));

        self.update_activity(snapshot, total_sessions);
        self.rebuild_body(
            &rows,
            remaining,
            body_height,
            snapshot.selected_session_id.as_ref(),
            mtm,
        );
    }

    fn set_attention(&mut self, attention: AttentionLevel) {
        let symbol = match attention {
            AttentionLevel::NeedsInput => "waveform.circle.fill",
            AttentionLevel::DoneUnseen => "waveform.circle.fill",
            AttentionLevel::Working => "waveform.circle",
            AttentionLevel::IdleSeen | AttentionLevel::None | AttentionLevel::Unknown => "waveform",
        };
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&NSString::from_str("diri agent status")),
        ) {
            image.setTemplate(true);
            self.button.setImage(Some(&image));
        }

        let (symbol, fallback, color) = match attention {
            AttentionLevel::NeedsInput => (
                "waveform.circle.fill",
                "exclamationmark.circle.fill",
                NSColor::systemOrangeColor(),
            ),
            AttentionLevel::DoneUnseen => (
                "checkmark.circle.fill",
                "waveform.circle.fill",
                NSColor::systemGreenColor(),
            ),
            AttentionLevel::Working => (
                "waveform.circle",
                "waveform",
                NSColor::secondaryLabelColor(),
            ),
            AttentionLevel::IdleSeen | AttentionLevel::None | AttentionLevel::Unknown => {
                ("waveform", "circle", NSColor::tertiaryLabelColor())
            }
        };
        self.header_icon
            .setImage(Some(&symbol_image(symbol, fallback)));
        self.header_icon.setContentTintColor(Some(&color));
    }

    fn update_activity(&self, snapshot: &StoreSnapshot, total_sessions: usize) {
        let needs_input = snapshot
            .sessions
            .iter()
            .filter(|session| !session.is_archived())
            .filter(|session| session.attention() == AttentionLevel::NeedsInput)
            .count();
        let done = snapshot
            .sessions
            .iter()
            .filter(|session| !session.is_archived())
            .filter(|session| session.attention() == AttentionLevel::DoneUnseen)
            .count();
        let (text, color) = if needs_input > 0 {
            (
                format!("{needs_input} need you"),
                NSColor::systemOrangeColor(),
            )
        } else if done > 0 {
            (format!("{done} finished"), NSColor::systemGreenColor())
        } else if total_sessions == 0 {
            (
                "No active sessions".to_owned(),
                NSColor::secondaryLabelColor(),
            )
        } else {
            (
                format!(
                    "{total_sessions} session{}",
                    if total_sessions == 1 { "" } else { "s" }
                ),
                NSColor::secondaryLabelColor(),
            )
        };
        self.activity_label
            .setStringValue(&NSString::from_str(&text));
        self.activity_label.setTextColor(Some(&color));
    }

    fn rebuild_body(
        &self,
        rows: &[MenuBodyRow<'_>],
        remaining: usize,
        body_height: f64,
        selected_session_id: Option<&SessionId>,
        mtm: MainThreadMarker,
    ) {
        for child in self.body.subviews().iter() {
            child.removeFromSuperview();
        }

        if rows.is_empty() {
            self._target.set_session_ids(Vec::new());
            self.add_empty_state(mtm);
            return;
        }

        let session_ids = rows
            .iter()
            .filter_map(|row| match row {
                MenuBodyRow::Session(session) => Some(session.id.clone()),
                MenuBodyRow::Project { .. } => None,
            })
            .collect();
        self._target.set_session_ids(session_ids);

        let mut y = body_height - BODY_PADDING;
        let mut session_tag = 0;
        for row in rows {
            y -= ROW_HEIGHT;
            match row {
                MenuBodyRow::Project { name, count } => {
                    self.add_project_header(name, *count, y, mtm);
                }
                MenuBodyRow::Session(session) => {
                    self.add_session_row(
                        session,
                        session_tag,
                        selected_session_id == Some(&session.id),
                        y,
                        mtm,
                    );
                    session_tag += 1;
                }
            }
        }
        if remaining > 0 {
            y -= ROW_HEIGHT;
            self.add_more_row(remaining, y, mtm);
        }
    }

    fn add_project_header(&self, name: &str, count: usize, y: f64, mtm: MainThreadMarker) {
        let icon = symbol_view(
            "folder.fill",
            "folder",
            &NSColor::secondaryLabelColor(),
            rect(16.0, y + 7.0, 14.0, 14.0),
            mtm,
        );
        self.body.addSubview(&icon);

        let name = label(
            name,
            13.0,
            FontStyle::Medium,
            &NSColor::labelColor().colorWithAlphaComponent(0.90),
            rect(38.0, y + 5.0, POPUP_WIDTH - 82.0, 18.0),
            mtm,
        );
        self.body.addSubview(&name);

        let count = label(
            &count.to_string(),
            11.0,
            FontStyle::Medium,
            &NSColor::tertiaryLabelColor(),
            rect(POPUP_WIDTH - 38.0, y + 5.0, 22.0, 18.0),
            mtm,
        );
        count.setAlignment(NSTextAlignment::Right);
        self.body.addSubview(&count);
    }

    fn add_session_row(
        &self,
        session: &SessionRecord,
        tag: isize,
        selected: bool,
        y: f64,
        mtm: MainThreadMarker,
    ) {
        let row = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::new(),
                Some(&*self._target as &AnyObject),
                Some(sel!(selectSession:)),
                mtm,
            )
        };
        row.setTag(tag);
        row.setFrame(rect(8.0, y + 1.0, POPUP_WIDTH - 16.0, ROW_HEIGHT - 2.0));
        row.setBordered(true);
        row.setBezelStyle(NSBezelStyle::AccessoryBar);
        row.setShowsBorderOnlyWhileMouseInside(!selected);
        row.setBezelColor(Some(
            &NSColor::labelColor().colorWithAlphaComponent(if selected { 0.10 } else { 0.08 }),
        ));
        row.setRefusesFirstResponder(true);
        row.setFocusRingType(NSFocusRingType::None);
        let tooltip = session.needs_input.as_ref().map_or_else(
            || display_title(session).to_owned(),
            |detail| detail.summary.clone(),
        );
        row.setToolTip(Some(&NSString::from_str(&tooltip)));
        unsafe {
            let accessibility_label = NSString::from_str(display_title(session));
            let _: () = msg_send![&*row, setAccessibilityLabel: &*accessibility_label];
        }

        let icon_color = session_color(session);
        let icon = symbol_view(
            agent_symbol(session.effective_kind()),
            "circle.fill",
            &icon_color,
            rect(18.0, 6.0, 14.0, 14.0),
            mtm,
        );
        row.addSubview(&icon);

        let status = trailing_status(session);
        let status_width = status.as_ref().map_or(0.0, |status| status.width + 8.0);
        let title_color = if session.hibernation.is_some() {
            NSColor::secondaryLabelColor()
        } else {
            NSColor::labelColor().colorWithAlphaComponent(0.82)
        };
        let title = label(
            display_title(session),
            13.0,
            FontStyle::Regular,
            &title_color,
            rect(
                42.0,
                4.0,
                POPUP_WIDTH - 16.0 - 42.0 - 10.0 - status_width,
                18.0,
            ),
            mtm,
        );
        row.addSubview(&title);

        if let Some(status) = status {
            let status_label = label(
                status.text,
                11.0,
                FontStyle::Medium,
                &status.color,
                rect(POPUP_WIDTH - 26.0 - status.width, 4.0, status.width, 18.0),
                mtm,
            );
            status_label.setAlignment(NSTextAlignment::Right);
            row.addSubview(&status_label);
        }
        self.body.addSubview(&row);
    }

    fn add_more_row(&self, remaining: usize, y: f64, mtm: MainThreadMarker) {
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(&format!(
                    "{remaining} more session{} in diri",
                    if remaining == 1 { "" } else { "s" }
                )),
                Some(&*self._target as &AnyObject),
                Some(sel!(openDiri:)),
                mtm,
            )
        };
        button.setFrame(rect(20.0, y + 1.0, POPUP_WIDTH - 40.0, ROW_HEIGHT - 2.0));
        button.setBordered(true);
        button.setBezelStyle(NSBezelStyle::AccessoryBar);
        button.setShowsBorderOnlyWhileMouseInside(true);
        button.setFont(Some(&system_font(11.0, FontStyle::Medium)));
        button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        button.setImage(Some(&symbol_image("ellipsis", "circle.grid.3x3.fill")));
        button.setImagePosition(NSCellImagePosition::ImageLeading);
        button.setImageHugsTitle(true);
        button.setRefusesFirstResponder(true);
        button.setFocusRingType(NSFocusRingType::None);
        self.body.addSubview(&button);
    }

    fn add_empty_state(&self, mtm: MainThreadMarker) {
        let icon = symbol_view(
            "waveform",
            "sparkles",
            &NSColor::tertiaryLabelColor(),
            rect((POPUP_WIDTH - 22.0) / 2.0, 52.0, 22.0, 22.0),
            mtm,
        );
        self.body.addSubview(&icon);

        let title = label(
            "No active sessions",
            13.0,
            FontStyle::Medium,
            &NSColor::secondaryLabelColor(),
            rect(32.0, 29.0, POPUP_WIDTH - 64.0, 18.0),
            mtm,
        );
        title.setAlignment(NSTextAlignment::Center);
        self.body.addSubview(&title);

        let hint = label(
            "Open diri to start an agent",
            11.0,
            FontStyle::Regular,
            &NSColor::tertiaryLabelColor(),
            rect(32.0, 11.0, POPUP_WIDTH - 64.0, 16.0),
            mtm,
        );
        hint.setAlignment(NSTextAlignment::Center);
        self.body.addSubview(&hint);
    }
}

#[derive(Clone, Copy)]
enum MenuBodyRow<'a> {
    Project { name: &'a str, count: usize },
    Session(&'a SessionRecord),
}

/// Hashes everything the panel body and header display, so an identical
/// publish never pays NSView teardown.
fn panel_fingerprint(
    rows: &[MenuBodyRow<'_>],
    remaining: usize,
    total_sessions: usize,
    selected: Option<&SessionId>,
    global_attention: AttentionLevel,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for row in rows {
        match row {
            MenuBodyRow::Project { name, count } => {
                0u8.hash(&mut hasher);
                name.hash(&mut hasher);
                count.hash(&mut hasher);
            }
            MenuBodyRow::Session(session) => {
                1u8.hash(&mut hasher);
                session.id.0.hash(&mut hasher);
                display_title(session).hash(&mut hasher);
                std::mem::discriminant(&session.attention()).hash(&mut hasher);
                session.effective_kind().id().hash(&mut hasher);
                session.hibernation.is_some().hash(&mut hasher);
                if let Some(detail) = &session.needs_input {
                    detail.summary.hash(&mut hasher);
                    std::mem::discriminant(&detail.risk_hint).hash(&mut hasher);
                }
            }
        }
    }
    remaining.hash(&mut hasher);
    total_sessions.hash(&mut hasher);
    selected.map(|id| id.0.as_str()).hash(&mut hasher);
    std::mem::discriminant(&global_attention).hash(&mut hasher);
    hasher.finish()
}

fn menu_rows(snapshot: &StoreSnapshot) -> Vec<MenuBodyRow<'_>> {
    let mut rows = Vec::new();
    for project in &snapshot.projects {
        let sessions: Vec<_> = snapshot
            .sessions
            .iter()
            .filter(|session| session.project_id == project.id && !session.is_archived())
            .collect();
        if sessions.is_empty() {
            continue;
        }
        rows.push(MenuBodyRow::Project {
            name: &project.name,
            count: sessions.len(),
        });
        for session in sessions {
            rows.push(MenuBodyRow::Session(session.as_ref()));
        }
    }

    if rows.len() > MAX_BODY_ROWS {
        rows.truncate(MAX_BODY_ROWS - 1);
        // Never leave a folder heading stranded at the bottom of the menu.
        if matches!(rows.last(), Some(MenuBodyRow::Project { .. })) {
            rows.pop();
        }
    }
    rows
}

fn active_session_count(snapshot: &StoreSnapshot) -> usize {
    snapshot
        .projects
        .iter()
        .map(|project| {
            snapshot
                .sessions
                .iter()
                .filter(|session| session.project_id == project.id && !session.is_archived())
                .count()
        })
        .sum()
}

struct TrailingStatus {
    text: &'static str,
    width: f64,
    color: Retained<NSColor>,
}

fn trailing_status(session: &SessionRecord) -> Option<TrailingStatus> {
    if session.hibernation.is_some() {
        return Some(TrailingStatus {
            text: "asleep",
            width: 48.0,
            color: NSColor::tertiaryLabelColor(),
        });
    }
    match session.attention() {
        AttentionLevel::NeedsInput => Some(TrailingStatus {
            text: "needs you",
            width: 58.0,
            color: if session
                .needs_input
                .as_ref()
                .is_some_and(|detail| detail.risk_hint == RiskHint::Destructive)
            {
                NSColor::systemRedColor()
            } else {
                NSColor::systemOrangeColor()
            },
        }),
        AttentionLevel::DoneUnseen => Some(TrailingStatus {
            text: "done",
            width: 40.0,
            color: NSColor::systemGreenColor(),
        }),
        _ => None,
    }
}

fn agent_symbol(kind: &AgentKind) -> &'static str {
    match kind.id() {
        AgentKind::CLAUDE_CODE_ID => "sparkles",
        AgentKind::CODEX_ID => "circle.hexagongrid.fill",
        AgentKind::CURSOR_ID => "cursorarrow.rays",
        AgentKind::GEMINI_ID => "diamond.fill",
        AgentKind::SHELL_ID | AgentKind::GENERIC_ID => "terminal.fill",
        _ => "circle.fill",
    }
}

fn session_color(session: &SessionRecord) -> Retained<NSColor> {
    if session.hibernation.is_some() {
        return NSColor::tertiaryLabelColor();
    }
    match session.attention() {
        AttentionLevel::NeedsInput => {
            if session
                .needs_input
                .as_ref()
                .is_some_and(|detail| detail.risk_hint == RiskHint::Destructive)
            {
                NSColor::systemRedColor()
            } else {
                NSColor::systemOrangeColor()
            }
        }
        AttentionLevel::DoneUnseen => NSColor::systemGreenColor(),
        AttentionLevel::Working => match session.effective_kind().id() {
            AgentKind::CLAUDE_CODE_ID => {
                NSColor::colorWithSRGBRed_green_blue_alpha(0.851, 0.467, 0.341, 1.0)
            }
            AgentKind::GEMINI_ID => {
                NSColor::colorWithSRGBRed_green_blue_alpha(0.306, 0.510, 0.933, 1.0)
            }
            _ => NSColor::labelColor().colorWithAlphaComponent(0.82),
        },
        AttentionLevel::IdleSeen | AttentionLevel::None | AttentionLevel::Unknown => {
            NSColor::tertiaryLabelColor()
        }
    }
}

fn display_title(session: &SessionRecord) -> &str {
    if session.title.is_empty() {
        match session.status {
            SessionStatus::Exited(_) => "Ended",
            _ => "Untitled",
        }
    } else {
        &session.title
    }
}

#[derive(Clone, Copy)]
enum FontStyle {
    Regular,
    Medium,
    Semibold,
}

fn system_font(size: f64, style: FontStyle) -> Retained<NSFont> {
    match style {
        FontStyle::Regular => NSFont::systemFontOfSize(size),
        // SAFETY: AppKit exports these process-lifetime CGFloat constants.
        FontStyle::Medium => NSFont::systemFontOfSize_weight(size, unsafe { NSFontWeightMedium }),
        FontStyle::Semibold => {
            NSFont::systemFontOfSize_weight(size, unsafe { NSFontWeightSemibold })
        }
    }
}

fn label(
    text: &str,
    size: f64,
    style: FontStyle,
    color: &NSColor,
    frame: NSRect,
    mtm: MainThreadMarker,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setFrame(frame);
    label.setFont(Some(&system_font(size, style)));
    label.setTextColor(Some(color));
    label.setMaximumNumberOfLines(1);
    label.setUsesSingleLineMode(true);
    label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    label.setAllowsDefaultTighteningForTruncation(true);
    label
}

fn symbol_image(name: &str, fallback: &str) -> Retained<NSImage> {
    let description = NSString::from_str(name);
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&description),
    )
    .or_else(|| {
        NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(fallback),
            Some(&description),
        )
    })
    .expect("macOS 15 always provides the fallback menu symbols");
    image.setTemplate(true);
    image
}

fn symbol_view(
    name: &str,
    fallback: &str,
    color: &NSColor,
    frame: NSRect,
    mtm: MainThreadMarker,
) -> Retained<NSImageView> {
    let image = symbol_image(name, fallback);
    let view = NSImageView::imageViewWithImage(&image, mtm);
    view.setFrame(frame);
    view.setContentTintColor(Some(color));
    view
}

fn separator(frame: NSRect, mtm: MainThreadMarker) -> Retained<NSBox> {
    let separator = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
    separator.setBoxType(NSBoxType::Separator);
    separator
}

fn style_footer_button(button: &NSButton, prominent: bool) {
    button.setFont(Some(&system_font(
        13.0,
        if prominent {
            FontStyle::Medium
        } else {
            FontStyle::Regular
        },
    )));
    button.setBordered(true);
    button.setBezelStyle(if prominent {
        NSBezelStyle::AccessoryBarAction
    } else {
        NSBezelStyle::AccessoryBar
    });
    button.setShowsBorderOnlyWhileMouseInside(!prominent);
    let tint = if prominent {
        NSColor::labelColor()
    } else {
        NSColor::secondaryLabelColor()
    };
    button.setContentTintColor(Some(&tint));
    button.setRefusesFirstResponder(true);
    button.setFocusRingType(NSFocusRingType::None);
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

struct MenuBarTargetIvars {
    panel: Retained<NSPanel>,
    button: Retained<NSStatusBarButton>,
    store: Arc<RwLock<SessionStore>>,
    session_ids: RefCell<Vec<SessionId>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. AppKit invokes these
    // control actions on the main thread and the class does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = MenuBarTargetIvars]
    struct MenuBarTarget;

    unsafe impl NSObjectProtocol for MenuBarTarget {}

    impl MenuBarTarget {
        #[unsafe(method(toggleDiriMenu:))]
        fn toggle_menu(&self, _sender: Option<&AnyObject>) {
            let panel = &self.ivars().panel;
            if panel.isVisible() {
                panel.orderOut(None);
                return;
            }

            // Updates are skipped while the panel is hidden; ask for one
            // fresh snapshot so the first visible frame shows current rows.
            self.ivars()
                .store
                .write()
                .expect("session store lock poisoned")
                .request_snapshot_publish();

            if let Some(window) = self.ivars().button.window() {
                let button_rect = self
                    .ivars()
                    .button
                    .convertRect_toView(self.ivars().button.bounds(), None);
                let screen_rect = window.convertRectToScreen(button_rect);
                panel.setFrameTopLeftPoint(NSPoint::new(
                    screen_rect.origin.x + (screen_rect.size.width - POPUP_WIDTH) / 2.0,
                    screen_rect.origin.y - 4.0,
                ));
            }
            panel.orderFront(None);
        }

        #[unsafe(method(openDiri:))]
        fn open_diri(&self, _sender: Option<&AnyObject>) {
            self.show_main_window();
        }

        #[unsafe(method(selectSession:))]
        fn select_session(&self, sender: Option<&AnyObject>) {
            if let Some(tag) = sender
                .and_then(|sender| sender.downcast_ref::<NSButton>())
                .map(|button| button.tag())
                && let Some(id) = self.ivars().session_ids.borrow().get(tag as usize).cloned()
            {
                self.ivars()
                    .store
                    .write()
                    .expect("session store lock poisoned")
                    .select(id);
            }
            self.show_main_window();
        }

        #[unsafe(method(quitDiri:))]
        fn quit_diri(&self, _sender: Option<&AnyObject>) {
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }
    }
);

impl MenuBarTarget {
    fn new(
        mtm: MainThreadMarker,
        panel: Retained<NSPanel>,
        button: Retained<NSStatusBarButton>,
        store: Arc<RwLock<SessionStore>>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MenuBarTargetIvars {
            panel,
            button,
            store,
            session_ids: RefCell::new(Vec::new()),
        });
        // SAFETY: NSObject's init is its designated initializer.
        unsafe { msg_send![super(this), init] }
    }

    fn set_session_ids(&self, ids: Vec<SessionId>) {
        self.ivars().session_ids.replace(ids);
    }

    fn show_main_window(&self) {
        self.ivars().panel.orderOut(None);
        let app = NSApplication::sharedApplication(self.mtm());
        app.activate();
        for window in app.windows().iter() {
            if window.canBecomeMainWindow() {
                window.makeKeyAndOrderFront(None);
                break;
            }
        }
    }
}
