//! Terminal pane composition.
//!
//! The daemon remains authoritative: this module only composes
//! `zeus-client::SessionAttachment`, `zeus-term`, and the T9 session store.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, ClickEvent, ClipboardEntry, ClipboardItem, Context, DragMoveEvent, Entity,
    EventEmitter, ExternalPaths, FocusHandle, KeyBinding, KeyDownEvent, KeyUpEvent,
    ModifiersChangedEvent, MouseButton, PathBuilder, Render, Rgba, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement, Subscription, Task, Window,
    actions, canvas, div, font, point, prelude::*, px, rgba,
};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use zeus_client::attachment::{SessionAttachment, TerminalChunk};
use zeus_proto::frames::TerminalModes as WireTerminalModes;
use zeus_proto::grid::{ChangedRow, GridUpdate};
use zeus_proto::{
    AgentKind as ProtoAgentKind, ArtifactKind, ExitReason, PrCheck, PullRequestStatus,
    Resumability, RiskHint, SessionArtifact, SessionId, SessionRecord, SessionStatus, TitleSource,
};
use zeus_term::buffer::GridBuffer;
use zeus_term::element::{SharedGridBuffer, TerminalElement, TerminalReference};
use zeus_term::find::{FindSnapshot, SearchRequest, TerminalFindModel};
use zeus_term::keys::{
    Key as TermKey, KeyEvent as TermKeyEvent, Modifiers as TermModifiers, NamedKey, TermInputModes,
    alternate_scroll, encode_key, paste,
};
use zeus_term::metrics::CellMetrics;
use zeus_term::mouse::{
    MouseButton as TermMouseButton, MouseModes, MouseModifiers, motion_report, press_report,
    release_report, wheel_reports,
};
use zeus_term::repaint::{RepaintAction, RepaintPacer};
use zeus_term::scrollback::{WheelDelta, WheelEvent, WheelRoute};
use zeus_term::theme::TermTheme;
use zeus_ui::{
    AgentKind as UiAgentKind, AgentLogo, Fill, FloatingSurface, Ink, Metrics, Radius,
    SemanticColors, StatusGlyph, StatusState, Typo, WorkingOrbit,
};

use crate::image_attachment::{
    AttachmentDecision, ImageStore, StagedImage, capability_from_descriptor, decide_drop,
    keep_staged, paste_paths, stage_bytes, stage_drop, unsupported_message,
};
use crate::macos::sf_symbols::{SymbolWeight, sf_symbol, sf_symbol_weighted};
use crate::navigation::{NavigationOverlay, ToggleCommandPalette, ToggleQuickOpen, query_label};
use crate::preview_terminal::{preview_session_grid, preview_session_grid_sized};
use crate::query_editor::{self, ClipboardEdit, Edit, QueryEditor};
use crate::session_surfaces::switcher_key;
use crate::store::{LineageNode, LineageStrip, LineageView, StoreRuntime};
use crate::surface_shell::UtilitySurfaces;
use crate::switcher::display_title;

const GRID_HORIZONTAL_PADDING: f32 = 24.0;
const GRID_VERTICAL_PADDING: f32 = 12.0;
// The outer terminal card has a one-pixel border on both sides and the pane
// adds its own left divider. These pixels are outside TerminalElement's actual
// paint bounds and therefore cannot be offered to the PTY as a text column.
const GRID_LAYOUT_HORIZONTAL_CHROME: f32 = 3.0;
const GRID_LAYOUT_VERTICAL_CHROME: f32 = 2.0;
const TOOLBAR_MAX_VISIBLE_LINKS: usize = 4;
const TOOLBAR_LINK_MAX_WIDTH: f32 = 176.0;
const TOOLBAR_OVERFLOW_WIDTH: f32 = 50.0;
const REATTACH_DELAY: Duration = Duration::from_millis(500);
/// Burst ceiling for repaints (~60fps). The pacer paints the first frame of a
/// burst and the next response after interactive input immediately; this only
/// caps sustained output, and background panes never invalidate the window, so
/// idle budgets are unaffected. Matched to the daemon's `gridFlushInterval`.
const ACTIVE_REPAINT_INTERVAL: Duration = Duration::from_millis(16);
/// How often a live drag is allowed to push a new PTY geometry. Matched to the
/// daemon's coalesced grid flush (also 16ms): resizing faster produces frames
/// the client can never see, resizing slower makes the drag look like it snaps
/// at the end instead of reflowing under the cursor.
const RESIZE_CADENCE: Duration = Duration::from_millis(16);
/// Two resizes further apart than this belong to different gestures. A drag
/// steps faster than this and must keep reflowing live; anything slower is a
/// discrete change -- a panel toggle, a window snap, a font-size change --
/// whose reflow is held still by [`REFLOW_HOLD`]. Matched to the window the
/// daemon uses to infer the same thing (`AgentSession.resizeDragWindow`).
const RESIZE_GESTURE_GAP: Duration = Duration::from_millis(200);
/// Ceiling on how long the grid is held still across a column change.
///
/// A cols-only resize comes back in two stages: the daemon re-wraps its
/// emulator and broadcasts that immediately, then the program answers SIGWINCH
/// and repaints. Painting the first stage is what made a sidebar toggle shove
/// the content up and drop it back a frame later -- re-wrapping at a fixed row
/// count spills the top into scrollback, and the grid is painted top-anchored
/// on row index, so every surviving line moves up until the program's repaint
/// puts it back. Holding both stages and applying them as one paint removes
/// the intermediate frame entirely. The hold ends as soon as the program's
/// repaint lands, so this bound only applies to one that is slow or absent.
const REFLOW_HOLD: Duration = Duration::from_millis(140);
/// Slack added to a bottom-anchored grid's height so layout rounding can never
/// shave its last row off. See `TerminalPane::grid_row_overflow`.
const ANCHOR_SLACK: f32 = 1.0;
/// How many evicted sessions keep their last-known grid parked for instant
/// re-selection. Cells only (~100KB each) — elements, channels, and shape
/// caches are rebuilt on promotion — so the ceiling is a memory bound, not a
/// residency one.
const PARKED_GRID_CAP: usize = 12;
const LINEAGE_TAB_HEIGHT: f32 = 26.0;
const LINEAGE_TREE_NODE_WIDTH: f32 = 124.0;
const LINEAGE_TREE_MARK: f32 = 40.0;
const LINEAGE_TREE_ICON: f32 = 26.0;
const LINEAGE_TREE_CAPTION_GAP: f32 = 6.0;
const LINEAGE_TREE_CAPTION_HEIGHT: f32 = 40.0;
const LINEAGE_TREE_NODE_HEIGHT: f32 =
    LINEAGE_TREE_MARK + LINEAGE_TREE_CAPTION_GAP + LINEAGE_TREE_CAPTION_HEIGHT;
const LINEAGE_TREE_H_GAP: f32 = 28.0;
const LINEAGE_TREE_V_GAP: f32 = 36.0;
const LINEAGE_TREE_PAD: f32 = 28.0;
const LINEAGE_TREE_ELBOW: f32 = 10.0;
actions!(
    zeus_terminal,
    [
        OpenFind,
        FindNext,
        FindPrevious,
        CloseFind,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        Paste,
        CopySelection,
    ]
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalPaneEvent {
    ToggleSidebar,
    ToggleInspector,
    OpenFileReference {
        reference: String,
        cwd: String,
        session_id: SessionId,
    },
}

pub fn bind_terminal_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-f", OpenFind, None),
        KeyBinding::new("cmd-g", FindNext, None),
        KeyBinding::new("cmd-shift-g", FindPrevious, None),
        KeyBinding::new("cmd-=", ZoomIn, None),
        KeyBinding::new("cmd-+", ZoomIn, None),
        KeyBinding::new("cmd--", ZoomOut, None),
        KeyBinding::new("cmd-0", ResetZoom, None),
        KeyBinding::new("cmd-v", Paste, None),
        KeyBinding::new("cmd-c", CopySelection, None),
    ]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipTint {
    Red,
    Orange,
    Yellow,
    Green,
    Purple,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaneChip {
    pub id: String,
    pub label: String,
    pub system_image: &'static str,
    pub open_url: Option<String>,
    pub copy_string: String,
    pub tint: Option<ChipTint>,
    pub help: String,
    pub checks: Option<PullRequestStatus>,
}

impl PaneChip {
    pub fn for_session(session: &SessionRecord) -> Vec<Self> {
        let mut result = Vec::new();
        let artifacts = session.artifacts.as_deref().unwrap_or_default();
        let statuses = session.pull_requests.as_deref().unwrap_or_default();
        let pull_requests = artifacts
            .iter()
            .filter(|artifact| artifact.kind == ArtifactKind::PullRequest)
            .map(|artifact| {
                (
                    artifact,
                    statuses.iter().find(|status| status.url == artifact.url),
                )
            })
            .collect::<Vec<_>>();

        // Primary PR destinations are the highest-value links, so expose all
        // of them before their supporting checks/comments or generic URLs.
        for (artifact, status) in &pull_requests {
            result.push(Self::from_artifact(artifact, *status));
        }
        for (artifact, status) in pull_requests {
            if let Some(status) = status {
                if let Some(checks) = Self::checks_chip(artifact, status) {
                    result.push(checks);
                }
                if let Some(comments) = Self::comments_chip(artifact, status) {
                    result.push(comments);
                }
            }
        }
        for artifact in artifacts
            .iter()
            .filter(|artifact| artifact.kind != ArtifactKind::PullRequest)
        {
            result.push(Self::from_artifact(artifact, None));
        }
        for port in session.listening_ports.as_deref().unwrap_or_default() {
            let url = format!("http://localhost:{}", port.port);
            result.push(Self {
                id: format!("port-{}", port.port),
                label: format!(":{}", port.port),
                system_image: "network",
                open_url: Some(url.clone()),
                copy_string: url.clone(),
                tint: None,
                help: url,
                checks: None,
            });
        }
        result
    }

    fn from_artifact(artifact: &SessionArtifact, pr: Option<&PullRequestStatus>) -> Self {
        match artifact.kind {
            ArtifactKind::PullRequest => {
                let mut label = pr_number(&artifact.url)
                    .map_or_else(|| "PR".to_owned(), |number| format!("PR #{number}"));
                if let Some(pr) = pr
                    && pr.additions + pr.deletions > 0
                {
                    label.push_str(&format!(" +{} −{}", pr.additions, pr.deletions));
                }
                Self {
                    id: format!("art-{}", artifact.url),
                    label,
                    system_image: pr.map_or("arrow.triangle.pull", |pr| match pr.state.as_str() {
                        "MERGED" => "arrow.triangle.merge",
                        "CLOSED" => "xmark.circle",
                        _ => "arrow.triangle.pull",
                    }),
                    open_url: Some(artifact.url.clone()),
                    copy_string: artifact.url.clone(),
                    tint: pr.and_then(pr_tint),
                    help: pr.map_or_else(|| artifact.url.clone(), pr_help),
                    checks: None,
                }
            }
            ArtifactKind::LinearIssue => Self::quiet_artifact(
                artifact,
                linear_key(&artifact.url).unwrap_or_else(|| "Linear".to_owned()),
                "checklist",
            ),
            ArtifactKind::Preview => Self::quiet_artifact(
                artifact,
                url_port(&artifact.url)
                    .map_or_else(|| url_host(&artifact.url), |port| format!(":{port}")),
                "network",
            ),
            ArtifactKind::Link | ArtifactKind::Unknown => {
                Self::quiet_artifact(artifact, url_host(&artifact.url), "link")
            }
        }
    }

    fn quiet_artifact(
        artifact: &SessionArtifact,
        label: String,
        system_image: &'static str,
    ) -> Self {
        Self {
            id: format!("art-{}", artifact.url),
            label,
            system_image,
            open_url: Some(artifact.url.clone()),
            copy_string: artifact.url.clone(),
            tint: None,
            help: artifact.url.clone(),
            checks: None,
        }
    }

    fn checks_chip(artifact: &SessionArtifact, pr: &PullRequestStatus) -> Option<Self> {
        let total = pr.checks_passed + pr.checks_failed + pr.checks_pending;
        if total <= 0 {
            return None;
        }
        let (system_image, tint) = if pr.checks_failed > 0 {
            ("xmark.circle.fill", ChipTint::Red)
        } else if pr.checks_pending > 0 {
            ("clock.fill", ChipTint::Yellow)
        } else {
            ("checkmark.circle.fill", ChipTint::Green)
        };
        let mut states = vec![format!("{} passed", pr.checks_passed)];
        if pr.checks_failed > 0 {
            states.push(format!("{} failed", pr.checks_failed));
        }
        if pr.checks_pending > 0 {
            states.push(format!("{} running", pr.checks_pending));
        }
        Some(Self {
            id: format!("art-{}-checks", artifact.url),
            label: format!("{}/{total}", pr.checks_passed),
            system_image,
            open_url: Some(format!("{}/checks", artifact.url.trim_end_matches('/'))),
            copy_string: artifact.url.clone(),
            tint: Some(tint),
            help: format!("Checks: {}", states.join(" · ")),
            checks: Some(pr.clone()),
        })
    }

    fn comments_chip(artifact: &SessionArtifact, pr: &PullRequestStatus) -> Option<Self> {
        let count = pr.comment_count + pr.review_count;
        let (label, tint) = if let Some(total) = pr.total_threads.filter(|total| *total > 0) {
            let resolved = pr.resolved_threads.unwrap_or(0);
            (
                format!("{resolved}/{total}"),
                Some(if resolved == total {
                    ChipTint::Green
                } else {
                    ChipTint::Orange
                }),
            )
        } else if count > 0 {
            (count.to_string(), None)
        } else {
            return None;
        };
        Some(Self {
            id: format!("art-{}-comments", artifact.url),
            label,
            system_image: "bubble.left",
            open_url: Some(artifact.url.clone()),
            copy_string: artifact.url.clone(),
            tint,
            help: comments_help(pr),
            checks: None,
        })
    }
}

fn toolbar_chip_width(chip: &PaneChip) -> f32 {
    let label_width = chip.label.chars().count().min(24) as f32 * 6.2;
    (label_width + 34.0).clamp(68.0, TOOLBAR_LINK_MAX_WIDTH)
}

/// `plain_toolbar` means neither the traffic-light lane nor the sidebar reveal
/// button is in the row; which side they sit on does not change their cost.
fn toolbar_visible_chip_count(
    chips: &[PaneChip],
    viewport_width: f32,
    plain_toolbar: bool,
) -> usize {
    if chips.is_empty() {
        return 0;
    }

    // Protect a readable session title, branch/host metadata, agent identity,
    // and (when needed) the macOS traffic-light lane + sidebar reveal button.
    let fixed_chrome = if plain_toolbar { 560.0 } else { 673.0 };
    let budget = (viewport_width - fixed_chrome).clamp(TOOLBAR_OVERFLOW_WIDTH, 720.0);
    let limit = chips.len().min(TOOLBAR_MAX_VISIBLE_LINKS);
    let mut used = 0.0;
    let mut visible = 0;

    for (index, chip) in chips.iter().take(limit).enumerate() {
        let gap = if index == 0 {
            0.0
        } else {
            Metrics::TOOLBAR_COMPACT_GAP
        };
        let candidate = used + gap + toolbar_chip_width(chip);
        let overflow = if index + 1 < chips.len() {
            Metrics::TOOLBAR_COMPACT_GAP + TOOLBAR_OVERFLOW_WIDTH
        } else {
            0.0
        };
        if candidate + overflow > budget {
            break;
        }
        used = candidate;
        visible += 1;
    }

    visible
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttachmentState {
    Attaching,
    Live,
    Reconnecting,
}

enum AttachmentCommand {
    Input(Vec<u8>),
    Resize(u16, u16),
    Close,
}

#[derive(Clone)]
struct AttachmentControl {
    tx: mpsc::UnboundedSender<AttachmentCommand>,
    pane_tx: mpsc::UnboundedSender<PaneEvent>,
}

impl AttachmentControl {
    fn inert() -> Self {
        let (tx, _) = mpsc::unbounded_channel();
        let (pane_tx, _) = mpsc::unbounded_channel();
        Self { tx, pane_tx }
    }

    fn input(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        // Queue the priority marker before the bytes leave for the daemon, so
        // an echo that returns immediately cannot land behind the UI's
        // background-output repaint timer.
        let _ = self.pane_tx.send(PaneEvent::InteractiveInput);
        let _ = self.tx.send(AttachmentCommand::Input(bytes));
    }

    fn resize(&self, cols: u16, rows: u16) {
        let _ = self.tx.send(AttachmentCommand::Resize(cols, rows));
    }

    fn close(&self) {
        let _ = self.tx.send(AttachmentCommand::Close);
    }
}

enum PaneEvent {
    InteractiveInput,
    AttachmentState(SessionId, AttachmentState),
    Chunk(SessionId, TerminalChunk),
    FindSnapshot(SessionId, SearchRequest, FindSnapshot),
    ScrollbackCells(SessionId, zeus_proto::ReadScrollbackCellsResult, usize),
    ScrollbackFailed(SessionId),
    AttachmentUploadFinished(SessionId, Result<Vec<String>, String>, Vec<String>),
}

#[derive(Clone, Debug)]
enum AttachmentUi {
    Hover { names: Vec<String>, message: String },
    Progress { message: String },
    Success { message: String },
    Failed { message: String },
    Unsupported { message: String },
}

struct PendingUpload {
    session_id: SessionId,
    local_paths: Vec<String>,
    display_names: Vec<String>,
}

struct ResidentTerminal {
    element: TerminalElement,
    attachment: AttachmentControl,
    attachment_state: AttachmentState,
    input_modes: TermInputModes,
    mouse_modes: MouseModes,
    /// Suppresses a release/motion sequence when a platform-click was consumed
    /// locally to open a reference instead of being pressed in the TUI.
    suppress_left_report: bool,
    reported_button_down: Option<TermMouseButton>,
    /// Mouse-tracking applications care about cell transitions, not every
    /// subpixel pointer event GPUI produces inside the same cell.
    last_reported_mouse: Option<(usize, usize, Option<TermMouseButton>)>,
    find: Option<TerminalFindModel>,
    /// The editable text behind `find`'s query, so ⌘F gets the same caret,
    /// selection, and readline keys as the other query fields.
    find_query: QueryEditor,
    last_size: (u16, u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SessionSource {
    FollowSelection,
    Fixed(SessionId),
}

/// Grid frames parked while a column change round-trips through the daemon,
/// so the re-wrap and the program's repaint reach the screen as one paint
/// rather than as a jump and a correction. See [`REFLOW_HOLD`].
struct ReflowHold {
    parked: Vec<GridUpdate>,
    /// The daemon's re-wrapped snapshot has landed, so the next frame after it
    /// is the program answering SIGWINCH and completes the pair.
    saw_snapshot: bool,
    /// The ceiling timer. Dropped with the hold, which cancels it.
    _release: Task<()>,
}

impl ReflowHold {
    /// Parks a frame, reporting whether the pair is now complete and the hold
    /// should be released.
    fn park(&mut self, update: GridUpdate) -> bool {
        let snapshot = update.is_full_snapshot;
        self.parked.push(update);
        if snapshot {
            // A later snapshot supersedes the first (a re-seed after
            // backpressure, or the daemon's own settle pass) rather than
            // standing in for the repaint we are waiting on.
            self.saw_snapshot = true;
            return false;
        }
        self.saw_snapshot
    }
}

/// What the surrounding workbench looks like this frame, so the pane's toolbar
/// can offer the right controls. `traffic_light_lane` is set when the card is
/// flush against the window's leading edge and therefore has to keep the macOS
/// window buttons clear; `mirrored` puts the sidebar on the trailing edge and
/// the inspector on the leading one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShellChrome {
    pub sidebar_visible: bool,
    pub inspector_open: bool,
    pub traffic_light_lane: bool,
    pub mirrored: bool,
}

/// Window-space allocation supplied by the workbench. Terminal input needs
/// the origin while PTY sizing needs the local width and height.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TerminalViewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Drop for ResidentTerminal {
    fn drop(&mut self) {
        self.attachment.close();
    }
}

pub struct TerminalPane {
    runtime: Arc<StoreRuntime>,
    _tokio_owner: Arc<tokio::runtime::Runtime>,
    tokio: Handle,
    residents: HashMap<SessionId, ResidentTerminal>,
    /// Last-known grids of recently evicted sessions, most recent last.
    /// Selecting a session paints its parked grid on the very first frame
    /// while the fresh attachment round-trips; the attach's full snapshot
    /// then overwrites the same buffer in place. This is what makes session
    /// switching read as instant with a residency of one.
    parked_grids: Vec<(SessionId, SharedGridBuffer)>,
    pane_tx: mpsc::UnboundedSender<PaneEvent>,
    focus: FocusHandle,
    glyphs: HashMap<SessionId, Entity<StatusGlyph>>,
    open_checks_for: Option<String>,
    overflow_open: bool,
    /// Paced PTY resizes: window and sidebar drags relayout every frame, but
    /// grid frames only leave the daemon every 50ms, so intermediate sizes are
    /// coalesced onto that cadence rather than dropped (see [`RESIZE_CADENCE`]).
    pending_resizes: HashMap<SessionId, (u16, u16)>,
    resize_flush: Option<Task<()>>,
    /// A cadence tick is already armed; further changes fold into it instead of
    /// rescheduling (which is what used to starve the flush during a drag).
    resize_flush_armed: bool,
    last_resize_sent: Option<Instant>,
    /// Grids held still while a column change round-trips. Keyed by session id
    /// so a hold follows the session rather than the pane: selection can move
    /// on mid-hold, and the parked frames still belong to the session that was
    /// resized.
    reflow_holds: HashMap<SessionId, ReflowHold>,
    started_at: Instant,
    repaint_pacer: RepaintPacer,
    session_source: SessionSource,
    /// Last selection observed by the primary pane. Spawn responses select the
    /// daemon-created id asynchronously, so this transition is also the
    /// reliable point at which keyboard focus can leave the picker.
    observed_selected_id: Option<SessionId>,
    preview: bool,
    lineage_tree_scroll: ScrollHandle,
    viewport: Option<TerminalViewport>,
    chrome: ShellChrome,
    navigation: Option<Entity<NavigationOverlay>>,
    utility_surfaces: Option<Entity<UtilitySurfaces>>,
    staged_images: ImageStore,
    attachment_ui: Option<AttachmentUi>,
    pending_upload: Option<PendingUpload>,
    _pane_events: Task<()>,
    _store_changes: Task<()>,
    _focus_subscriptions: Vec<Subscription>,
}

impl EventEmitter<TerminalPaneEvent> for TerminalPane {}

impl TerminalPane {
    pub fn new(
        runtime: Arc<StoreRuntime>,
        tokio_owner: Arc<tokio::runtime::Runtime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_source(
            runtime,
            tokio_owner,
            SessionSource::FollowSelection,
            false,
            window,
            cx,
        )
    }

    pub fn new_preview(
        runtime: Arc<StoreRuntime>,
        tokio_owner: Arc<tokio::runtime::Runtime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_source(
            runtime,
            tokio_owner,
            SessionSource::FollowSelection,
            true,
            window,
            cx,
        )
    }

    pub fn new_fixed(
        runtime: Arc<StoreRuntime>,
        tokio_owner: Arc<tokio::runtime::Runtime>,
        session_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_source(
            runtime,
            tokio_owner,
            SessionSource::Fixed(session_id),
            false,
            window,
            cx,
        )
    }

    fn new_with_source(
        runtime: Arc<StoreRuntime>,
        tokio_owner: Arc<tokio::runtime::Runtime>,
        session_source: SessionSource,
        preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        if matches!(session_source, SessionSource::FollowSelection) {
            window.focus(&focus, cx);
        }
        let focus_in = cx.on_focus_in(&focus, window, |this, _window, cx| {
            this.report_terminal_focus(true, cx);
        });
        let focus_out = cx.on_focus_out(&focus, window, |this, _event, _window, cx| {
            this.report_terminal_focus(false, cx);
        });
        let (pane_tx, mut pane_rx) = mpsc::unbounded_channel();
        let pane_events = cx.spawn_in(window, async move |this, cx| {
            let mut batch = Vec::new();
            while let Some(event) = pane_rx.recv().await {
                // Drain whatever else has queued and cross to the main thread
                // once per burst, not once per frame: with several attached
                // sessions streaming, per-event hops made the UI thread wake
                // at frame-rate × session-count.
                batch.push(event);
                while let Ok(next) = pane_rx.try_recv() {
                    batch.push(next);
                }
                if this
                    .update_in(cx, |this, window, cx| {
                        for event in batch.drain(..) {
                            this.handle_pane_event(event, window, cx);
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        });

        let mut changes = runtime.changes();
        let store_changes = cx.spawn_in(window, async move |this, cx| {
            loop {
                match changes.recv().await {
                    Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if this
                            .update_in(cx, |this, window, cx| {
                                this.reconcile_store_change(window, cx);
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

        let tokio = tokio_owner.handle().clone();
        let observed_selected_id = matches!(session_source, SessionSource::FollowSelection)
            .then(|| {
                runtime
                    .store
                    .read()
                    .expect("session store lock poisoned")
                    .selected_session_id()
                    .cloned()
            })
            .flatten();
        let mut pane = Self {
            runtime,
            _tokio_owner: tokio_owner,
            tokio,
            residents: HashMap::new(),
            parked_grids: Vec::new(),
            pane_tx,
            focus,
            glyphs: HashMap::new(),
            open_checks_for: None,
            overflow_open: false,
            pending_resizes: HashMap::new(),
            resize_flush: None,
            resize_flush_armed: false,
            last_resize_sent: None,
            reflow_holds: HashMap::new(),
            started_at: Instant::now(),
            repaint_pacer: RepaintPacer::new(ACTIVE_REPAINT_INTERVAL),
            session_source,
            observed_selected_id,
            preview,
            lineage_tree_scroll: ScrollHandle::new(),
            viewport: None,
            chrome: ShellChrome {
                sidebar_visible: true,
                ..ShellChrome::default()
            },
            navigation: None,
            utility_surfaces: None,
            staged_images: ImageStore::default(),
            attachment_ui: None,
            pending_upload: None,
            _pane_events: pane_events,
            _store_changes: store_changes,
            _focus_subscriptions: vec![focus_in, focus_out],
        };
        pane.reconcile_residency();
        pane.sync_status_glyphs(pane.current_colors(), window, cx);
        pane
    }

    fn reconcile_residency(&mut self) {
        let store = self
            .runtime
            .store
            .read()
            .expect("session store lock poisoned");
        let resident_ids: HashSet<_> = match &self.session_source {
            SessionSource::FollowSelection => {
                store.terminal_residency().resident().cloned().collect()
            }
            SessionSource::Fixed(id) if store.sessions().contains_key(id) => {
                HashSet::from([id.clone()])
            }
            SessionSource::Fixed(_) => HashSet::new(),
        };
        // A parked grid for a session the store no longer lists is dead
        // weight; one for a session that just became resident is superseded
        // below by promotion.
        self.parked_grids
            .retain(|(id, _)| store.sessions().contains_key(id));
        let preview_grids: HashMap<SessionId, GridBuffer> = if self.preview {
            store
                .sessions()
                .iter()
                .filter(|(id, _)| resident_ids.contains(*id) && !self.residents.contains_key(*id))
                .map(|(id, session)| (id.clone(), preview_session_grid(session)))
                .collect()
        } else {
            HashMap::new()
        };
        drop(store);
        // Park the last-known grid of every session about to be evicted, so
        // re-selecting it paints instantly instead of flashing blank while
        // the fresh attachment round-trips.
        for (id, resident) in &self.residents {
            if resident_ids.contains(id) {
                continue;
            }
            self.parked_grids.retain(|(parked, _)| parked != id);
            self.parked_grids
                .push((id.clone(), resident.element.buffer()));
        }
        if self.parked_grids.len() > PARKED_GRID_CAP {
            let excess = self.parked_grids.len() - PARKED_GRID_CAP;
            self.parked_grids.drain(..excess);
        }
        self.residents.retain(|id, _| resident_ids.contains(id));
        // A hold outliving its resident would park frames belonging to a
        // session id that has been re-attached since, and paint them into a
        // grid that never asked for them.
        let residents = &self.residents;
        self.reflow_holds.retain(|id, _| residents.contains_key(id));

        let socket = self.runtime.client().socket_path().to_path_buf();
        for id in resident_ids {
            if self.residents.contains_key(&id) {
                continue;
            }
            let mut mono = font(crate::fonts::mono_family());
            mono.fallbacks = Some(gpui::FontFallbacks::from_fonts(vec![
                ".SF NS Mono".to_owned(),
                "Menlo".to_owned(),
                "Apple Symbols".to_owned(),
                "STIX Two Math".to_owned(),
                "Apple Color Emoji".to_owned(),
            ]));
            let parked = self
                .parked_grids
                .iter()
                .position(|(parked, _)| parked == &id)
                .map(|index| self.parked_grids.remove(index).1);
            let (attachment, attachment_state) = if self.preview {
                (AttachmentControl::inert(), AttachmentState::Live)
            } else {
                (
                    spawn_attachment(
                        &self.tokio,
                        socket.clone(),
                        id.clone(),
                        self.pane_tx.clone(),
                    ),
                    AttachmentState::Attaching,
                )
            };
            let ime_attachment = attachment.clone();
            let element = match parked {
                // The parked cells paint on the first frame; the attach's
                // full snapshot overwrites the same shared buffer moments
                // later, so stale content lives for one round-trip at most.
                Some(buffer) => TerminalElement::new(buffer),
                None if self.preview => TerminalElement::with_buffer(
                    preview_grids
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(GridBuffer::default),
                ),
                None => TerminalElement::with_buffer(GridBuffer::default()),
            }
            .font(mono)
            .focus_handle(self.focus.clone())
            .on_text_input(move |text| ime_attachment.input(text.as_bytes().to_vec()));
            self.residents.insert(
                id,
                ResidentTerminal {
                    element,
                    attachment,
                    attachment_state,
                    input_modes: TermInputModes::default(),
                    mouse_modes: MouseModes::default(),
                    suppress_left_report: false,
                    reported_button_down: None,
                    last_reported_mouse: None,
                    find: None,
                    find_query: QueryEditor::default(),
                    last_size: (0, 0),
                },
            );
        }
    }

    fn reconcile_store_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected_id = matches!(self.session_source, SessionSource::FollowSelection)
            .then(|| {
                self.runtime
                    .store
                    .read()
                    .expect("session store lock poisoned")
                    .selected_session_id()
                    .cloned()
            })
            .flatten();
        let previous_id = self.observed_selected_id.clone();
        let selection_changed = selected_id != previous_id;
        let was_focused = self.focus.is_focused(window);
        if selection_changed
            && was_focused
            && let Some(previous) = previous_id.as_ref()
            && let Some(resident) = self.residents.get(previous)
            && resident.input_modes.focus_reporting
        {
            resident.attachment.input(b"\x1b[O".to_vec());
        }
        self.observed_selected_id = selected_id.clone();

        self.reconcile_residency();
        self.sync_status_glyphs(self.current_colors(), window, cx);

        // Explicit sidebar clicks already focus through SessionActivated, but
        // successful spawns select their daemon-assigned id on the async store
        // path. Following the selection here covers both RPC/event orderings
        // and avoids trying to focus a terminal before its id exists.
        if selection_changed && let Some(selected_id) = selected_id {
            if was_focused {
                if let Some(resident) = self.residents.get(&selected_id)
                    && resident.input_modes.focus_reporting
                {
                    resident.attachment.input(b"\x1b[I".to_vec());
                }
            } else {
                window.focus(&self.focus, cx);
            }
        }
        cx.notify();
    }

    pub fn resident_buffers(&mut self) -> HashMap<SessionId, SharedGridBuffer> {
        self.reconcile_residency();
        self.residents
            .iter()
            .map(|(id, resident)| (id.clone(), resident.element.buffer()))
            .collect()
    }

    pub fn set_shell_entities(
        &mut self,
        navigation: Entity<NavigationOverlay>,
        utility_surfaces: Entity<UtilitySurfaces>,
    ) {
        self.navigation = Some(navigation);
        self.utility_surfaces = Some(utility_surfaces);
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    fn report_terminal_focus(&mut self, focused: bool, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id() else {
            return;
        };
        let Some(resident) = self.residents.get_mut(&id) else {
            return;
        };
        if resident.input_modes.focus_reporting {
            resident
                .attachment
                .input(if focused { b"\x1b[I" } else { b"\x1b[O" }.to_vec());
        }
        cx.notify();
    }

    pub fn set_viewport(&mut self, viewport: TerminalViewport, cx: &mut Context<Self>) {
        if self.viewport == Some(viewport) {
            return;
        }
        self.viewport = Some(viewport);
        cx.notify();
    }

    pub fn set_shell_chrome(&mut self, chrome: ShellChrome, cx: &mut Context<Self>) {
        if self.chrome == chrome {
            return;
        }
        self.chrome = chrome;
        cx.notify();
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    fn sync_status_glyphs(
        &mut self,
        colors: SemanticColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fixed_id = match &self.session_source {
            SessionSource::Fixed(id) => Some(id),
            SessionSource::FollowSelection => None,
        };
        let snapshots: Vec<_> = {
            let store = self
                .runtime
                .store
                .read()
                .expect("session store lock poisoned");
            store
                .sessions()
                .iter()
                .filter(|(id, _)| fixed_id.is_none_or(|fixed| fixed == *id))
                .map(|(id, session)| {
                    (
                        id.clone(),
                        ui_agent_kind(session.effective_kind()),
                        status_state(session),
                    )
                })
                .collect()
        };
        self.glyphs
            .retain(|id, _| snapshots.iter().any(|(live, _, _)| live == id));
        for (id, kind, state) in snapshots {
            if let Some(glyph) = self.glyphs.get(&id) {
                glyph.update(cx, |glyph, cx| {
                    glyph.set_kind(kind, cx);
                    glyph.set_state(state, window, cx);
                    glyph.set_colors(colors, cx);
                });
            } else {
                let glyph = StatusGlyph::entity(kind, state, 16.0, colors, cx);
                self.glyphs.insert(id, glyph);
            }
        }
    }

    fn current_colors(&self) -> SemanticColors {
        let store = self
            .runtime
            .store
            .read()
            .expect("session store lock poisoned");
        crate::app_theme::colors(&store.preferences().terminal_theme)
    }

    fn handle_pane_event(&mut self, event: PaneEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            PaneEvent::InteractiveInput => self.repaint_pacer.prioritize_interactive_damage(),
            PaneEvent::AttachmentState(id, state) => {
                if let Some(resident) = self.residents.get_mut(&id) {
                    resident.attachment_state = state;
                }
                if self.selected_id().as_ref() == Some(&id) {
                    cx.notify();
                }
            }
            PaneEvent::Chunk(id, TerminalChunk::Grid(update)) => {
                if let Some(hold) = self.reflow_holds.get_mut(&id) {
                    if hold.park(update) {
                        self.release_reflow_hold(&id, window, cx);
                    }
                    return;
                }
                self.apply_grid_updates(id, [update], window, cx);
            }
            PaneEvent::Chunk(id, TerminalChunk::Modes(modes)) => {
                let selected = self.selected_id().as_ref() == Some(&id);
                let pane_focused = self.focus.is_focused(window);
                if let Some(resident) = self.residents.get_mut(&id) {
                    let previously_reported_focus = resident.input_modes.focus_reporting;
                    resident.element.set_modes(
                        modes.alt_screen,
                        modes.mouse_reporting,
                        modes.alternate_scroll,
                    );
                    resident.input_modes = terminal_input_modes(modes);
                    resident.mouse_modes = terminal_mouse_modes(modes);
                    if !modes.mouse_reporting {
                        resident.suppress_left_report = false;
                        resident.reported_button_down = None;
                        resident.last_reported_mouse = None;
                    }
                    if selected
                        && pane_focused
                        && !previously_reported_focus
                        && modes.focus_reporting
                    {
                        resident.attachment.input(b"\x1b[I".to_vec());
                    }
                }
                if selected {
                    cx.notify();
                }
            }
            PaneEvent::Chunk(_, TerminalChunk::Pong) => {}
            PaneEvent::FindSnapshot(id, request, snapshot) => {
                let visible = self.selected_id().as_ref() == Some(&id);
                if let Some(resident) = self.residents.get_mut(&id)
                    && let Some(find) = resident.find.as_mut()
                    && resident
                        .element
                        .apply_find_snapshot(find, &request, snapshot)
                {
                    resident.element.sync_find_highlights(find);
                    if visible {
                        cx.notify();
                    }
                }
            }
            PaneEvent::ScrollbackCells(id, result, visible_rows) => {
                if let Some(resident) = self.residents.get_mut(&id) {
                    let _ = resident
                        .element
                        .complete_scrollback_fetch(result, visible_rows);
                }
                self.pump_scrollback_fetch(&id, visible_rows);
                if self.selected_id().as_ref() == Some(&id) {
                    cx.notify();
                }
            }
            PaneEvent::ScrollbackFailed(id) => {
                if let Some(resident) = self.residents.get_mut(&id) {
                    resident.element.fail_scrollback_fetch();
                }
                if self.selected_id().as_ref() == Some(&id) {
                    cx.notify();
                }
            }
            PaneEvent::AttachmentUploadFinished(id, result, local_paths) => {
                self.finish_remote_upload(id, result, local_paths, cx);
            }
        }
    }

    /// Applies grid frames to a resident and repaints if what landed is worth a
    /// frame. Takes a batch because a held reflow releases its parked frames
    /// together: applying them one by one would paint each intermediate.
    fn apply_grid_updates(
        &mut self,
        id: SessionId,
        updates: impl IntoIterator<Item = GridUpdate>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = self.started_at.elapsed();
        let selected = self.selected_id();
        let mut schedule_find = false;
        let mut changed = false;
        let mut applied = false;
        if let Some(resident) = self.residents.get_mut(&id) {
            for update in updates {
                applied = true;
                changed |= resident.element.apply_damage(update).changed;
            }
            if applied && let Some(find) = resident.find.as_mut() {
                schedule_find = find.on_output(now);
            }
        }
        if !applied {
            return;
        }
        // Visibility/occlusion is GPUI's job (display-link stops when the
        // window is truly hidden). `is_window_active` is only OS focus, so
        // gating on it freezes a still-visible window on another monitor.
        let repaint = terminal_damage_should_repaint(selected.as_ref(), &id, changed);
        if schedule_find {
            self.schedule_find(id, Duration::from_millis(100), window, cx);
        }
        if repaint {
            self.request_terminal_repaint(window, cx);
        }
    }

    /// Holds a session's grid still until its column change has fully
    /// round-tripped. A hold already in flight is extended rather than
    /// released, so a second change landing mid-hold covers its own reflow too;
    /// its frames carry over, because a daemon that never answers the second
    /// resize (a hibernated tree, a session the phone owns) would otherwise
    /// leave the pane painting whatever was on screen before the first one.
    fn hold_reflow(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        let held = id.clone();
        let release = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(REFLOW_HOLD).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.release_reflow_hold(&held, window, cx);
            });
        });
        let parked = self
            .reflow_holds
            .remove(&id)
            .map_or_else(Vec::new, |hold| hold.parked);
        self.reflow_holds.insert(
            id,
            ReflowHold {
                parked,
                saw_snapshot: false,
                _release: release,
            },
        );
    }

    /// Ends a hold and paints everything it parked as a single frame.
    fn release_reflow_hold(&mut self, id: &SessionId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hold) = self.reflow_holds.remove(id) else {
            return;
        };
        self.apply_grid_updates(id.clone(), hold.parked, window, cx);
    }

    fn request_terminal_repaint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.repaint_pacer.on_damage(self.started_at.elapsed()) {
            RepaintAction::RepaintNow => cx.notify(),
            RepaintAction::Schedule(delay) => {
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = this.update_in(cx, |this, _window, cx| {
                        if this.repaint_pacer.on_timer(this.started_at.elapsed()) {
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            RepaintAction::None => {}
        }
    }

    fn schedule_find(
        &self,
        id: SessionId,
        delay: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update_in(cx, |this, _window, _cx| this.start_due_find(&id));
        })
        .detach();
    }

    fn start_due_find(&mut self, id: &SessionId) {
        let now = self.started_at.elapsed();
        let Some(request) = self
            .residents
            .get_mut(id)
            .and_then(|resident| resident.find.as_mut())
            .and_then(|find| find.take_due_search(now))
        else {
            return;
        };
        let client = Arc::clone(self.runtime.client());
        let pane_tx = self.pane_tx.clone();
        let id = id.clone();
        self.tokio.spawn(async move {
            if let Ok(snapshot) = client.read_scrollback(&id).await {
                let _ = pane_tx.send(PaneEvent::FindSnapshot(id, request, snapshot.into()));
            }
        });
    }

    fn selected_id(&self) -> Option<SessionId> {
        match &self.session_source {
            SessionSource::FollowSelection => self
                .runtime
                .store
                .read()
                .expect("session store lock poisoned")
                .selected_session_id()
                .cloned(),
            SessionSource::Fixed(id) => Some(id.clone()),
        }
    }

    fn selected_session(&self) -> Option<Arc<SessionRecord>> {
        let id = self.selected_id()?;
        self.runtime
            .store
            .read()
            .expect("session store lock poisoned")
            .sessions()
            .get(&id)
            .map(Arc::clone)
    }

    fn render_empty_session(&self, colors: SemanticColors) -> AnyElement {
        let workspace = self
            .runtime
            .store
            .read()
            .expect("session store lock poisoned")
            .active_workspace()
            .map(|project| project.name.clone());
        let detail = workspace.map_or_else(
            || "Add a workspace from the sidebar to get started.".to_owned(),
            |workspace| format!("Start an agent from the sidebar in {workspace}."),
        );
        div()
            .id("workspace-empty")
            .debug_selector(|| "WORKSPACE_EMPTY".into())
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(5.0))
            .child(
                div()
                    .text_size(px(Typo::ROW_EMPHASIZED.size))
                    .font_weight(Typo::ROW_EMPHASIZED.weight)
                    .text_color(colors.secondary)
                    .child("No active session"),
            )
            .child(
                div()
                    .text_size(px(Typo::META.size))
                    .text_color(colors.tertiary)
                    .child(detail),
            )
            .into_any_element()
    }

    fn open_find(&mut self, _: &OpenFind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id() else {
            return;
        };
        let Some(resident) = self.residents.get_mut(&id) else {
            return;
        };
        if resident.find.is_none() {
            resident.find = Some(TerminalFindModel::default());
            // Reopening keeps the last query but selects it, so ⌘F then typing
            // starts a new search while ⌘F then ⏎ repeats the old one.
            resident.find_query.select_all();
        }
        window.focus(&self.focus, cx);
        cx.stop_propagation();
        cx.notify();
    }

    fn close_find(&mut self, _: &CloseFind, _window: &mut Window, cx: &mut Context<Self>) {
        if self.close_find_for_selected() {
            cx.stop_propagation();
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    fn close_find_for_selected(&mut self) -> bool {
        let Some(id) = self.selected_id() else {
            return false;
        };
        let Some(resident) = self.residents.get_mut(&id) else {
            return false;
        };
        if resident.find.take().is_none() {
            return false;
        }
        resident.element.set_find_highlights(Vec::new());
        true
    }

    fn find_next(&mut self, _: &FindNext, _window: &mut Window, cx: &mut Context<Self>) {
        self.navigate_find(false, cx);
    }

    fn find_previous(&mut self, _: &FindPrevious, _window: &mut Window, cx: &mut Context<Self>) {
        self.navigate_find(true, cx);
    }

    fn navigate_find(&mut self, backwards: bool, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id() else {
            return;
        };
        let Some(resident) = self.residents.get_mut(&id) else {
            return;
        };
        let Some(find) = resident.find.as_mut() else {
            return;
        };
        if backwards {
            resident.element.find_previous(find);
        } else {
            resident.element.find_next(find);
        }
        resident.element.sync_find_highlights(find);
        cx.stop_propagation();
        cx.notify();
    }

    fn zoom_in(&mut self, _: &ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        self.change_zoom(1.0, false, window, cx);
    }

    fn zoom_out(&mut self, _: &ZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        self.change_zoom(-1.0, false, window, cx);
    }

    fn reset_zoom(&mut self, _: &ResetZoom, window: &mut Window, cx: &mut Context<Self>) {
        self.change_zoom(0.0, true, window, cx);
    }

    fn change_zoom(
        &mut self,
        delta: f32,
        reset: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = {
            let mut store = self
                .runtime
                .store
                .write()
                .expect("session store lock poisoned");
            if reset {
                store.reset_terminal_zoom()
            } else {
                store.zoom_terminal(delta)
            }
        };
        if result.is_ok() {
            self.update_selected_geometry(window, cx);
            cx.stop_propagation();
            cx.notify();
        }
    }

    /// Grid cell under a window-space pointer position, using the same
    /// geometry as `handle_scroll`.
    fn grid_cell_at(
        &self,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
    ) -> Option<(usize, usize)> {
        self.selected_session()?;
        let store = self
            .runtime
            .store
            .read()
            .expect("session store lock poisoned");
        let font_size = store.preferences().terminal_font_size;
        drop(store);
        let metrics = CellMetrics::measure(
            window.text_system(),
            &font(crate::fonts::mono_family()),
            px(font_size),
        );
        let viewport = self.viewport.unwrap_or_default();
        let grid_x = viewport.x + GRID_HORIZONTAL_PADDING / 2.0;
        // An overflowing grid is bottom-anchored (see render_grid_and_overlays),
        // so its first row sits above the surface -- selection has to follow it
        // or clicks land on the wrong line while a resize is in flight.
        let resident = self.selected_id().and_then(|id| self.residents.get(&id))?;
        let grid_cols = resident.element.grid_cols();
        let grid_rows = resident.element.grid_rows();
        if grid_cols == 0 || grid_rows == 0 {
            return None;
        }
        let anchor = self
            .grid_row_overflow(grid_rows, font_size, window)
            .map_or(0.0, |grid_height| self.grid_inner_height() - grid_height);
        let grid_y = viewport.y + Metrics::TITLE_BAR + self.lineage_chrome_height() + 2.0 + anchor;
        let col = ((f32::from(position.x) - grid_x) / f32::from(metrics.cell_width))
            .floor()
            .max(0.0) as usize;
        let row = ((f32::from(position.y) - grid_y) / f32::from(metrics.line_height))
            .floor()
            .max(0.0) as usize;
        Some((
            col.min(usize::from(grid_cols - 1)),
            row.min(usize::from(grid_rows - 1)),
        ))
    }

    /// The height the mirrored grid needs when the daemon's screen is taller
    /// than the pane can show, or `None` when it fits. Only a resize still in
    /// flight puts the two out of step, so this is `None` on settled frames.
    fn grid_row_overflow(
        &self,
        grid_rows: u16,
        font_size: f32,
        window: &mut Window,
    ) -> Option<f32> {
        if grid_rows == 0 || self.viewport.is_none() {
            return None;
        }
        let metrics = CellMetrics::measure(
            window.text_system(),
            &font(crate::fonts::mono_family()),
            px(font_size),
        );
        // A pixel of slack on top of the exact row height: the element derives
        // its row count back out with `floor(height / line_height)`, and an
        // exactly-sized box loses its last row to float error or to layout
        // rounding -- which is the row this anchoring exists to keep on screen.
        (grid_rows > metrics.rows_for_height(px(self.grid_inner_height())))
            .then(|| f32::from(metrics.line_height).mul_add(f32::from(grid_rows), ANCHOR_SLACK))
    }

    /// Height available to `TerminalElement` inside the terminal surface -- the
    /// same figure [`estimated_grid_size`] turns into a row count.
    fn grid_inner_height(&self) -> f32 {
        let height = self.viewport.map_or(0.0, |viewport| viewport.height);
        (height
            - Metrics::TITLE_BAR
            - self.lineage_chrome_height()
            - GRID_VERTICAL_PADDING
            - GRID_LAYOUT_VERTICAL_CHROME)
            .max(1.0)
    }

    fn lineage_strip(&self) -> Option<LineageStrip> {
        if !matches!(self.session_source, SessionSource::FollowSelection) {
            return None;
        }
        let id = self.selected_id()?;
        self.runtime
            .store
            .read()
            .expect("session store lock poisoned")
            .lineage_strip_for(&id)
    }

    fn lineage_view(&self) -> LineageView {
        self.runtime
            .store
            .read()
            .expect("session store lock poisoned")
            .lineage_view()
    }

    fn lineage_tree_open(&self) -> bool {
        self.lineage_view() == LineageView::Tree && self.lineage_strip().is_some()
    }

    fn set_lineage_view(&mut self, view: LineageView) {
        self.runtime
            .store
            .write()
            .expect("session store lock poisoned")
            .set_lineage_view(view);
    }

    fn lineage_chrome_height(&self) -> f32 {
        if self.lineage_strip().is_some() {
            Metrics::LINEAGE_STRIP
        } else {
            0.0
        }
    }

    fn copy_selection(&mut self, _: &CopySelection, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id() else {
            return;
        };
        let Some(resident) = self.residents.get_mut(&id) else {
            return;
        };
        let text = resident.element.selected_text();
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(id) = self.selected_id() else {
            return;
        };

        if let Some((bytes, extension)) = clipboard_image(&item) {
            let in_find = self
                .residents
                .get(&id)
                .is_some_and(|resident| resident.find.is_some());
            if in_find {
                return;
            }
            match stage_bytes(bytes, format!("clipboard.{extension}")) {
                Ok(staged) => self.deliver_images(id, vec![staged], window, cx),
                Err(error) => self.set_attachment_ui(
                    AttachmentUi::Failed {
                        message: error.user_message().to_owned(),
                    },
                    cx,
                ),
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }

        let Some(text) = item.text() else {
            return;
        };
        let now = self.started_at.elapsed();
        let Some(resident) = self.residents.get_mut(&id) else {
            return;
        };
        if let Some(find) = resident.find.as_mut() {
            resident.find_query.insert(&text);
            let query = resident.find_query.text().to_owned();
            find.set_query(query, now);
            self.schedule_find(id, Duration::from_millis(200), window, cx);
        } else {
            resident
                .attachment
                .input(paste(&text, resident.input_modes.bracketed_paste));
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn set_attachment_ui(&mut self, ui: AttachmentUi, cx: &mut Context<Self>) {
        self.attachment_ui = Some(ui);
        cx.notify();
    }

    fn session_image_capability(
        &self,
        id: &SessionId,
    ) -> (Option<zeus_proto::AgentDescriptor>, bool) {
        let store = self
            .runtime
            .store
            .read()
            .expect("session store lock poisoned");
        let Some(session) = store.sessions().get(id) else {
            return (None, false);
        };
        let descriptor = store.agent_descriptor(&session.kind).cloned();
        let remote = session.host.is_some();
        (descriptor, remote)
    }

    fn deliver_images(
        &mut self,
        id: SessionId,
        images: Vec<StagedImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (descriptor, remote) = self.session_image_capability(&id);
        if capability_from_descriptor(descriptor.as_ref()).is_none() {
            let name = descriptor
                .as_ref()
                .map(|descriptor| descriptor.display_name.as_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("This agent");
            self.set_attachment_ui(
                AttachmentUi::Unsupported {
                    message: unsupported_message(name),
                },
                cx,
            );
            return;
        }
        let display_names: Vec<String> = images
            .iter()
            .map(|image| image.original_name.clone())
            .collect();
        let local_paths = keep_staged(&mut self.staged_images, images);
        if remote {
            self.start_remote_upload(id, local_paths, display_names, cx);
            return;
        }
        self.insert_attachment_paths(&id, &local_paths);
        self.set_attachment_ui(
            AttachmentUi::Success {
                message: attachment_success_message(&display_names),
            },
            cx,
        );
        self.focus(window, cx);
    }

    fn insert_attachment_paths(&mut self, id: &SessionId, paths: &[String]) {
        let payload = paste_paths(paths);
        if let Some(resident) = self.residents.get(id) {
            resident
                .attachment
                .input(paste(&payload, resident.input_modes.bracketed_paste));
        }
    }

    fn start_remote_upload(
        &mut self,
        id: SessionId,
        local_paths: Vec<String>,
        display_names: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.pending_upload = Some(PendingUpload {
            session_id: id.clone(),
            local_paths: local_paths.clone(),
            display_names: display_names.clone(),
        });
        self.set_attachment_ui(
            AttachmentUi::Progress {
                message: format!("Uploading {}…", attachment_count(&display_names)),
            },
            cx,
        );
        let client = Arc::clone(self.runtime.client());
        let pane_tx = self.pane_tx.clone();
        let upload_id = id;
        self.tokio.spawn(async move {
            let mut remote_paths = Vec::with_capacity(local_paths.len());
            for path in local_paths.clone() {
                match client.upload_attachment(&upload_id, path).await {
                    Ok(remote) => remote_paths.push(remote),
                    Err(error) => {
                        let _ = pane_tx.send(PaneEvent::AttachmentUploadFinished(
                            upload_id,
                            Err(error.to_string()),
                            local_paths,
                        ));
                        return;
                    }
                }
            }
            let _ = pane_tx.send(PaneEvent::AttachmentUploadFinished(
                upload_id,
                Ok(remote_paths),
                local_paths,
            ));
        });
    }

    fn finish_remote_upload(
        &mut self,
        id: SessionId,
        result: Result<Vec<String>, String>,
        local_paths: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(remote_paths) => {
                self.insert_attachment_paths(&id, &remote_paths);
                let names = self
                    .pending_upload
                    .as_ref()
                    .filter(|pending| pending.session_id == id)
                    .map(|pending| pending.display_names.clone())
                    .unwrap_or_default();
                self.pending_upload = None;
                self.set_attachment_ui(
                    AttachmentUi::Success {
                        message: attachment_success_message(&names),
                    },
                    cx,
                );
            }
            Err(error) => {
                self.pending_upload = Some(PendingUpload {
                    session_id: id,
                    local_paths,
                    display_names: self
                        .pending_upload
                        .as_ref()
                        .map(|pending| pending.display_names.clone())
                        .unwrap_or_default(),
                });
                self.set_attachment_ui(
                    AttachmentUi::Failed {
                        message: format!("Upload failed: {error}"),
                    },
                    cx,
                );
            }
        }
    }

    fn retry_pending_upload(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_upload.take() else {
            return;
        };
        self.start_remote_upload(
            pending.session_id,
            pending.local_paths,
            pending.display_names,
            cx,
        );
    }

    fn cancel_pending_upload(&mut self, cx: &mut Context<Self>) {
        self.pending_upload = None;
        self.attachment_ui = None;
        cx.notify();
    }

    fn handle_external_paths(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected_id() else {
            return;
        };
        let (descriptor, _) = self.session_image_capability(&id);
        match stage_drop(descriptor.as_ref(), paths.paths()) {
            Ok(images) => self.deliver_images(id, images, window, cx),
            Err(AttachmentDecision::Unsupported { message }) => {
                self.set_attachment_ui(AttachmentUi::Unsupported { message }, cx);
            }
            Err(AttachmentDecision::Rejected { message }) => {
                self.set_attachment_ui(AttachmentUi::Failed { message }, cx);
            }
            Err(AttachmentDecision::Ready { .. }) => {}
        }
        self.focus(window, cx);
    }

    fn preview_external_paths(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id() else {
            return;
        };
        let (descriptor, _) = self.session_image_capability(&id);
        match decide_drop(descriptor.as_ref(), paths.paths()) {
            AttachmentDecision::Ready { display_names } => {
                let message = format!("Drop {} to attach", attachment_count(&display_names));
                self.set_attachment_ui(
                    AttachmentUi::Hover {
                        names: display_names,
                        message,
                    },
                    cx,
                );
            }
            AttachmentDecision::Unsupported { message }
            | AttachmentDecision::Rejected { message } => {
                self.set_attachment_ui(
                    AttachmentUi::Hover {
                        names: Vec::new(),
                        message,
                    },
                    cx,
                );
            }
        }
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.platform {
            let handled = match event.keystroke.key.as_str() {
                "k" => self.navigation.as_ref().is_some_and(|navigation| {
                    navigation.update(cx, |navigation, cx| {
                        navigation.toggle_command_palette(&ToggleCommandPalette, window, cx);
                    });
                    true
                }),
                "p" => self.navigation.as_ref().is_some_and(|navigation| {
                    navigation.update(cx, |navigation, cx| {
                        navigation.toggle_quick_open(&ToggleQuickOpen, window, cx);
                    });
                    true
                }),
                "h" if event.keystroke.modifiers.shift => {
                    self.utility_surfaces.as_ref().is_some_and(|surfaces| {
                        surfaces.update(cx, |surfaces, cx| surfaces.toggle_history(cx));
                        true
                    })
                }
                "," => self.utility_surfaces.as_ref().is_some_and(|surfaces| {
                    surfaces.update(cx, |surfaces, cx| surfaces.open_settings(cx));
                    true
                }),
                "o" if event.keystroke.modifiers.shift => {
                    self.runtime
                        .store
                        .write()
                        .expect("session store lock poisoned")
                        .toggle_overview();
                    true
                }
                _ => false,
            };
            if handled {
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }

        if let Some(navigation) = &self.navigation
            && navigation.read(cx).is_open()
        {
            navigation.update(cx, |navigation, cx| {
                navigation.on_key_down(event, window, cx);
            });
            cx.stop_propagation();
            return;
        }
        if let Some(surfaces) = &self.utility_surfaces
            && surfaces.read(cx).is_open()
        {
            surfaces.update(cx, |surfaces, cx| {
                surfaces.key_down(event, window, cx);
            });
            cx.stop_propagation();
            return;
        }

        let switcher_key = switcher_key(event);
        let switcher_handled = {
            let mut store = self
                .runtime
                .store
                .write()
                .expect("session store lock poisoned");
            let was_visible = store.switcher_state().is_visible();
            let handled = if was_visible
                || matches!(
                    switcher_key,
                    crate::switcher::SwitcherKey::Tab { control: true, .. }
                ) {
                store.handle_switcher_key(switcher_key)
            } else {
                false
            };
            if handled && !was_visible && store.switcher_state().is_visible() {
                store.dismiss_overview();
            }
            handled
        };
        if switcher_handled {
            cx.stop_propagation();
            cx.notify();
            return;
        }

        if self.lineage_tree_open() {
            self.handle_lineage_tree_key(event, window, cx);
            cx.stop_propagation();
            return;
        }

        let Some(id) = self.selected_id() else {
            return;
        };
        let now = self.started_at.elapsed();
        let Some(resident) = self.residents.get_mut(&id) else {
            return;
        };

        if let Some(find) = resident.find.as_mut() {
            match event.keystroke.key.as_str() {
                "escape" => {
                    resident.find = None;
                    resident.element.set_find_highlights(Vec::new());
                    cx.notify();
                }
                "enter" => {
                    if event.keystroke.modifiers.shift {
                        resident.element.find_previous(find);
                    } else {
                        resident.element.find_next(find);
                    }
                    resident.element.sync_find_highlights(find);
                    cx.notify();
                }
                // Everything else is text editing, through the same key map the
                // command palette and Quick Open use.
                _ => {
                    let Some(edit) = query_editor::edit_for(&event.keystroke) else {
                        cx.propagate();
                        return;
                    };
                    let changed = match edit {
                        Edit::Local(local) => resident.find_query.apply(local),
                        Edit::Clipboard(ClipboardEdit::Copy) => {
                            query_editor::copy_selection(&resident.find_query, cx);
                            false
                        }
                        Edit::Clipboard(ClipboardEdit::Cut) => {
                            query_editor::cut_selection(&mut resident.find_query, cx)
                        }
                        // ⌘V is already an action (it also handles image
                        // pastes); claiming it here too would insert twice.
                        Edit::Clipboard(ClipboardEdit::Paste) => {
                            cx.propagate();
                            return;
                        }
                    };
                    if changed {
                        let query = resident.find_query.text().to_owned();
                        find.set_query(query, now);
                        self.schedule_find(id, Duration::from_millis(200), window, cx);
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }

        if event.keystroke.modifiers.platform && event.keystroke.key != "backspace" {
            cx.propagate();
            return;
        }
        let Some(term_event) = terminal_key_event(event) else {
            cx.propagate();
            return;
        };
        let modifiers = TermModifiers {
            shift: event.keystroke.modifiers.shift,
            ctrl: event.keystroke.modifiers.control,
            alt: event.keystroke.modifiers.alt,
            cmd: event.keystroke.modifiers.platform,
        };
        let option_as_meta = self
            .runtime
            .store
            .read()
            .expect("session store lock poisoned")
            .preferences()
            .terminal_option_as_meta;
        let mut input_modes = resident.input_modes;
        input_modes.option_as_meta = option_as_meta;
        let bytes = encode_key(&term_event, modifiers, input_modes);
        if bytes.is_empty() {
            cx.propagate();
        } else {
            resident.attachment.input(bytes);
            cx.stop_propagation();
        }
    }

    fn handle_key_up(&mut self, event: &KeyUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if matches!(event.keystroke.key.as_str(), "control" | "ctrl") {
            self.runtime
                .store
                .write()
                .expect("session store lock poisoned")
                .handle_switcher_modifiers_changed(false);
            cx.notify();
        }
    }

    fn handle_modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut store = self
            .runtime
            .store
            .write()
            .expect("session store lock poisoned");
        let was_visible = store.switcher_state().is_visible();
        store.handle_switcher_modifiers_changed(event.modifiers.control);
        if was_visible != store.switcher_state().is_visible() {
            cx.notify();
        }
    }

    /// Starts the next queued scrollback fetch for `id`, if the viewport wants
    /// one and none is in flight. Called from wheel events AND from fetch
    /// completion: a fast wheel burst queues the next window while a fetch is
    /// in flight, and nothing else would ever start it — the stranded queue
    /// painted as a transient blank region in deep scrollback.
    fn pump_scrollback_fetch(&mut self, id: &SessionId, visible_rows: usize) {
        let Some(resident) = self.residents.get_mut(id) else {
            return;
        };
        let Some(request) = resident.element.begin_scrollback_fetch(visible_rows) else {
            return;
        };
        let client = Arc::clone(self.runtime.client());
        let pane_tx = self.pane_tx.clone();
        let fetch_id = id.clone();
        self.tokio.spawn(async move {
            match client
                .read_scrollback_cells(&fetch_id, request.first_row, request.max_rows)
                .await
            {
                Ok(result) => {
                    let _ =
                        pane_tx.send(PaneEvent::ScrollbackCells(fetch_id, result, visible_rows));
                }
                Err(_) => {
                    let _ = pane_tx.send(PaneEvent::ScrollbackFailed(fetch_id));
                }
            }
        });
    }

    fn report_mouse_button(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        modifiers: gpui::Modifiers,
        button: TermMouseButton,
        pressed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected_id() else {
            return;
        };
        let Some((col, row)) = self.grid_cell_at(position, window) else {
            return;
        };
        let Some(resident) = self.residents.get_mut(&id) else {
            return;
        };
        let continuing_report = resident.reported_button_down == Some(button);
        if !resident.mouse_modes.reporting || (modifiers.shift && !continuing_report) {
            return;
        }
        let report = if pressed {
            press_report(
                col,
                row,
                button,
                terminal_mouse_modifiers(modifiers),
                resident.mouse_modes,
            )
        } else {
            release_report(
                col,
                row,
                button,
                terminal_mouse_modifiers(modifiers),
                resident.mouse_modes,
            )
        };
        if let Some(bytes) = report {
            resident.attachment.input(bytes);
        }
        resident.reported_button_down = pressed.then_some(button);
        resident.last_reported_mouse = pressed.then_some((col, row, Some(button)));
        window.focus(&self.focus, cx);
        cx.stop_propagation();
    }

    fn handle_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected_id() else {
            return;
        };
        let font_size = self
            .runtime
            .store
            .read()
            .expect("session store lock poisoned")
            .preferences()
            .terminal_font_size;
        let font = font(crate::fonts::mono_family());
        let metrics = CellMetrics::measure(window.text_system(), &font, px(font_size));
        let Some((col, row)) = self.grid_cell_at(event.position, window) else {
            return;
        };
        let col = u16::try_from(col).unwrap_or(u16::MAX);
        let row = u16::try_from(row).unwrap_or(u16::MAX);
        let delta = match event.delta {
            ScrollDelta::Pixels(point) => WheelDelta::PrecisePoints(f32::from(point.y)),
            ScrollDelta::Lines(point) => WheelDelta::Lines(point.y),
        };
        let Some(resident) = self.residents.get_mut(&id) else {
            return;
        };
        let visible_rows = resident.last_size.1.max(1);
        let route = resident.element.route_wheel(WheelEvent {
            delta,
            col,
            row,
            visible_rows,
            line_height: f32::from(metrics.line_height),
        });
        match route {
            Some(WheelRoute::Mouse {
                up,
                lines,
                col,
                row,
            }) => {
                let reports = wheel_reports(
                    usize::from(col),
                    usize::from(row),
                    up,
                    usize::from(lines),
                    terminal_mouse_modifiers(event.modifiers),
                    resident.mouse_modes,
                );
                if let Some(reports) = reports {
                    resident.attachment.input(reports.flatten().collect());
                }
            }
            Some(WheelRoute::AlternateScroll { up, lines }) => {
                resident
                    .attachment
                    .input(alternate_scroll(up, usize::from(lines)));
            }
            Some(WheelRoute::Local { .. }) => {
                self.pump_scrollback_fetch(&id, usize::from(visible_rows));
            }
            None => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn reflow_preview_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.selected_session() else {
            return;
        };
        let font_size = self
            .runtime
            .store
            .read()
            .expect("session store lock poisoned")
            .preferences()
            .terminal_font_size;
        let viewport = self.viewport.unwrap_or_else(|| {
            let size = window.viewport_size();
            TerminalViewport {
                x: 0.0,
                y: 0.0,
                width: f32::from(size.width),
                height: f32::from(size.height),
            }
        });
        let metrics = CellMetrics::measure(
            window.text_system(),
            &font(crate::fonts::mono_family()),
            px(font_size),
        );
        let size = estimated_grid_size(
            viewport.width,
            viewport.height,
            0.0,
            self.lineage_chrome_height(),
            metrics,
        );
        let Some(resident) = self.residents.get_mut(&session.id) else {
            return;
        };
        if resident.last_size == size {
            return;
        }
        resident.last_size = size;
        let grid = preview_session_grid_sized(session.as_ref(), size.0, size.1);
        let changed_rows = (0..grid.rows)
            .filter_map(|row| {
                grid.row(usize::from(row))
                    .map(|cells| ChangedRow::new(row, cells.to_vec()))
            })
            .collect();
        resident.element.apply(
            GridUpdate {
                cols: grid.cols,
                rows: grid.rows,
                cursor_col: grid.cursor.col,
                cursor_row: grid.cursor.row,
                cursor_visible: grid.cursor.visible,
                is_full_snapshot: true,
                changed_rows,
            },
            window,
        );
        cx.notify();
    }

    fn update_selected_geometry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.lineage_tree_open() {
            return;
        }
        if self.preview {
            self.reflow_preview_grid(window, cx);
            return;
        }
        let font_size = self
            .runtime
            .store
            .read()
            .expect("session store lock poisoned")
            .preferences()
            .terminal_font_size;
        let viewport = self.viewport.unwrap_or_else(|| {
            let size = window.viewport_size();
            TerminalViewport {
                x: 0.0,
                y: 0.0,
                width: f32::from(size.width),
                height: f32::from(size.height),
            }
        });
        let metrics = CellMetrics::measure(
            window.text_system(),
            &font(crate::fonts::mono_family()),
            px(font_size),
        );
        let size = estimated_grid_size(
            viewport.width,
            viewport.height,
            0.0,
            self.lineage_chrome_height(),
            metrics,
        );
        self.apply_grid_size(size, window, cx);
    }

    /// Resize the selected session's PTY to `size`, coalesced by the existing
    /// drag cadence. The settled viewport estimate is the only source: measuring
    /// the painted box as well disagreed by a few chrome pixels and ping-ponged
    /// the PTY, which flickered the grid with full snapshots.
    fn apply_grid_size(&mut self, size: (u16, u16), window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.selected_session() else {
            return;
        };
        if let Some(resident) = self.residents.get_mut(&session.id)
            && resident.last_size != size
        {
            // Leading edge: an isolated change (first measure after attach, a
            // session switch, a window snap, the first frame of a drag) reaches
            // the daemon immediately so the pane feels instant.
            let previous = resident.last_size;
            let first_measure = previous == (0, 0);
            resident.last_size = size;
            let now = Instant::now();
            let since_sent = self.last_resize_sent.map(|at| now.duration_since(at));
            let delay = match plan_resize(first_measure, since_sent, self.resize_flush_armed) {
                ResizePlan::SendNow => {
                    self.last_resize_sent = Some(now);
                    self.pending_resizes.remove(&session.id);
                    resident.attachment.resize(size.0, size.1);
                    if should_hold_reflow(previous, size, since_sent) {
                        self.hold_reflow(session.id.clone(), window, cx);
                    }
                    return;
                }
                // Mid-drag: fold into the tick already armed. It is never
                // rescheduled by a later frame -- it fires on the cadence
                // carrying whatever the newest size is by then -- so a
                // continuous drag keeps the PTY reflowing at ~20Hz instead of
                // waiting for the mouse to stop.
                ResizePlan::Fold => {
                    self.pending_resizes.insert(session.id.clone(), size);
                    return;
                }
                ResizePlan::Arm(delay) => delay,
            };
            self.pending_resizes.insert(session.id.clone(), size);
            self.resize_flush_armed = true;
            let timer = cx.background_executor().timer(delay);
            self.resize_flush = Some(cx.spawn(async move |this, cx| {
                timer.await;
                let _ = this.update(cx, |this, _cx| {
                    this.resize_flush_armed = false;
                    this.last_resize_sent = Some(Instant::now());
                    let pending = std::mem::take(&mut this.pending_resizes);
                    for (id, size) in pending {
                        if let Some(resident) = this.residents.get(&id) {
                            resident.attachment.resize(size.0, size.1);
                        }
                    }
                });
            }));
        }
    }

    /// Keeps the macOS window buttons clear when the card owns the window's
    /// leading edge. The visible lights need more breathing room than their
    /// native frames imply, so this is an intentional optical safe area.
    fn render_traffic_light_lane(&self) -> AnyElement {
        div()
            // The header supplies its own leading edge inset. Reserve only
            // the balance needed to reach the shared window-space boundary.
            .w(px(
                Metrics::TOOLBAR_TRAFFIC_LIGHT_SAFE_RIGHT - Metrics::TOOLBAR_EDGE_INSET
            ))
            .flex_none()
            .into_any_element()
    }

    fn render_sidebar_reveal_control(
        &self,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let icon = if self.chrome.mirrored {
            "sidebar.right"
        } else {
            "sidebar.left"
        };
        div()
            .id("show-sidebar")
            .debug_selector(|| "show-sidebar".into())
            .size(px(Metrics::TOOLBAR_CONTROL_SIZE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(Radius::BADGE))
            .cursor_pointer()
            .hover(move |button| button.bg(Fill::subtle(colors)))
            .child(sf_symbol(icon, 15.0, colors.secondary))
            .on_click(cx.listener(|_, _, _, cx| {
                cx.emit(TerminalPaneEvent::ToggleSidebar);
                cx.stop_propagation();
            }))
            .into_any_element()
    }

    fn render_inspector_reveal_control(
        &self,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let icon = if self.chrome.mirrored {
            "sidebar.left"
        } else {
            "sidebar.right"
        };
        div()
            .id("toggle-inspector")
            .debug_selector(|| "TERMINAL_INSPECTOR_TOGGLE".into())
            .size(px(Metrics::TOOLBAR_CONTROL_SIZE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(Radius::BADGE))
            .cursor_pointer()
            .hover(move |button| button.bg(Fill::subtle(colors)))
            .child(sf_symbol(icon, 15.0, colors.secondary))
            .on_click(cx.listener(|_, _, _, cx| {
                cx.emit(TerminalPaneEvent::ToggleInspector);
                cx.stop_propagation();
            }))
            .into_any_element()
    }

    fn render_header(
        &self,
        session: &SessionRecord,
        chips: &[PaneChip],
        visible_chip_count: usize,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let glyph = self.glyphs.get(&session.id).cloned();
        let branch = session.git_branch.clone();
        let host = session.host.as_ref().map(|host| {
            self.runtime
                .store
                .read()
                .expect("session store lock poisoned")
                .host_display_name(host)
        });
        let kind = ui_agent_kind(session.effective_kind());
        let shell_controls = matches!(self.session_source, SessionSource::FollowSelection);
        let mirrored = self.chrome.mirrored;
        let show_sidebar = shell_controls && !self.chrome.sidebar_visible;
        // The reveal button belongs beside the edge the sidebar returns to.
        let leading_reveal =
            (show_sidebar && !mirrored).then(|| self.render_sidebar_reveal_control(colors, cx));
        let trailing_reveal =
            (show_sidebar && mirrored).then(|| self.render_sidebar_reveal_control(colors, cx));
        let traffic_light_lane = (shell_controls && self.chrome.traffic_light_lane)
            .then(|| self.render_traffic_light_lane());
        let inspector_open = self.chrome.inspector_open;
        // Keep the reveal control on the edge where the inspector returns:
        // leading for the mirrored left panel, trailing for the right panel.
        // While the inspector is open its own header owns this control.
        let leading_inspector = (shell_controls && !inspector_open && mirrored)
            .then(|| self.render_inspector_reveal_control(colors, cx));
        let trailing_inspector = (shell_controls && !inspector_open && !mirrored)
            .then(|| self.render_inspector_reveal_control(colors, cx));
        let visible_chip_count = visible_chip_count.min(chips.len());
        let overflow_count = chips.len().saturating_sub(visible_chip_count);
        let mut toolbar_links = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(Metrics::TOOLBAR_COMPACT_GAP));
        for chip in chips.iter().take(visible_chip_count).cloned() {
            toolbar_links = toolbar_links.child(self.render_chip(chip, colors, cx));
        }
        if overflow_count > 0 {
            toolbar_links = toolbar_links.child(
                div()
                    .id("terminal-chip-overflow")
                    .h(px(Metrics::TOOLBAR_CHIP_HEIGHT))
                    .px(px(6.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(Metrics::TOOLBAR_COMPACT_GAP))
                    .rounded(px(Radius::CHIP))
                    .bg(Fill::subtle(colors))
                    .text_size(px(Typo::META.size))
                    .text_color(colors.secondary)
                    .cursor_pointer()
                    .hover(move |button| button.bg(colors.primary.alpha(0.10)))
                    .child(sf_symbol("ellipsis", 10.0, colors.secondary))
                    .child(format!("+{overflow_count}"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.overflow_open = !this.overflow_open;
                        this.open_checks_for = None;
                        cx.notify();
                        cx.stop_propagation();
                    })),
            );
        }
        div()
            .h(px(Metrics::TITLE_BAR))
            .flex_none()
            .px(px(Metrics::TOOLBAR_EDGE_INSET))
            .flex()
            .items_center()
            .justify_between()
            .bg(colors.sidebar_surface())
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(Metrics::TOOLBAR_ITEM_GAP))
                    .overflow_hidden()
                    .when_some(traffic_light_lane, |title, lane| title.child(lane))
                    .when_some(leading_inspector, |title, control| title.child(control))
                    .when_some(leading_reveal, |title, control| title.child(control))
                    .child(sf_symbol("terminal", 15.0, colors.secondary))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(Typo::TITLE.size))
                            .font_weight(Typo::TITLE.weight)
                            .text_color(colors.primary)
                            .child(session.title.clone()),
                    )
                    .when_some(branch, |title, branch| {
                        title.child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(Metrics::TOOLBAR_COMPACT_GAP))
                                .px(px(5.0))
                                .py(px(2.0))
                                .rounded(px(Radius::CHIP))
                                .bg(Fill::subtle(colors))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(Typo::META.size))
                                .text_color(colors.tertiary)
                                .child(sf_symbol("arrow.branch", 10.5, colors.tertiary))
                                .child(branch),
                        )
                    })
                    .when_some(host, |title, host| {
                        // Remote-host chip: the agent runs on that configured machine.
                        title.child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(Metrics::TOOLBAR_COMPACT_GAP))
                                .rounded(px(Radius::CHIP))
                                .px(px(5.0))
                                .py(px(2.0))
                                .bg(Fill::subtle(colors))
                                .text_size(px(Typo::META.size))
                                .text_color(colors.secondary)
                                .child(sf_symbol("network", 9.0, colors.secondary))
                                .child(host),
                        )
                    })
                    .when(!chips.is_empty(), |title| title.child(toolbar_links)),
            )
            .child(
                div()
                    .flex_none()
                    .pl(px(Metrics::TOOLBAR_EDGE_INSET))
                    .flex()
                    .items_center()
                    .gap(px(Metrics::TOOLBAR_ITEM_GAP))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(Metrics::TOOLBAR_COMPACT_GAP))
                            .when_some(glyph, |identity, glyph| identity.child(glyph))
                            .child(
                                div()
                                    .text_size(px(Typo::META.size))
                                    .text_color(colors.tertiary)
                                    .child(kind.label()),
                            ),
                    )
                    .when_some(trailing_inspector, |trailing, control| {
                        trailing.child(control)
                    })
                    .when_some(trailing_reveal, |trailing, control| trailing.child(control)),
            )
            .into_any_element()
    }

    fn handle_lineage_tree_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "escape" | "enter" => {
                self.set_lineage_view(LineageView::Tabs);
                self.focus(window, cx);
                cx.notify();
            }
            "up" | "down" => {
                let Some(strip) = self.lineage_strip() else {
                    return;
                };
                let Some(selected) = self.selected_id() else {
                    return;
                };
                let Some(index) = strip
                    .nodes
                    .iter()
                    .position(|node| node.session.id == selected)
                else {
                    return;
                };
                let next = if event.keystroke.key == "up" {
                    index.saturating_sub(1)
                } else {
                    (index + 1).min(strip.nodes.len().saturating_sub(1))
                };
                if let Some(node) = strip.nodes.get(next) {
                    self.runtime
                        .store
                        .write()
                        .expect("session store lock poisoned")
                        .select(node.session.id.clone());
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn render_lineage_chrome(
        &self,
        selected: &SessionRecord,
        strip: &LineageStrip,
        view: LineageView,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut bar = div()
            .id("lineage-strip")
            .debug_selector(|| {
                if view == LineageView::Tree {
                    "AGENT_FAMILY_CHROME".into()
                } else {
                    "AGENT_TABS".into()
                }
            })
            .h(px(Metrics::LINEAGE_STRIP))
            .flex_none()
            .px(px(Metrics::TOOLBAR_EDGE_INSET))
            .flex()
            .items_center()
            .gap(px(8.0))
            .overflow_x_scroll()
            .bg(colors.sidebar_surface())
            .border_b_1()
            .border_color(colors.primary.alpha(0.06))
            .child(self.render_lineage_mode_switch(view, colors, cx));
        if view == LineageView::Tabs {
            for node in &strip.nodes {
                bar = bar.child(self.render_lineage_tab(
                    &node.session,
                    selected.id == node.session.id,
                    node.depth == 0,
                    colors,
                    cx,
                ));
            }
        } else {
            bar = bar.child(lineage_family_summary(strip, colors));
        }
        bar.into_any_element()
    }

    fn render_lineage_mode_switch(
        &self,
        current: LineageView,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex_none()
            .h(px(LINEAGE_TAB_HEIGHT))
            .px(px(2.0))
            .flex()
            .items_center()
            .gap(px(1.0))
            .rounded(px(Radius::BADGE))
            .bg(colors.primary.alpha(0.05))
            .border_1()
            .border_color(colors.primary.alpha(0.08))
            .child(self.lineage_mode_button(
                LineageView::Tabs,
                current,
                "list.bullet",
                "Tabs",
                colors,
                cx,
            ))
            .child(self.lineage_mode_button(
                LineageView::Tree,
                current,
                "arrow.triangle.branch",
                "Tree",
                colors,
                cx,
            ))
            .into_any_element()
    }

    fn lineage_mode_button(
        &self,
        mode: LineageView,
        current: LineageView,
        icon: &'static str,
        label: &'static str,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = mode == current;
        let selector = format!("LINEAGE_MODE_{}", label.to_ascii_uppercase());
        div()
            .id(SharedString::from(format!("lineage-mode-{label}")))
            .debug_selector({
                let selector = selector.clone();
                move || selector.clone()
            })
            .h(px(22.0))
            .px(px(7.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .rounded(px(Radius::CHIP))
            .bg(colors.primary.alpha(if active { 0.12 } else { 0.0 }))
            .cursor_pointer()
            .hover(move |button| button.bg(colors.primary.alpha(if active { 0.16 } else { 0.06 })))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_lineage_view(mode);
                if mode == LineageView::Tabs {
                    this.focus(window, cx);
                }
                cx.notify();
                cx.stop_propagation();
            }))
            .child(sf_symbol(
                icon,
                10.0,
                if active {
                    colors.primary
                } else {
                    colors.secondary
                },
            ))
            .child(
                div()
                    .text_size(px(10.5))
                    .font_weight(if active {
                        Typo::SECTION_HEADER.weight
                    } else {
                        Typo::META.weight
                    })
                    .text_color(if active {
                        colors.primary
                    } else {
                        colors.secondary
                    })
                    .child(label),
            )
            .into_any_element()
    }

    fn render_lineage_tab(
        &self,
        session: &SessionRecord,
        selected: bool,
        is_root: bool,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = session.id.clone();
        let glyph = self.glyphs.get(&session.id).cloned();
        let label = lineage_tab_label(session);
        let tint = Ink::working(ui_agent_kind(session.effective_kind()), colors);
        let background = if selected {
            tint.alpha(0.13)
        } else {
            colors.primary.alpha(0.03)
        };
        let border = if selected {
            tint.alpha(0.42)
        } else {
            colors.primary.alpha(0.08)
        };
        let hover = tint.alpha(0.09);
        div()
            .id(SharedString::from(format!("lineage-tab:{}", id.0)))
            .debug_selector({
                let key = id.0.clone();
                move || format!("LINEAGE_TAB_{key}")
            })
            .relative()
            .h(px(LINEAGE_TAB_HEIGHT))
            .max_w(px(200.0))
            .flex_none()
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(5.0))
            .rounded(px(Radius::BADGE))
            .bg(background)
            .border_1()
            .border_color(border)
            .hover(move |tab| tab.bg(hover))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.runtime
                    .store
                    .write()
                    .expect("session store lock poisoned")
                    .select(id.clone());
                this.focus(window, cx);
                cx.notify();
                cx.stop_propagation();
            }))
            .when_some(glyph, |tab, glyph| {
                tab.child(div().size(px(15.0)).flex_none().child(glyph))
            })
            .child(
                div()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(Typo::META.size))
                    .font_weight(if selected {
                        Typo::ROW_EMPHASIZED.weight
                    } else {
                        Typo::META.weight
                    })
                    .text_color(if selected {
                        colors.primary
                    } else {
                        colors.secondary
                    })
                    .child(label),
            )
            .when(is_root, |tab| {
                tab.child(
                    div()
                        .flex_none()
                        .px(px(4.0))
                        .py(px(1.0))
                        .rounded(px(Radius::CHIP))
                        .bg(tint.alpha(0.10))
                        .text_size(px(8.5))
                        .font_weight(Typo::SECTION_HEADER.weight)
                        .text_color(tint)
                        .child("Lead"),
                )
            })
            .when(selected, |tab| {
                tab.child(
                    div()
                        .absolute()
                        .left(px(8.0))
                        .right(px(8.0))
                        .bottom(px(1.0))
                        .h(px(1.5))
                        .rounded_full()
                        .bg(tint),
                )
            })
            .into_any_element()
    }

    fn render_lineage_tree(
        &self,
        selected: &SessionRecord,
        strip: LineageStrip,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = lineage_selected_path(&strip, &selected.id);
        let layout = layout_lineage_tree(&strip);
        let pane_width = self
            .viewport
            .map_or(layout.width, |viewport| viewport.width);
        let content_width = layout.width.max(pane_width);
        let origin_x = ((content_width - layout.width) / 2.0).max(0.0);
        let path_tint = Ink::working(ui_agent_kind(selected.effective_kind()), colors);
        let mut board = div()
            .id("lineage-tree-board")
            .relative()
            .w(px(content_width))
            .h(px(layout.height))
            .child(lineage_tree_edges(
                &layout, origin_x, &path, path_tint, colors,
            ));
        for (index, placed) in layout.nodes.iter().enumerate() {
            let Some(node) = strip.nodes.get(index) else {
                continue;
            };
            let mut placed = placed.clone();
            placed.x += origin_x;
            placed.cx += origin_x;
            board = board.child(self.render_lineage_tree_node(
                node,
                &placed,
                selected.id == node.session.id,
                path.contains(&node.session.id),
                colors,
                cx,
            ));
        }
        div()
            .id("lineage-tree")
            .debug_selector(|| "AGENT_TREE".into())
            .relative()
            .min_h(px(0.0))
            .flex_1()
            .flex()
            .flex_col()
            .rounded_tl(px(Radius::CARD))
            .rounded_tr(px(Radius::CARD))
            .overflow_hidden()
            .bg(colors.background)
            .child(
                div()
                    .id("lineage-tree-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .track_scroll(&self.lineage_tree_scroll)
                    .overflow_scroll()
                    .child(board),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(14.0))
                    .py(px(8.0))
                    .border_t_1()
                    .border_color(colors.primary.alpha(0.06))
                    .text_size(px(Typo::META.size))
                    .text_color(colors.tertiary)
                    .child(
                        "↑↓ to move · Enter or double-click to open the terminal · Esc for tabs",
                    ),
            )
            .into_any_element()
    }

    fn render_lineage_tree_node(
        &self,
        node: &LineageNode,
        placed: &LineageTreePlacement,
        selected: bool,
        on_path: bool,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let session = node.session.as_ref();
        let id = session.id.clone();
        let kind = ui_agent_kind(session.effective_kind());
        let tint = Ink::working(kind, colors);
        let state = status_state(session);
        let working = matches!(state, StatusState::Working);
        let dimmed = matches!(state, StatusState::None | StatusState::Hibernated);
        let status = lineage_status_label(session);
        let status_color = lineage_status_color(session, colors);
        let label_color = if selected {
            colors.primary
        } else if on_path {
            colors.primary.alpha(0.82)
        } else {
            colors.secondary
        };
        let ring = if selected {
            tint.alpha(0.55)
        } else {
            colors.primary.alpha(0.0)
        };
        let hover_ring = tint.alpha(0.28);
        let spin_id = SharedString::from(format!("lineage-spin:{}", id.0));
        div()
            .id(SharedString::from(format!("lineage-node:{}", id.0)))
            .debug_selector({
                let key = id.0.clone();
                move || format!("LINEAGE_NODE_{key}")
            })
            .absolute()
            .left(px(placed.x))
            .top(px(placed.y))
            .w(px(LINEAGE_TREE_NODE_WIDTH))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(LINEAGE_TREE_CAPTION_GAP))
            .cursor_pointer()
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.runtime
                    .store
                    .write()
                    .expect("session store lock poisoned")
                    .select(id.clone());
                if event.click_count() >= 2 {
                    this.set_lineage_view(LineageView::Tabs);
                    this.focus(window, cx);
                }
                cx.notify();
                cx.stop_propagation();
            }))
            .child(
                div()
                    .relative()
                    .size(px(LINEAGE_TREE_MARK))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(if dimmed { 0.42 } else { 1.0 })
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded_full()
                            .border_1()
                            .border_color(ring)
                            .hover(move |orbit| orbit.border_color(hover_ring)),
                    )
                    .when(working, |mark| {
                        mark.child(div().absolute().inset_0().child(WorkingOrbit::new(
                            spin_id,
                            LINEAGE_TREE_MARK,
                            tint,
                        )))
                    })
                    .child(AgentLogo::new(kind, LINEAGE_TREE_ICON, colors).badged(false)),
            )
            .child(
                div()
                    .w_full()
                    .h(px(LINEAGE_TREE_CAPTION_HEIGHT))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(1.0))
                    .px(px(8.0))
                    .rounded(px(Radius::BADGE))
                    .bg(colors.floating_surface())
                    .border_1()
                    .border_color(if selected {
                        tint.alpha(0.48)
                    } else if on_path {
                        tint.alpha(0.22)
                    } else {
                        colors.primary.alpha(0.10)
                    })
                    .child(
                        div()
                            .w_full()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_center()
                            .text_size(px(Typo::META.size))
                            .font_weight(Typo::TITLE.weight)
                            .text_color(label_color)
                            .child(lineage_tab_label(session)),
                    )
                    .child(
                        div()
                            .w_full()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_center()
                            .text_size(px(10.0))
                            .font_weight(Typo::SECTION_HEADER.weight)
                            .text_color(status_color)
                            .child(if node.depth == 0 {
                                format!("Lead · {status}")
                            } else {
                                status.to_owned()
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_chip(
        &self,
        chip: PaneChip,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tint = chip.tint.map(chip_tint_color);
        let background = tint.map_or_else(|| Fill::subtle(colors), |color| color.alpha(0.13));
        let hover_background =
            tint.map_or_else(|| colors.primary.alpha(0.10), |color| color.alpha(0.20));
        let activation = chip.clone();
        div()
            .id(SharedString::from(chip.id.clone()))
            .h(px(Metrics::TOOLBAR_CHIP_HEIGHT))
            .max_w(px(TOOLBAR_LINK_MAX_WIDTH))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(Metrics::TOOLBAR_COMPACT_GAP))
            .rounded(px(Radius::CHIP))
            .px(px(6.0))
            .bg(background)
            .hover(move |style| style.bg(hover_background))
            .cursor_pointer()
            .text_size(px(Typo::META.size))
            .text_color(colors.secondary)
            .child(sf_symbol(
                chip.system_image,
                10.0,
                tint.unwrap_or(colors.secondary),
            ))
            .child(
                div()
                    .min_w(px(0.0))
                    .max_w(px(138.0))
                    .truncate()
                    .child(chip.label),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.modifiers().alt {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        activation.copy_string.clone(),
                    ));
                } else if activation.checks.is_some() {
                    this.open_checks_for = if this.open_checks_for.as_ref() == Some(&activation.id)
                    {
                        None
                    } else {
                        Some(activation.id.clone())
                    };
                    this.overflow_open = false;
                    cx.notify();
                } else if let Some(url) = activation.open_url.as_deref() {
                    cx.open_url(url);
                }
            }))
            .into_any_element()
    }

    fn render_grid_and_overlays(
        &mut self,
        session: &SessionRecord,
        theme: TermTheme,
        font_size: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if session.is_archived() {
            return self.render_archived_overlay(session, cx);
        }
        let exited = matches!(session.status, SessionStatus::Exited(_));
        // An exited agent leaves its last screen behind in the daemon, and that
        // output is exactly what people want to read after closing an agent --
        // so only take the pane over when there is no terminal left to show.
        if exited && let Some(takeover) = self.render_exited_takeover(session, cx) {
            return takeover;
        }
        let Some(resident) = self.residents.get(&session.id) else {
            return centered_message("Preparing terminal…", "").into_any_element();
        };
        let element = resident
            .element
            .clone()
            .theme(theme)
            .font_size(px(font_size))
            .focus_handle(self.focus.clone());
        let view_offset = resident.element.view_offset();
        let attachment_state = resident.attachment_state;
        let overflow = self.grid_row_overflow(resident.element.grid_rows(), font_size, window);

        let id_for_focus = session.id.clone();
        let follows_selection = matches!(self.session_source, SessionSource::FollowSelection);
        let mut body = div()
            .relative()
            .flex_1()
            .overflow_hidden()
            .pt(px(2.0))
            .pb(px(10.0))
            .px(px(12.0))
            .bg(theme.background)
            .track_focus(&self.focus)
            .can_drop(|value, _, _| value.downcast_ref::<ExternalPaths>().is_some())
            .drag_over::<ExternalPaths>(|style, _, _, _| {
                style
                    .border_1()
                    .border_dashed()
                    .border_color(rgba(0x6aa6ffff))
                    .bg(rgba(0x6aa6ff14))
            })
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                    if let Some(paths) = event.dragged_item().downcast_ref::<ExternalPaths>() {
                        this.preview_external_paths(paths, cx);
                    }
                }),
            )
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.handle_external_paths(paths, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    if follows_selection {
                        this.runtime
                            .store
                            .write()
                            .expect("session store lock poisoned")
                            .select(id_for_focus.clone());
                    }
                    let Some(id) = this.selected_id() else {
                        return;
                    };
                    let Some((col, row)) = this.grid_cell_at(event.position, window) else {
                        return;
                    };
                    if event.modifiers.platform {
                        let reference = this
                            .residents
                            .get(&id)
                            .and_then(|resident| resident.element.reference_at(col, row));
                        if let Some(reference) = reference {
                            if let Some(resident) = this.residents.get_mut(&id) {
                                resident.suppress_left_report =
                                    resident.mouse_modes.reporting && !event.modifiers.shift;
                                resident.reported_button_down = None;
                                resident.last_reported_mouse = None;
                            }
                            match reference {
                                TerminalReference::Url(url) => cx.open_url(&url),
                                TerminalReference::File(reference) => {
                                    let Some(session) = this.selected_session() else {
                                        return;
                                    };
                                    cx.emit(TerminalPaneEvent::OpenFileReference {
                                        reference,
                                        cwd: session.cwd.clone(),
                                        session_id: session.id.clone(),
                                    });
                                }
                            }
                            cx.stop_propagation();
                            return;
                        }
                    }
                    let Some(resident) = this.residents.get_mut(&id) else {
                        return;
                    };
                    if resident.mouse_modes.reporting && !event.modifiers.shift {
                        resident.suppress_left_report = false;
                        resident.reported_button_down = Some(TermMouseButton::Left);
                        resident.last_reported_mouse =
                            Some((col, row, Some(TermMouseButton::Left)));
                        if let Some(bytes) = press_report(
                            col,
                            row,
                            TermMouseButton::Left,
                            terminal_mouse_modifiers(event.modifiers),
                            resident.mouse_modes,
                        ) {
                            resident.attachment.input(bytes);
                        }
                        cx.stop_propagation();
                        return;
                    }
                    resident.suppress_left_report = false;
                    resident.reported_button_down = None;
                    resident.last_reported_mouse = None;
                    // Shift is the same local-selection escape hatch Zed uses
                    // while a full-screen program owns ordinary mouse input.
                    if event.modifiers.shift && event.click_count == 1 {
                        resident.element.extend_selection(col, row);
                    } else {
                        match event.click_count {
                            1 => resident.element.begin_selection(col, row),
                            2 => resident.element.select_word(col, row),
                            3 => resident.element.select_line(row),
                            _ => {}
                        }
                    }
                    // notify, never window.refresh(): refresh() flags the
                    // whole frame as caching-disabled, repainting every cached
                    // view at pointer-event rate.
                    cx.notify();
                }),
            )
            .on_mouse_move(
                cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                    let Some(id) = this.selected_id() else {
                        return;
                    };
                    let Some((col, row)) = this.grid_cell_at(event.position, window) else {
                        return;
                    };
                    let Some(resident) = this.residents.get_mut(&id) else {
                        return;
                    };
                    if resident.mouse_modes.reporting
                        && (!event.modifiers.shift || resident.reported_button_down.is_some())
                    {
                        let pressed_button = resident
                            .reported_button_down
                            .or_else(|| event.pressed_button.and_then(terminal_mouse_button));
                        if resident.suppress_left_report
                            && pressed_button == Some(TermMouseButton::Left)
                        {
                            cx.stop_propagation();
                            return;
                        }
                        let position = (col, row, pressed_button);
                        if resident.last_reported_mouse == Some(position) {
                            return;
                        }
                        resident.last_reported_mouse = Some(position);
                        if let Some(bytes) = motion_report(
                            col,
                            row,
                            pressed_button,
                            terminal_mouse_modifiers(event.modifiers),
                            resident.mouse_modes,
                        ) {
                            resident.attachment.input(bytes);
                            cx.stop_propagation();
                        }
                        return;
                    }
                    if event.pressed_button != Some(MouseButton::Left) {
                        return;
                    }
                    resident.element.drag_selection(col, row);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseUpEvent, window, cx| {
                    let Some(id) = this.selected_id() else {
                        return;
                    };
                    let Some((col, row)) = this.grid_cell_at(event.position, window) else {
                        return;
                    };
                    let Some(resident) = this.residents.get_mut(&id) else {
                        return;
                    };
                    if resident.mouse_modes.reporting
                        && (!event.modifiers.shift
                            || resident.reported_button_down == Some(TermMouseButton::Left))
                    {
                        resident.last_reported_mouse = None;
                        if std::mem::take(&mut resident.suppress_left_report) {
                            resident.reported_button_down = None;
                            cx.stop_propagation();
                            return;
                        }
                        if let Some(bytes) = release_report(
                            col,
                            row,
                            TermMouseButton::Left,
                            terminal_mouse_modifiers(event.modifiers),
                            resident.mouse_modes,
                        ) {
                            resident.attachment.input(bytes);
                        }
                        resident.reported_button_down = None;
                        cx.stop_propagation();
                        return;
                    }
                    resident.suppress_left_report = false;
                    resident.reported_button_down = None;
                    resident.last_reported_mouse = None;
                    let copy_on_select = this
                        .runtime
                        .store
                        .read()
                        .expect("session store lock poisoned")
                        .preferences()
                        .terminal_copy_on_select;
                    if copy_on_select {
                        let text = resident.element.selected_text();
                        if !text.is_empty() {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        }
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    this.report_mouse_button(
                        event.position,
                        event.modifiers,
                        TermMouseButton::Middle,
                        true,
                        window,
                        cx,
                    );
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, event: &gpui::MouseUpEvent, window, cx| {
                    this.report_mouse_button(
                        event.position,
                        event.modifiers,
                        TermMouseButton::Middle,
                        false,
                        window,
                        cx,
                    );
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    this.report_mouse_button(
                        event.position,
                        event.modifiers,
                        TermMouseButton::Right,
                        true,
                        window,
                        cx,
                    );
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, event: &gpui::MouseUpEvent, window, cx| {
                    this.report_mouse_button(
                        event.position,
                        event.modifiers,
                        TermMouseButton::Right,
                        false,
                        window,
                        cx,
                    );
                }),
            )
            .on_scroll_wheel(cx.listener(Self::handle_scroll))
            .child(match overflow {
                // Settled: the mirrored screen fits, so the grid fills the pane
                // exactly as before.
                None => div().size_full().child(element),
                // The daemon's screen is still taller than the pane -- a shrink
                // that has not round-tripped yet. Give the grid its natural
                // height, bottom-anchored: the extra rows clip off the top, the
                // way a terminal drops scrollback, instead of the prompt and the
                // agent's input box vanishing off the bottom until the reflow
                // lands. Collapses back to the branch above on the next frame.
                Some(grid_height) => div().size_full().relative().overflow_hidden().child(
                    div()
                        .absolute()
                        .bottom(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .h(px(grid_height))
                        .child(element),
                ),
            });

        // The exit pill owns the bottom slot; the transient pills stack above it.
        let pill_bottom = if exited { 52.0 } else { 18.0 };
        if view_offset > 0 {
            let return_id = session.id.clone();
            body = body.child(
                div()
                    .id("scrolled-pill")
                    .absolute()
                    .bottom(px(pill_bottom))
                    .left_1_2()
                    .ml(px(-90.0))
                    .rounded(px(999.0))
                    .px(px(12.0))
                    .py(px(6.0))
                    .bg(rgba(0x303238e8))
                    .text_size(px(11.5))
                    .text_color(rgba(0xffffff99))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .child(sf_symbol("arrow.down", 11.5, rgba(0xffffff99)))
                    .child(format!("{view_offset} lines · Return to live"))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        if let Some(resident) = this.residents.get_mut(&return_id) {
                            resident
                                .element
                                .scroll_to_live(usize::from(resident.last_size.1));
                            cx.notify();
                        }
                    })),
            );
        }
        if attachment_state != AttachmentState::Live {
            let message = match attachment_state {
                AttachmentState::Attaching => "Attaching…",
                AttachmentState::Reconnecting => "Reconnecting terminal…",
                AttachmentState::Live => "",
            };
            body = body.child(
                div()
                    .absolute()
                    .bottom(px(pill_bottom))
                    .left_1_2()
                    .ml(px(-72.0))
                    .rounded(px(999.0))
                    .px(px(12.0))
                    .py(px(6.0))
                    .bg(rgba(0x303238e8))
                    .text_size(px(11.5))
                    .text_color(rgba(0xffffff99))
                    .child(message),
            );
        }
        if exited {
            body = body.child(self.render_exit_pill(session, cx));
        }
        if let Some(banner) = self.render_attachment_banner(cx) {
            body = body.child(banner);
        }
        body.into_any_element()
    }

    /// Slim status pill over an exited session's last screen: says what happened
    /// and offers the resume that the pane-filling card used to.
    fn render_exit_pill(&self, session: &SessionRecord, cx: &mut Context<Self>) -> AnyElement {
        let id = session.id.clone();
        let resumable = session.resumability == Resumability::Resumable;
        let mut pill = div()
            .id("exit-pill")
            .rounded(px(999.0))
            .pl(px(12.0))
            .pr(if resumable { px(4.0) } else { px(12.0) })
            .py(px(4.0))
            .bg(rgba(0x303238e8))
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_size(px(11.5))
            .text_color(rgba(0xffffff99))
            .child(sf_symbol("power", 11.0, rgba(0xffffff66)))
            .child(exit_description(session));
        if resumable {
            pill = pill.child(
                div()
                    .id("exit-pill-resume")
                    .rounded(px(999.0))
                    .px(px(9.0))
                    .py(px(3.0))
                    .bg(rgba(0xffffff1a))
                    .hover(|style| style.bg(rgba(0xffffff2e)))
                    .cursor_pointer()
                    .text_color(rgba(0xffffffe6))
                    .child("Resume")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.runtime
                            .store
                            .read()
                            .expect("session store lock poisoned")
                            .resume(id.clone());
                        cx.notify();
                    })),
            );
        } else if session.resumability == Resumability::TranscriptMissing {
            pill = pill.child(
                div()
                    .text_color(rgba(0xffffff4d))
                    .child("· transcript gone"),
            );
        }
        // Centered by a full-width row rather than a guessed half-width offset,
        // since the description's length varies with the exit reason.
        div()
            .absolute()
            .bottom(px(18.0))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(pill)
            .into_any_element()
    }

    fn render_attachment_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ui = self.attachment_ui.as_ref()?;
        let (message, failed) = match ui {
            AttachmentUi::Hover { message, .. }
            | AttachmentUi::Progress { message }
            | AttachmentUi::Success { message }
            | AttachmentUi::Unsupported { message } => (message.as_str(), false),
            AttachmentUi::Failed { message } => (message.as_str(), true),
        };
        let names = match ui {
            AttachmentUi::Hover { names, .. } => names.clone(),
            _ => Vec::new(),
        };
        let mut banner = div()
            .id("attachment-banner")
            .absolute()
            .top(px(12.0))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .max_w(px(520.0))
                    .rounded(px(10.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .bg(rgba(0x1b1d24f2))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgba(0xffffffe6))
                            .child(message.to_owned()),
                    )
                    .when(!names.is_empty(), |banner| {
                        banner.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgba(0xffffff99))
                                .child(names.join(", ")),
                        )
                    }),
            );
        if failed && self.pending_upload.is_some() {
            banner = banner.child(
                div()
                    .flex()
                    .justify_center()
                    .gap(px(8.0))
                    .mt(px(4.0))
                    .child(attachment_action_button(
                        "attachment-retry",
                        "Retry",
                        cx.listener(|this, _, _, cx| this.retry_pending_upload(cx)),
                    ))
                    .child(attachment_action_button(
                        "attachment-cancel",
                        "Cancel",
                        cx.listener(|this, _, _, cx| this.cancel_pending_upload(cx)),
                    )),
            );
        }
        Some(banner.into_any_element())
    }

    fn render_find_bar(
        &self,
        session: &SessionRecord,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let resident = self.residents.get(&session.id)?;
        let find = resident.find.as_ref()?;
        let count = if find.matches().is_empty() {
            if find.query().is_empty() {
                String::new()
            } else {
                "No matches".to_owned()
            }
        } else {
            format!("{}/{}", find.current_index() + 1, find.matches().len())
        };
        let query = if resident.find_query.is_empty() {
            div().child("Find").into_any_element()
        } else {
            query_label(&resident.find_query)
        };
        let alt_screen = find.is_alt_screen();
        Some(
            div()
                .id("find-bar")
                .absolute()
                .top(px(Metrics::TITLE_BAR + self.lineage_chrome_height() + 6.0))
                .right(px(16.0))
                .w(px(360.0))
                .child(FloatingSurface::new(
                    colors,
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .px(px(10.0))
                        .py(px(7.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .text_size(px(Typo::ROW.size))
                                .text_color(rgba(0xffffffd9))
                                .child(sf_symbol("magnifyingglass", 12.0, rgba(0xffffff66)))
                                .child(div().flex_1().child(query))
                                .child(
                                    div()
                                        .text_size(px(Typo::META.size))
                                        .text_color(rgba(0xffffff4d))
                                        .child(count),
                                )
                                .child(div().w(px(1.0)).h(px(16.0)).bg(rgba(0xffffff1a)))
                                .child(find_icon_button(
                                    "find-previous",
                                    "chevron.up",
                                    cx,
                                    |this, _w, cx| {
                                        this.navigate_find(true, cx);
                                    },
                                ))
                                .child(find_icon_button(
                                    "find-next",
                                    "chevron.down",
                                    cx,
                                    |this, _w, cx| {
                                        this.navigate_find(false, cx);
                                    },
                                ))
                                .child(find_icon_button(
                                    "find-close",
                                    "xmark",
                                    cx,
                                    |this, _w, cx| {
                                        this.close_find_for_selected();
                                        cx.notify();
                                    },
                                )),
                        )
                        .when(alt_screen, |bar| {
                            bar.child(
                                div()
                                    .pl(px(20.0))
                                    .text_size(px(Typo::META.size))
                                    .text_color(rgba(0xffffff4d))
                                    .child("full-screen app — screen only"),
                            )
                        }),
                ))
                .into_any_element(),
        )
    }

    /// The pane-filling card for an exited session, or `None` when the terminal
    /// itself should stay on screen (with [`Self::render_exit_pill`] over it).
    fn render_exited_takeover(
        &self,
        session: &SessionRecord,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (auto_resuming, migrating) = {
            let store = self
                .runtime
                .store
                .read()
                .expect("session store lock poisoned");
            (
                store.auto_resuming().contains(&session.id),
                store.migrating().contains(&session.id),
            )
        };
        // Mid-migration the source agent is briefly down; show the busy state
        // instead of an exit card with a doomed Resume button.
        if migrating {
            return Some(centered_message("◌", "Moving session…").into_any_element());
        }
        if auto_resuming {
            return Some(centered_message("◌", "Resuming conversation…").into_any_element());
        }
        if self
            .residents
            .get(&session.id)
            .is_some_and(|resident| resident.element.has_content())
        {
            return None;
        }
        Some(self.render_exited_card(session, cx))
    }

    fn render_exited_card(&self, session: &SessionRecord, cx: &mut Context<Self>) -> AnyElement {
        let id = session.id.clone();
        let content = centered_message("", &exit_description(session));
        if session.resumability == Resumability::Resumable {
            content
                .child(primary_button(
                    "resume-conversation",
                    "Resume Conversation",
                    cx,
                    move |this, cx| {
                        this.runtime
                            .store
                            .read()
                            .expect("session store lock poisoned")
                            .resume(id.clone());
                        cx.notify();
                    },
                ))
                .into_any_element()
        } else if session.resumability == Resumability::TranscriptMissing {
            content
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgba(0xffffff4d))
                        .child("Transcript is gone — start a fresh session in the same folder."),
                )
                .into_any_element()
        } else {
            content.into_any_element()
        }
    }

    fn render_archived_overlay(
        &self,
        session: &SessionRecord,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = session.id.clone();
        let mut content = centered_symbol_message("archivebox", 30.0, &session.title).child(
            div()
                .text_size(px(13.0))
                .text_color(rgba(0xffffff99))
                .child("Archived"),
        );
        if session.resumability == Resumability::NotResumable {
            content = content.child(
                div()
                    .max_w(px(320.0))
                    .text_size(px(11.5))
                    .text_color(rgba(0xffffff4d))
                    .child(
                        "This session can't resume its conversation; revive restores it as ended.",
                    ),
            );
        }
        content
            .child(primary_button(
                "revive-session",
                "Revive Session",
                cx,
                move |this, cx| {
                    this.runtime
                        .store
                        .write()
                        .expect("session store lock poisoned")
                        .revive_sessions(vec![id.clone()]);
                    this.reconcile_residency();
                    cx.notify();
                },
            ))
            .into_any_element()
    }

    fn render_checks_popover(
        &self,
        session: &SessionRecord,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let chip_id = self.open_checks_for.as_ref()?;
        let chip = PaneChip::for_session(session)
            .into_iter()
            .find(|chip| &chip.id == chip_id)?;
        let pr = chip.checks?;
        let total = pr.checks_passed + pr.checks_failed + pr.checks_pending;
        let headline = if pr.checks_failed > 0 {
            format!("{} of {total} checks failing", pr.checks_failed)
        } else if pr.checks_pending > 0 {
            format!("{} of {total} checks running", pr.checks_pending)
        } else {
            format!("All {total} checks passed")
        };
        let footer = comments_help(&pr);
        let mut rows = div().flex().flex_col().py(px(4.0)).px(px(6.0));
        for (index, check) in sorted_checks(&pr).into_iter().enumerate() {
            let color = match check.result.as_str() {
                "pass" => Ink::FRESH,
                "fail" => Ink::DANGER,
                _ => Ink::ATTENTION,
            };
            let word = match check.result.as_str() {
                "fail" => "failed",
                "pending" => "running",
                _ => "",
            };
            let url = check.url.clone();
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("pr-check-{index}")))
                    .h(px(24.0))
                    .rounded(px(Radius::ROW))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .hover(|style| style.bg(rgba(0xffffff0f)))
                    .when(url.is_some(), |row| row.cursor_pointer())
                    .child(div().size(px(6.0)).rounded(px(3.0)).bg(color))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(Typo::ROW.size))
                            .text_color(rgba(0xffffffd9))
                            .child(check.name),
                    )
                    .child(
                        div()
                            .text_size(px(Typo::META.size))
                            .text_color(color)
                            .child(word),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(url) = url.as_deref() {
                            cx.open_url(url);
                            this.open_checks_for = None;
                            cx.notify();
                        }
                    })),
            );
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .child(div().absolute().inset_0().occlude().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.open_checks_for = None;
                        cx.notify();
                        cx.stop_propagation();
                    }),
                ))
                .child(
                    div()
                        .id("checks-popover")
                        .absolute()
                        .top(px(Metrics::TITLE_BAR + self.lineage_chrome_height() + 4.0))
                        .right(px(112.0))
                        .w(px(300.0))
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.open_checks_for = None;
                            cx.notify();
                        }))
                        .child(FloatingSurface::new(
                            colors,
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .px(px(12.0))
                                        .py(px(8.0))
                                        .text_size(px(Typo::ROW_EMPHASIZED.size))
                                        .font_weight(Typo::ROW_EMPHASIZED.weight)
                                        .text_color(rgba(0xffffffff))
                                        .child(headline),
                                )
                                .child(div().h(px(1.0)).bg(rgba(0xffffff14)))
                                .child(div().max_h(px(246.0)).overflow_hidden().child(rows))
                                .child(div().h(px(1.0)).bg(rgba(0xffffff14)))
                                .child(
                                    div()
                                        .px(px(12.0))
                                        .py(px(7.0))
                                        .text_size(px(Typo::META.size))
                                        .text_color(rgba(0xffffff99))
                                        .child(footer),
                                ),
                        )),
                )
                .into_any_element(),
        )
    }

    fn render_overflow(
        &self,
        session: &SessionRecord,
        visible_chip_count: usize,
        colors: SemanticColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let chips = PaneChip::for_session(session);
        if !self.overflow_open || visible_chip_count >= chips.len() {
            return None;
        }
        let mut list = div().flex().flex_col().p(px(6.0));
        for (index, chip) in chips.into_iter().skip(visible_chip_count).enumerate() {
            let url = chip.open_url.clone();
            let checks = chip.checks.is_some();
            let chip_id = chip.id.clone();
            let tint = chip.tint.map(chip_tint_color);
            list = list.child(
                div()
                    .id(SharedString::from(format!("overflow-chip-{index}")))
                    .h(px(26.0))
                    .rounded(px(Radius::ROW))
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .px(px(8.0))
                    .text_size(px(Typo::ROW.size))
                    .text_color(rgba(0xffffffd9))
                    .hover(|style| style.bg(rgba(0xffffff0f)))
                    .cursor_pointer()
                    .child(sf_symbol(
                        chip.system_image,
                        11.0,
                        tint.unwrap_or(rgba(0xffffff99)),
                    ))
                    .child(div().min_w(px(0.0)).flex_1().truncate().child(chip.label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if checks {
                            this.open_checks_for = Some(chip_id.clone());
                        } else if let Some(url) = url.as_deref() {
                            cx.open_url(url);
                        }
                        this.overflow_open = false;
                        cx.notify();
                    })),
            );
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .child(div().absolute().inset_0().occlude().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.overflow_open = false;
                        cx.notify();
                        cx.stop_propagation();
                    }),
                ))
                .child(
                    div()
                        .absolute()
                        .top(px(Metrics::TITLE_BAR + self.lineage_chrome_height() + 4.0))
                        .right(px(112.0))
                        .w(px(280.0))
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.overflow_open = false;
                            cx.notify();
                        }))
                        .child(FloatingSurface::new(
                            colors,
                            list.id("toolbar-overflow-list")
                                .max_h(px(320.0))
                                .overflow_y_scroll(),
                        )),
                )
                .into_any_element(),
        )
    }
}

impl Render for TerminalPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.reconcile_residency();
        let (theme, colors, sidebar_colors, font_size) = {
            let store = self
                .runtime
                .store
                .read()
                .expect("session store lock poisoned");
            let theme_id = &store.preferences().terminal_theme;
            (
                crate::app_theme::terminal_theme(theme_id),
                crate::app_theme::colors(theme_id),
                crate::app_theme::sidebar_colors(theme_id),
                store.preferences().terminal_font_size,
            )
        };
        self.sync_status_glyphs(colors, window, cx);
        self.update_selected_geometry(window, cx);

        let selected = self.selected_session();

        let content = if let Some(session) = selected {
            let chips = PaneChip::for_session(&session);
            let visible_chip_count = toolbar_visible_chip_count(
                &chips,
                self.viewport.map_or(900.0, |viewport| viewport.width),
                self.chrome.sidebar_visible && !self.chrome.traffic_light_lane,
            );
            if visible_chip_count >= chips.len() {
                self.overflow_open = false;
            }
            let mut pane = div()
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .h_full()
                .overflow_hidden()
                .border_l_1()
                .border_color(sidebar_colors.primary.alpha(0.08))
                .bg(sidebar_colors.sidebar_surface())
                .child(self.render_header(
                    &session,
                    &chips,
                    visible_chip_count,
                    sidebar_colors,
                    cx,
                ));
            let strip = self.lineage_strip();
            let lineage_view = self.lineage_view();
            if let Some(strip) = strip.as_ref() {
                pane = pane.child(self.render_lineage_chrome(
                    &session,
                    strip,
                    lineage_view,
                    sidebar_colors,
                    cx,
                ));
            }
            if lineage_view == LineageView::Tree
                && let Some(strip) = strip
            {
                pane = pane.child(self.render_lineage_tree(&session, strip, sidebar_colors, cx));
            } else {
                let terminal_surface = div()
                    .relative()
                    .min_h(px(0.0))
                    .flex_1()
                    .flex()
                    .flex_col()
                    .rounded_tl(px(Radius::CARD))
                    .rounded_tr(px(Radius::CARD))
                    .overflow_hidden()
                    .bg(theme.background)
                    .child(self.render_grid_and_overlays(&session, theme, font_size, window, cx));
                pane = pane.child(terminal_surface);
                if let Some(find) = self.render_find_bar(&session, colors, cx) {
                    pane = pane.child(find);
                }
            }
            if let Some(popover) = self.render_checks_popover(&session, colors, cx) {
                pane = pane.child(popover);
            }
            if let Some(overflow) = self.render_overflow(&session, visible_chip_count, colors, cx) {
                pane = pane.child(overflow);
            }
            pane.into_any_element()
        } else {
            let follows_selection = matches!(self.session_source, SessionSource::FollowSelection);
            let show_sidebar = follows_selection && !self.chrome.sidebar_visible;
            let sidebar_reveal =
                show_sidebar.then(|| self.render_sidebar_reveal_control(sidebar_colors, cx));
            let welcome_lane = (follows_selection && self.chrome.traffic_light_lane)
                .then(|| self.render_traffic_light_lane());
            // The strip exists for either control, and for the window buttons
            // alone when the empty workbench owns the window's leading edge.
            let empty_bar = (sidebar_reveal.is_some() || welcome_lane.is_some()).then_some((
                sidebar_reveal,
                welcome_lane,
                self.chrome.mirrored,
            ));
            let empty_content = if follows_selection {
                self.render_empty_session(colors)
            } else {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(Typo::ROW.size))
                    .text_color(colors.tertiary)
                    .child("Opening terminal…")
                    .into_any_element()
            };
            div()
                .flex_1()
                .h_full()
                .flex()
                .flex_col()
                .bg(theme.background)
                .when_some(empty_bar, |pane, (reveal, lane, mirrored)| {
                    pane.child(
                        div()
                            .h(px(Metrics::TITLE_BAR))
                            .flex_none()
                            .px(px(Metrics::TOOLBAR_EDGE_INSET))
                            .flex()
                            .items_center()
                            .gap(px(Metrics::TOOLBAR_ITEM_GAP))
                            .justify_between()
                            .bg(sidebar_colors.sidebar_surface())
                            .when_some(lane, |bar, lane| bar.child(lane))
                            .when(mirrored, |bar| bar.child(div().flex_1()))
                            .when_some(reveal, |bar, control| bar.child(control)),
                    )
                })
                .child(empty_content)
                .into_any_element()
        };

        let root_id = match &self.session_source {
            SessionSource::FollowSelection => SharedString::from("zeus-terminal-root"),
            SessionSource::Fixed(id) => SharedString::from(format!("zeus-terminal-root-{}", id.0)),
        };
        div()
            .id(root_id)
            .track_focus(&self.focus)
            .flex()
            .size_full()
            .text_color(colors.primary)
            .on_action(cx.listener(Self::open_find))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_previous))
            .on_action(cx.listener(Self::close_find))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::reset_zoom))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy_selection))
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_key_up(cx.listener(Self::handle_key_up))
            .on_modifiers_changed(cx.listener(Self::handle_modifiers_changed))
            .child(content)
    }
}

fn find_icon_button(
    id: &'static str,
    system_image: &'static str,
    cx: &mut Context<TerminalPane>,
    handler: impl Fn(&mut TerminalPane, &mut Window, &mut Context<TerminalPane>) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .size(px(20.0))
        .rounded(px(4.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.0))
        .text_color(rgba(0xffffff99))
        .hover(|style| style.bg(rgba(0xffffff0f)))
        .cursor_pointer()
        .child(sf_symbol_weighted(
            system_image,
            11.0,
            SymbolWeight::Semibold,
            rgba(0xffffff99),
        ))
        .on_click(cx.listener(move |this, _, window, cx| handler(this, window, cx)))
        .into_any_element()
}

fn primary_button(
    id: &'static str,
    label: &'static str,
    cx: &mut Context<TerminalPane>,
    handler: impl Fn(&mut TerminalPane, &mut Context<TerminalPane>) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .mt(px(2.0))
        .rounded(px(7.0))
        .px(px(14.0))
        .py(px(7.0))
        .bg(rgba(0xffffffeb))
        .text_size(px(13.0))
        .font_weight(Typo::ROW_EMPHASIZED.weight)
        .text_color(rgba(0x121318ff))
        .hover(|style| style.bg(rgba(0xffffffff)))
        .cursor_pointer()
        .child(label)
        .on_click(cx.listener(move |this, _, _, cx| handler(this, cx)))
        .into_any_element()
}

fn centered_message(icon: &str, message: &str) -> gpui::Div {
    div()
        .flex_1()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(12.0))
        .when(!icon.is_empty(), |content| {
            content.child(
                div()
                    .text_size(px(30.0))
                    .text_color(rgba(0xffffff4d))
                    .child(icon.to_owned()),
            )
        })
        .when(!message.is_empty(), |content| {
            content.child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgba(0xffffff99))
                    .child(message.to_owned()),
            )
        })
}

fn centered_symbol_message(system_image: &str, size: f32, message: &str) -> gpui::Div {
    div()
        .flex_1()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(12.0))
        .child(sf_symbol_weighted(
            system_image,
            size,
            SymbolWeight::Regular,
            rgba(0xffffff4d),
        ))
        .when(!message.is_empty(), |content| {
            content.child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgba(0xffffff99))
                    .child(message.to_owned()),
            )
        })
}

fn chip_tint_color(tint: ChipTint) -> gpui::Rgba {
    match tint {
        ChipTint::Red => Ink::DANGER,
        ChipTint::Orange => rgba(0xf59e42ff),
        ChipTint::Yellow => Ink::ATTENTION,
        ChipTint::Green => Ink::FRESH,
        ChipTint::Purple => rgba(0xa879f7ff),
    }
}

fn terminal_input_modes(modes: WireTerminalModes) -> TermInputModes {
    TermInputModes {
        application_cursor_keys: modes.application_cursor_keys,
        bracketed_paste: modes.bracketed_paste,
        alternate_scroll: modes.alternate_scroll,
        focus_reporting: modes.focus_reporting,
        ..TermInputModes::default()
    }
}

fn terminal_mouse_modes(modes: WireTerminalModes) -> MouseModes {
    MouseModes {
        reporting: modes.mouse_reporting,
        sgr: modes.mouse_sgr,
        utf8: modes.mouse_utf8,
        drag: modes.mouse_drag,
        motion: modes.mouse_motion,
    }
}

fn terminal_mouse_button(button: MouseButton) -> Option<TermMouseButton> {
    match button {
        MouseButton::Left => Some(TermMouseButton::Left),
        MouseButton::Middle => Some(TermMouseButton::Middle),
        MouseButton::Right => Some(TermMouseButton::Right),
        MouseButton::Navigate(_) => None,
    }
}

fn terminal_mouse_modifiers(modifiers: gpui::Modifiers) -> MouseModifiers {
    MouseModifiers {
        shift: modifiers.shift,
        alt: modifiers.alt,
        control: modifiers.control,
    }
}

fn terminal_key_event(event: &KeyDownEvent) -> Option<TermKeyEvent> {
    let named = match event.keystroke.key.as_str() {
        "up" => Some(NamedKey::ArrowUp),
        "down" => Some(NamedKey::ArrowDown),
        "right" => Some(NamedKey::ArrowRight),
        "left" => Some(NamedKey::ArrowLeft),
        "home" => Some(NamedKey::Home),
        "end" => Some(NamedKey::End),
        "pageup" => Some(NamedKey::PageUp),
        "pagedown" => Some(NamedKey::PageDown),
        "insert" => Some(NamedKey::Insert),
        "delete" => Some(NamedKey::Delete),
        "tab" => Some(NamedKey::Tab),
        "enter" => Some(NamedKey::Enter),
        "escape" => Some(NamedKey::Escape),
        "backspace" => Some(NamedKey::Backspace),
        "f1" => Some(NamedKey::F1),
        "f2" => Some(NamedKey::F2),
        "f3" => Some(NamedKey::F3),
        "f4" => Some(NamedKey::F4),
        "f5" => Some(NamedKey::F5),
        "f6" => Some(NamedKey::F6),
        "f7" => Some(NamedKey::F7),
        "f8" => Some(NamedKey::F8),
        "f9" => Some(NamedKey::F9),
        "f10" => Some(NamedKey::F10),
        "f11" => Some(NamedKey::F11),
        "f12" => Some(NamedKey::F12),
        "f13" => Some(NamedKey::F13),
        "f14" => Some(NamedKey::F14),
        "f15" => Some(NamedKey::F15),
        "f16" => Some(NamedKey::F16),
        "f17" => Some(NamedKey::F17),
        "f18" => Some(NamedKey::F18),
        "f19" => Some(NamedKey::F19),
        "f20" => Some(NamedKey::F20),
        _ => None,
    };
    if let Some(named) = named {
        return Some(TermKeyEvent::named(named));
    }
    let logical = event.keystroke.key.clone();
    let text = event
        .keystroke
        .key_char
        .clone()
        .unwrap_or_else(|| logical.clone());
    (!logical.is_empty()).then_some(TermKeyEvent {
        key: TermKey::Character(logical),
        text: Some(text),
    })
}

fn spawn_attachment(
    runtime: &Handle,
    socket: std::path::PathBuf,
    id: SessionId,
    pane_tx: mpsc::UnboundedSender<PaneEvent>,
) -> AttachmentControl {
    let (command_tx, mut commands) = mpsc::unbounded_channel();
    let control = AttachmentControl {
        tx: command_tx,
        pane_tx: pane_tx.clone(),
    };
    runtime.spawn(async move {
        // The first resize must be the measured pane geometry: deferred agent
        // launch waits for it. Do not seed an arbitrary 80×24 size.
        let mut last_resize = None;
        loop {
            let _ = pane_tx.send(PaneEvent::AttachmentState(
                id.clone(),
                AttachmentState::Attaching,
            ));
            let mut attachment = match SessionAttachment::connect(&socket, id.clone()).await {
                Ok(attachment) => attachment,
                Err(_) => {
                    let _ = pane_tx.send(PaneEvent::AttachmentState(
                        id.clone(),
                        AttachmentState::Reconnecting,
                    ));
                    if wait_for_retry(&mut commands, &mut last_resize).await {
                        return;
                    }
                    continue;
                }
            };
            let writer = attachment.handle();
            if let Some((cols, rows)) = last_resize {
                let _ = writer.resize(cols, rows);
            }
            let _ = pane_tx.send(PaneEvent::AttachmentState(
                id.clone(),
                AttachmentState::Live,
            ));

            let should_close = loop {
                tokio::select! {
                    chunk = attachment.chunks.recv() => {
                        let Some(chunk) = chunk else { break false };
                        if pane_tx.send(PaneEvent::Chunk(id.clone(), chunk)).is_err() {
                            break true;
                        }
                    }
                    command = commands.recv() => {
                        match command {
                            Some(AttachmentCommand::Input(bytes)) => {
                                let _ = writer.send_input(bytes);
                            }
                            Some(AttachmentCommand::Resize(cols, rows)) => {
                                last_resize = Some((cols, rows));
                                let _ = writer.resize(cols, rows);
                            }
                            Some(AttachmentCommand::Close) | None => break true,
                        }
                    }
                }
            };
            attachment.close().await;
            if should_close {
                return;
            }
            let _ = pane_tx.send(PaneEvent::AttachmentState(
                id.clone(),
                AttachmentState::Reconnecting,
            ));
            if wait_for_retry(&mut commands, &mut last_resize).await {
                return;
            }
        }
    });
    control
}

async fn wait_for_retry(
    commands: &mut mpsc::UnboundedReceiver<AttachmentCommand>,
    last_resize: &mut Option<(u16, u16)>,
) -> bool {
    let delay = tokio::time::sleep(REATTACH_DELAY);
    tokio::pin!(delay);
    loop {
        tokio::select! {
            () = &mut delay => return false,
            command = commands.recv() => match command {
                Some(AttachmentCommand::Resize(cols, rows)) => *last_resize = Some((cols, rows)),
                Some(AttachmentCommand::Close) | None => return true,
                Some(AttachmentCommand::Input(_)) => {}
            }
        }
    }
}

fn lineage_tab_label(session: &SessionRecord) -> String {
    if session.title_source == TitleSource::Placeholder {
        ui_agent_kind(session.effective_kind()).label().to_owned()
    } else {
        display_title(session)
    }
}

fn lineage_status_label(session: &SessionRecord) -> &'static str {
    if session.hibernation.is_some() {
        return "Hibernated";
    }
    match session.status {
        SessionStatus::Starting => "Starting",
        SessionStatus::Working => "Working",
        SessionStatus::NeedsInput(_) => "Needs input",
        SessionStatus::Idle => match session.attention() {
            zeus_proto::AttentionLevel::DoneUnseen => "Done",
            _ => "Idle",
        },
        SessionStatus::Exited(_) => "Ended",
        SessionStatus::Unknown => "Unknown",
    }
}

fn lineage_status_color(session: &SessionRecord, colors: SemanticColors) -> Rgba {
    if session.hibernation.is_some() {
        return colors.tertiary;
    }
    match session.status {
        SessionStatus::Working | SessionStatus::Starting => Ink::FRESH,
        SessionStatus::NeedsInput(_) => Ink::ATTENTION,
        SessionStatus::Exited(_) => colors.tertiary,
        SessionStatus::Idle | SessionStatus::Unknown => colors.secondary,
    }
}

fn lineage_family_summary(strip: &LineageStrip, colors: SemanticColors) -> AnyElement {
    let total = strip.nodes.len();
    let working = strip
        .nodes
        .iter()
        .filter(|node| {
            matches!(
                node.session.status,
                SessionStatus::Working | SessionStatus::Starting
            ) && node.session.hibernation.is_none()
        })
        .count();
    let waiting = strip
        .nodes
        .iter()
        .filter(|node| matches!(node.session.status, SessionStatus::NeedsInput(_)))
        .count();
    let mut parts = vec![format!(
        "{total} agent{}",
        if total == 1 { "" } else { "s" }
    )];
    if working > 0 {
        parts.push(format!("{working} working"));
    }
    if waiting > 0 {
        parts.push(format!("{waiting} waiting"));
    }
    div()
        .min_w(px(0.0))
        .flex_1()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_size(px(Typo::META.size))
        .text_color(colors.secondary)
        .child(parts.join(" · "))
        .into_any_element()
}

fn lineage_selected_path(strip: &LineageStrip, selected: &SessionId) -> HashSet<SessionId> {
    let mut by_id = HashMap::new();
    for node in &strip.nodes {
        by_id.insert(node.session.id.clone(), Arc::clone(&node.session));
    }
    let mut path = HashSet::new();
    let mut current = Some(selected.clone());
    while let Some(id) = current {
        if !path.insert(id.clone()) {
            break;
        }
        current = by_id.get(&id).and_then(|session| session.parent.clone());
    }
    path
}

#[derive(Clone, Debug)]
struct LineageTreePlacement {
    id: SessionId,
    parent: Option<SessionId>,
    x: f32,
    y: f32,
    cx: f32,
}

#[derive(Clone, Debug)]
struct LineageTreeLayout {
    nodes: Vec<LineageTreePlacement>,
    width: f32,
    height: f32,
}

/// Top-down genealogy: each leaf occupies one slot, a parent is centered over
/// its children, and levels share a y. That is the spatial tree, not a list.
fn layout_lineage_tree(strip: &LineageStrip) -> LineageTreeLayout {
    let count = strip.nodes.len();
    let mut index_of = HashMap::with_capacity(count);
    for (index, node) in strip.nodes.iter().enumerate() {
        index_of.insert(node.session.id.clone(), index);
    }
    let mut children = vec![Vec::new(); count];
    let mut root = 0usize;
    for (index, node) in strip.nodes.iter().enumerate() {
        if node.depth == 0 {
            root = index;
        }
        if let Some(parent) = &node.session.parent
            && let Some(&parent_index) = index_of.get(parent)
        {
            children[parent_index].push(index);
        }
    }
    let mut units = vec![1.0f32; count];
    for index in (0..count).rev() {
        if !children[index].is_empty() {
            units[index] = children[index].iter().map(|&child| units[child]).sum();
        }
    }
    let mut starts = vec![0.0f32; count];
    let mut order = vec![root];
    let mut cursor = 0usize;
    while cursor < order.len() {
        let index = order[cursor];
        let mut child_start = starts[index];
        for &child in &children[index] {
            starts[child] = child_start;
            child_start += units[child];
            order.push(child);
        }
        cursor += 1;
    }

    let slot = LINEAGE_TREE_NODE_WIDTH + LINEAGE_TREE_H_GAP;
    let mut nodes = Vec::with_capacity(count);
    for (index, node) in strip.nodes.iter().enumerate() {
        let center = (starts[index] + units[index] / 2.0) * slot;
        let x = center - LINEAGE_TREE_NODE_WIDTH / 2.0;
        let y = f32::from(node.depth) * (LINEAGE_TREE_NODE_HEIGHT + LINEAGE_TREE_V_GAP)
            + LINEAGE_TREE_PAD;
        nodes.push(LineageTreePlacement {
            id: node.session.id.clone(),
            parent: node.session.parent.clone(),
            x,
            y,
            cx: x + LINEAGE_TREE_NODE_WIDTH / 2.0,
        });
    }
    let min_x = nodes
        .iter()
        .map(|node| node.x)
        .fold(f32::INFINITY, f32::min);
    let shift = LINEAGE_TREE_PAD - min_x;
    for node in &mut nodes {
        node.x += shift;
        node.cx += shift;
    }
    let width = nodes
        .iter()
        .map(|node| node.x + LINEAGE_TREE_NODE_WIDTH)
        .fold(0.0f32, f32::max)
        + LINEAGE_TREE_PAD;
    let height = nodes
        .iter()
        .map(|node| node.y + LINEAGE_TREE_NODE_HEIGHT)
        .fold(0.0f32, f32::max)
        + LINEAGE_TREE_PAD;
    LineageTreeLayout {
        nodes,
        width,
        height,
    }
}

fn lineage_tree_edges(
    layout: &LineageTreeLayout,
    origin_x: f32,
    path: &HashSet<SessionId>,
    path_tint: Rgba,
    colors: SemanticColors,
) -> AnyElement {
    let layout = layout.clone();
    let path = path.clone();
    let muted = colors.primary.alpha(0.16);
    canvas(
        move |bounds, _, _| {
            let mut quiet = PathBuilder::stroke(px(1.25));
            let mut active = PathBuilder::stroke(px(1.75));
            let mut has_quiet = false;
            let mut has_active = false;
            let by_id: HashMap<_, _> = layout
                .nodes
                .iter()
                .map(|node| (node.id.clone(), node))
                .collect();
            for child in &layout.nodes {
                let Some(parent_id) = &child.parent else {
                    continue;
                };
                let Some(parent) = by_id.get(parent_id) else {
                    continue;
                };
                let from = point(
                    bounds.origin.x + px(origin_x + parent.cx),
                    bounds.origin.y + px(parent.y + LINEAGE_TREE_NODE_HEIGHT),
                );
                let to = point(
                    bounds.origin.x + px(origin_x + child.cx),
                    bounds.origin.y + px(child.y),
                );
                if path.contains(&parent.id) && path.contains(&child.id) {
                    lineage_tree_elbow(&mut active, from, to);
                    has_active = true;
                } else {
                    lineage_tree_elbow(&mut quiet, from, to);
                    has_quiet = true;
                }
            }
            (
                has_quiet.then(|| quiet.build().ok()).flatten(),
                has_active.then(|| active.build().ok()).flatten(),
            )
        },
        move |_, mut paths, window, _| {
            if let Some(path) = paths.0.take() {
                window.paint_path(path, muted);
            }
            if let Some(path) = paths.1.take() {
                window.paint_path(path, path_tint.alpha(0.62));
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// Down from the parent's caption, across in the row gutter, then down to the
/// child's mark. A straight drop when parent and child share a column.
fn lineage_tree_elbow(
    builder: &mut PathBuilder,
    from: gpui::Point<gpui::Pixels>,
    to: gpui::Point<gpui::Pixels>,
) {
    let fx = f32::from(from.x);
    let fy = f32::from(from.y);
    let tx = f32::from(to.x);
    let ty = f32::from(to.y);
    let dx = tx - fx;
    let dy = ty - fy;
    builder.move_to(from);
    if dx.abs() < 0.5 {
        builder.line_to(to);
        return;
    }
    let mid_y = fy + dy / 2.0;
    let radius = LINEAGE_TREE_ELBOW
        .min(dx.abs() / 2.0)
        .min(dy.abs() / 2.0)
        .max(0.0);
    if radius < 0.5 {
        builder.line_to(point(px(fx), px(mid_y)));
        builder.line_to(point(px(tx), px(mid_y)));
        builder.line_to(to);
        return;
    }
    let sign = if dx > 0.0 { 1.0 } else { -1.0 };
    let kappa = radius * 0.5523;
    builder.line_to(point(px(fx), px(mid_y - radius)));
    builder.cubic_bezier_to(
        point(px(fx + sign * radius), px(mid_y)),
        point(px(fx), px(mid_y - radius + kappa)),
        point(px(fx + sign * (radius - kappa)), px(mid_y)),
    );
    builder.line_to(point(px(tx - sign * radius), px(mid_y)));
    builder.cubic_bezier_to(
        point(px(tx), px(mid_y + radius)),
        point(px(tx - sign * (radius - kappa)), px(mid_y)),
        point(px(tx), px(mid_y + radius - kappa)),
    );
    builder.line_to(to);
}

fn ui_agent_kind(kind: &ProtoAgentKind) -> UiAgentKind {
    UiAgentKind::from_id(kind.id())
}

fn status_state(session: &SessionRecord) -> StatusState {
    if session.hibernation.is_some() {
        return StatusState::Hibernated;
    }
    match session.attention() {
        zeus_proto::AttentionLevel::Working => StatusState::Working,
        zeus_proto::AttentionLevel::NeedsInput => StatusState::NeedsInput {
            destructive: session
                .needs_input
                .as_ref()
                .is_some_and(|detail| detail.risk_hint == RiskHint::Destructive),
        },
        zeus_proto::AttentionLevel::DoneUnseen => StatusState::DoneUnseen,
        zeus_proto::AttentionLevel::IdleSeen => StatusState::IdleSeen,
        zeus_proto::AttentionLevel::None | zeus_proto::AttentionLevel::Unknown => StatusState::None,
    }
}

fn pr_number(url: &str) -> Option<String> {
    let parts: Vec<_> = url.split('/').filter(|part| !part.is_empty()).collect();
    if let Some(index) = parts.iter().position(|part| *part == "pull") {
        return parts
            .get(index + 1)
            .map(|part| part.chars().take_while(char::is_ascii_digit).collect())
            .filter(|part: &String| !part.is_empty());
    }
    parts
        .last()
        .filter(|part| part.chars().all(|character| character.is_ascii_digit()))
        .map(|part| (*part).to_owned())
}

fn linear_key(url: &str) -> Option<String> {
    let parts: Vec<_> = url.split('/').collect();
    let index = parts.iter().position(|part| *part == "issue")?;
    parts.get(index + 1).map(|part| (*part).to_owned())
}

fn url_host(url: &str) -> String {
    url.split_once("://")
        .map_or(url, |(_, remainder)| remainder)
        .split('/')
        .next()
        .unwrap_or(url)
        .split(':')
        .next()
        .unwrap_or(url)
        .to_owned()
}

fn url_port(url: &str) -> Option<u16> {
    let authority = url
        .split_once("://")
        .map_or(url, |(_, remainder)| remainder)
        .split('/')
        .next()?;
    authority.rsplit_once(':')?.1.parse().ok()
}

fn pr_tint(pr: &PullRequestStatus) -> Option<ChipTint> {
    if pr.state == "MERGED" {
        return Some(ChipTint::Purple);
    }
    if pr.state == "CLOSED" || pr.mergeable.as_deref() == Some("CONFLICTING") {
        return Some(ChipTint::Red);
    }
    if pr.is_draft {
        return None;
    }
    match pr.review_decision.as_deref() {
        Some("CHANGES_REQUESTED") => Some(ChipTint::Orange),
        Some("REVIEW_REQUIRED") => Some(ChipTint::Yellow),
        Some("APPROVED") => Some(ChipTint::Green),
        _ => None,
    }
}

fn pr_help(pr: &PullRequestStatus) -> String {
    let overall = if pr.state == "MERGED" {
        "merged"
    } else if pr.state == "CLOSED" {
        "closed"
    } else if pr.is_draft {
        "draft"
    } else {
        "open"
    };
    let title = pr.title.as_deref().map_or_else(
        || overall.to_owned(),
        |title| format!("{title} — {overall}"),
    );
    format!(
        "{title} · +{} −{} · {} file{}",
        pr.additions,
        pr.deletions,
        pr.changed_files,
        if pr.changed_files == 1 { "" } else { "s" }
    )
}

fn comments_help(pr: &PullRequestStatus) -> String {
    let mut parts = Vec::new();
    if let Some(total) = pr.total_threads.filter(|total| *total > 0) {
        parts.push(format!(
            "{} of {total} threads resolved",
            pr.resolved_threads.unwrap_or(0)
        ));
    }
    parts.push(format!(
        "{} comment{}",
        pr.comment_count,
        if pr.comment_count == 1 { "" } else { "s" }
    ));
    parts.push(format!(
        "{} review{}",
        pr.review_count,
        if pr.review_count == 1 { "" } else { "s" }
    ));
    parts.join(" · ")
}

fn sorted_checks(pr: &PullRequestStatus) -> Vec<PrCheck> {
    let mut checks = pr.checks.clone().unwrap_or_default();
    checks.sort_by_key(|check| match check.result.as_str() {
        "fail" => 0,
        "pending" => 1,
        "pass" => 2,
        _ => 3,
    });
    checks
}

fn terminal_damage_should_repaint(
    selected: Option<&SessionId>,
    updated: &SessionId,
    changed: bool,
) -> bool {
    changed && selected == Some(updated)
}

/// What to do with a geometry change that just landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResizePlan {
    /// Push it to the daemon now.
    SendNow,
    /// Hold it and arm a tick to send in this long.
    Arm(Duration),
    /// Hold it; a tick is already armed and will carry it.
    Fold,
}

/// Decides whether a geometry change goes out now or rides the next cadence
/// tick. Pure, and deliberately named: the version this replaced looked correct
/// but rescheduled its timer on every frame, so a smooth drag cancelled its own
/// flush forever and the PTY only ever heard the size the mouse stopped at.
fn plan_resize(first_measure: bool, since_sent: Option<Duration>, armed: bool) -> ResizePlan {
    // The first measure after attach is what a deferred agent launch waits for,
    // and an isolated change (session switch, window snap, the opening frame of
    // a drag) should feel instant -- neither may wait on the cadence.
    if first_measure || since_sent.is_none_or(|since| since >= RESIZE_CADENCE) {
        return ResizePlan::SendNow;
    }
    if armed {
        return ResizePlan::Fold;
    }
    ResizePlan::Arm(RESIZE_CADENCE.saturating_sub(since_sent.unwrap_or_default()))
}

/// Whether a geometry change should hold the grid still while it round-trips.
/// Pure so the three conditions stay stated rather than implied:
///
/// - a first measure has nothing on screen to hold;
/// - only a column change reflows, and it is the reflow that moves content
///   vertically -- a rows-only change crops or extends the grid, which the
///   bottom-anchor path already covers;
/// - a drag steps faster than [`RESIZE_GESTURE_GAP`] and has to keep reflowing
///   under the cursor, so only a discrete change holds.
fn should_hold_reflow(
    previous: (u16, u16),
    next: (u16, u16),
    since_sent: Option<Duration>,
) -> bool {
    previous != (0, 0)
        && previous.0 != next.0
        && since_sent.is_none_or(|since| since >= RESIZE_GESTURE_GAP)
}

/// The current window-space estimate used for PTY sizing. Keeping this
/// calculation named makes the protocol-vs-painted-width invariant directly
/// testable: the daemon must never receive more columns than the grid element
/// can actually paint after layout chrome is applied.
fn estimated_grid_size(
    window_width: f32,
    window_height: f32,
    chrome_inset: f32,
    extra_header: f32,
    metrics: CellMetrics,
) -> (u16, u16) {
    let width = px((window_width
        - chrome_inset
        - GRID_HORIZONTAL_PADDING
        - GRID_LAYOUT_HORIZONTAL_CHROME)
        .max(1.0));
    let height = px((window_height
        - Metrics::TITLE_BAR
        - extra_header
        - GRID_VERTICAL_PADDING
        - GRID_LAYOUT_VERTICAL_CHROME)
        .max(1.0));
    (
        metrics.cols_for_width(width).max(2),
        metrics.rows_for_height(height).max(2),
    )
}

fn attachment_count(names: &[String]) -> String {
    match names.len() {
        0 => "images".to_owned(),
        1 => names[0].clone(),
        n => format!("{n} images"),
    }
}

fn attachment_success_message(names: &[String]) -> String {
    format!("Attached {}", attachment_count(names))
}

fn attachment_action_button(
    id: &'static str,
    label: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .rounded(px(999.0))
        .px(px(10.0))
        .py(px(4.0))
        .bg(rgba(0xffffff1a))
        .hover(|style| style.bg(rgba(0xffffff2e)))
        .cursor_pointer()
        .text_size(px(11.5))
        .text_color(rgba(0xffffffe6))
        .child(label)
        .on_click(on_click)
        .into_any_element()
}

fn clipboard_image(item: &ClipboardItem) -> Option<(&[u8], &'static str)> {
    item.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::Image(image) => Some((image.bytes.as_slice(), image.format.extension())),
        ClipboardEntry::String(_) | ClipboardEntry::ExternalPaths(_) => None,
    })
}

fn exit_description(session: &SessionRecord) -> String {
    let SessionStatus::Exited(info) = &session.status else {
        return "Session ended".to_owned();
    };
    match info.reason {
        ExitReason::DaemonRestart => "Session ended when the daemon restarted".to_owned(),
        ExitReason::Signaled => "Agent was stopped".to_owned(),
        ExitReason::Exited if info.code == Some(0) => "Agent exited".to_owned(),
        ExitReason::Exited => format!("Agent exited (code {})", info.code.unwrap_or(-1)),
        ExitReason::External => "Imported session — not started yet".to_owned(),
        ExitReason::Archived => "Archived".to_owned(),
        ExitReason::Unknown => "Session ended".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Image, ImageFormat, KeyDownEvent, Keystroke, Modifiers, TestAppContext, point};
    use zeus_proto::{
        DateMillis, ExitInfo, NeedsInputDetail, NeedsInputKind, NeedsInputSource, SessionListResult,
    };

    use super::*;

    /// Replays a drag as the render loop sees it -- a geometry change every
    /// `frame`, for `frames` frames -- and returns when each size reached the
    /// daemon. Mirrors `update_selected_geometry`: `Arm`/`Fold` hold the size,
    /// and an armed tick fires on the cadence carrying the newest one.
    fn simulate_drag(frames: u32, frame: Duration) -> Vec<Duration> {
        let mut sent = Vec::new();
        let mut last_sent: Option<Duration> = None;
        let mut armed_at: Option<Duration> = None;
        let mut now = Duration::ZERO;
        for tick in 0..frames {
            now += frame;
            // The armed tick fires on its own, independent of the frame.
            if let Some(at) = armed_at
                && now >= at
            {
                sent.push(at);
                last_sent = Some(at);
                armed_at = None;
            }
            let since = last_sent.map(|at| now.saturating_sub(at));
            match plan_resize(tick == 0, since, armed_at.is_some()) {
                ResizePlan::SendNow => {
                    sent.push(now);
                    last_sent = Some(now);
                }
                ResizePlan::Arm(delay) => armed_at = Some(now + delay),
                ResizePlan::Fold => {}
            }
        }
        if let Some(at) = armed_at {
            sent.push(at);
        }
        sent
    }

    #[test]
    fn a_live_drag_keeps_resizing_the_pty_at_the_cadence() {
        // One second of dragging at 120Hz. The trailing-edge debounce this
        // replaced sent exactly one resize here -- after the mouse stopped --
        // which is why the terminal appeared to reflow only on drop. The
        // expected count derives from the cadence so it moves with it.
        let sent = simulate_drag(120, Duration::from_millis(8));
        let expected = (1000 / RESIZE_CADENCE.as_millis()) as usize;
        assert!(
            sent.len().abs_diff(expected) <= 3,
            "expected ~{expected} resizes in a second of dragging, got {}",
            sent.len()
        );
        // Leading edge: the drag's first frame is not made to wait.
        assert_eq!(sent[0], Duration::from_millis(8));
        // And no two land closer together than the cadence.
        for pair in sent.windows(2) {
            assert!(
                pair[1].saturating_sub(pair[0]) >= RESIZE_CADENCE,
                "{pair:?} are closer than the cadence"
            );
        }
    }

    #[test]
    fn the_size_a_drag_ends_on_always_reaches_the_daemon() {
        // Three frames then release: the last size must still go out, or the
        // pane keeps painting a grid the daemon has never been told about.
        let sent = simulate_drag(3, Duration::from_millis(8));
        assert!(sent.len() >= 2, "the release size must be sent: {sent:?}");
        let release = Duration::from_millis(3 * 8);
        assert!(
            *sent.last().expect("sent") <= release + RESIZE_CADENCE,
            "the final size lands within one cadence of release: {sent:?}"
        );
    }

    #[test]
    fn an_isolated_resize_never_waits() {
        // A window snap or a session switch is one change after a long idle.
        assert_eq!(
            plan_resize(false, Some(Duration::from_secs(3)), false),
            ResizePlan::SendNow
        );
        assert_eq!(plan_resize(false, None, false), ResizePlan::SendNow);
        // The first measure after attach is what a deferred launch waits for.
        assert_eq!(
            plan_resize(true, Some(Duration::ZERO), true),
            ResizePlan::SendNow
        );
    }

    fn grid_frame(cols: u16, full: bool) -> GridUpdate {
        GridUpdate {
            cols,
            rows: 40,
            cursor_col: 0,
            cursor_row: 0,
            cursor_visible: true,
            is_full_snapshot: full,
            changed_rows: Vec::new(),
        }
    }

    fn reflow_hold() -> ReflowHold {
        ReflowHold {
            parked: Vec::new(),
            saw_snapshot: false,
            _release: Task::ready(()),
        }
    }

    #[test]
    fn a_panel_toggle_holds_the_grid_but_a_drag_keeps_reflowing() {
        // ⌘B after any pause: one column change, held so the re-wrap and the
        // program's repaint land together.
        assert!(should_hold_reflow(
            (120, 40),
            (100, 40),
            Some(Duration::from_secs(3))
        ));
        // A drag steps every few frames; freezing it would stop the grid from
        // reflowing under the cursor, which is the whole point of the cadence.
        assert!(!should_hold_reflow(
            (120, 40),
            (119, 40),
            Some(Duration::from_millis(16))
        ));
    }

    #[test]
    fn a_change_with_no_reflow_in_it_is_never_held() {
        // Rows-only: the daemon crops or extends, nothing re-wraps.
        assert!(!should_hold_reflow((120, 40), (120, 30), None));
        // The first measure after attach has nothing on screen to hold.
        assert!(!should_hold_reflow((0, 0), (120, 40), None));
    }

    #[test]
    fn a_hold_ends_on_the_repaint_that_follows_the_re_wrap() {
        let mut hold = reflow_hold();
        // The daemon's re-wrapped snapshot: on its own this is the frame that
        // used to shove the content up, so it must not release the hold.
        assert!(!hold.park(grid_frame(100, true)));
        // The program answering SIGWINCH completes the pair.
        assert!(hold.park(grid_frame(100, false)));
        assert_eq!(hold.parked.len(), 2);
    }

    #[test]
    fn a_re_seed_mid_hold_does_not_stand_in_for_the_repaint() {
        let mut hold = reflow_hold();
        assert!(!hold.park(grid_frame(100, true)));
        assert!(!hold.park(grid_frame(100, true)));
        assert!(hold.park(grid_frame(100, false)));
    }

    #[test]
    fn a_repaint_arriving_before_any_snapshot_keeps_waiting() {
        // Output already in flight when the resize went out is not the answer
        // to it; releasing on it would paint the pre-reflow grid.
        let mut hold = reflow_hold();
        assert!(!hold.park(grid_frame(120, false)));
    }

    fn fixture_session() -> SessionRecord {
        let envelope: serde_json::Value = serde_json::from_str(include_str!(
            "../../zeus-proto/tests/fixtures/session_list_response.json"
        ))
        .unwrap();
        let list: SessionListResult = serde_json::from_value(envelope["ok"].clone()).unwrap();
        list.sessions[0].clone()
    }

    fn pull_request(url: &str) -> PullRequestStatus {
        PullRequestStatus {
            url: url.to_owned(),
            number: 42,
            title: Some("Keep terminal resident".to_owned()),
            author: None,
            body: None,
            base_ref_name: None,
            head_ref_name: None,
            state: "OPEN".to_owned(),
            is_draft: false,
            review_decision: Some("APPROVED".to_owned()),
            mergeable: Some("MERGEABLE".to_owned()),
            merge_state_status: Some("CLEAN".to_owned()),
            additions: 45,
            deletions: 12,
            changed_files: 3,
            comment_count: 2,
            review_count: 1,
            resolved_threads: Some(3),
            total_threads: Some(5),
            checks_passed: 3,
            checks_failed: 1,
            checks_pending: 1,
            checks: Some(vec![
                PrCheck {
                    name: "build".to_owned(),
                    result: "pending".to_owned(),
                    detail: None,
                    url: None,
                },
                PrCheck {
                    name: "lint".to_owned(),
                    result: "fail".to_owned(),
                    detail: None,
                    url: Some("https://example.com/lint".to_owned()),
                },
                PrCheck {
                    name: "test".to_owned(),
                    result: "pass".to_owned(),
                    detail: None,
                    url: None,
                },
            ]),
            discussion: None,
            fetched_at: DateMillis(1.0),
        }
    }

    #[test]
    fn chips_follow_swift_artifact_pr_family_then_ports_order() {
        let mut session = fixture_session();
        let url = "https://github.com/zeus/zeus/pull/42";
        session.artifacts = Some(vec![SessionArtifact {
            kind: ArtifactKind::PullRequest,
            url: url.to_owned(),
            first_seen_at: DateMillis(1.0),
        }]);
        session.pull_requests = Some(vec![pull_request(url)]);
        session.listening_ports = Some(vec![zeus_proto::PortInfo {
            port: 3000,
            process_name: "vite".to_owned(),
        }]);

        let chips = PaneChip::for_session(&session);
        assert_eq!(chips.len(), 4);
        assert_eq!(chips[0].label, "PR #42 +45 −12");
        assert_eq!(chips[0].tint, Some(ChipTint::Green));
        assert_eq!(chips[1].label, "3/5");
        assert_eq!(chips[1].tint, Some(ChipTint::Red));
        assert!(chips[1].checks.is_some());
        assert_eq!(chips[2].label, "3/5");
        assert_eq!(chips[2].tint, Some(ChipTint::Orange));
        assert_eq!(chips[3].label, ":3000");
        assert_eq!(chips[3].open_url.as_deref(), Some("http://localhost:3000"));
    }

    #[test]
    fn toolbar_prioritizes_pr_destinations_and_collapses_low_priority_links() {
        let mut session = fixture_session();
        let first_pr = "https://github.com/zeus/zeus/pull/7";
        let second_pr = "https://github.com/zeus/zeus/pull/8";
        session.artifacts = Some(vec![
            SessionArtifact {
                kind: ArtifactKind::Link,
                url: "https://docs.example.com/reference".to_owned(),
                first_seen_at: DateMillis(1.0),
            },
            SessionArtifact {
                kind: ArtifactKind::PullRequest,
                url: first_pr.to_owned(),
                first_seen_at: DateMillis(2.0),
            },
            SessionArtifact {
                kind: ArtifactKind::Preview,
                url: "https://preview.example.com".to_owned(),
                first_seen_at: DateMillis(3.0),
            },
            SessionArtifact {
                kind: ArtifactKind::PullRequest,
                url: second_pr.to_owned(),
                first_seen_at: DateMillis(4.0),
            },
        ]);
        session.pull_requests = Some(vec![pull_request(first_pr), pull_request(second_pr)]);

        let chips = PaneChip::for_session(&session);
        assert!(chips[0].label.starts_with("PR #7"));
        assert!(chips[1].label.starts_with("PR #8"));
        assert!(
            chips
                .iter()
                .position(|chip| chip.label == "docs.example.com")
                .is_some_and(|index| index > 1)
        );
        assert_eq!(
            toolbar_visible_chip_count(&chips, 5_000.0, true),
            TOOLBAR_MAX_VISIBLE_LINKS
        );
        assert_eq!(toolbar_visible_chip_count(&chips, 700.0, false), 0);
        assert_eq!(
            toolbar_visible_chip_count(&chips, crate::workbench::MIN_TERMINAL_WIDTH, true),
            0,
            "the reserved terminal minimum collapses chips rather than clipping them"
        );
        let compact_plain = toolbar_visible_chip_count(&chips, 780.0, true);
        let compact_chrome = toolbar_visible_chip_count(&chips, 780.0, false);
        assert!(compact_plain >= compact_chrome);
        assert!(compact_plain <= TOOLBAR_MAX_VISIBLE_LINKS);
    }

    #[test]
    fn check_popover_prioritizes_failure_then_running() {
        let checks = sorted_checks(&pull_request("https://example.com/pull/42"));
        assert_eq!(
            checks
                .iter()
                .map(|check| check.result.as_str())
                .collect::<Vec<_>>(),
            ["fail", "pending", "pass"]
        );
    }

    #[gpui::test]
    fn an_empty_terminal_pane_shows_neutral_state_and_sidebar_control(cx: &mut TestAppContext) {
        let runtime = Arc::new(StoreRuntime::inert());
        let tokio = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );

        let (pane, cx) = cx.add_window_view(move |window, cx| {
            let mut pane = TerminalPane::new(runtime, tokio, window, cx);
            pane.set_shell_chrome(ShellChrome::default(), cx);
            pane
        });

        assert!(
            pane.read_with(cx, |pane, _| pane.selected_session().is_none()),
            "fixture must exercise the empty terminal state"
        );
        assert!(cx.debug_bounds("WORKSPACE_EMPTY").is_some());
        assert!(cx.debug_bounds("workspace-welcome").is_none());
        assert!(cx.debug_bounds("welcome-open-folder").is_none());
        assert!(
            cx.debug_bounds("show-sidebar").is_some(),
            "collapsing the sidebar must leave a way to reveal it"
        );
    }

    #[test]
    fn lineage_tree_layout_centers_parents_over_their_children() {
        let store =
            crate::sidebar::SidebarPreviewFixture::make(crate::sidebar::PreviewScenario::Typical)
                .into_store();
        let strip = store
            .lineage_strip_for(&SessionId::new("preview-codex"))
            .expect("spawn family");
        let layout = layout_lineage_tree(&strip);
        let by_id: HashMap<_, _> = layout
            .nodes
            .iter()
            .map(|node| (node.id.clone(), node))
            .collect();
        let root = by_id.get(&SessionId::new("preview-codex")).expect("root");
        let child = by_id.get(&SessionId::new("preview-cursor")).expect("child");
        let sibling = by_id
            .get(&SessionId::new("preview-spawned-review"))
            .expect("sibling");
        let grandchild = by_id
            .get(&SessionId::new("preview-spawned-deep"))
            .expect("grandchild");
        assert!(root.y < child.y);
        assert_eq!(child.y, sibling.y);
        assert!(child.y < grandchild.y);
        assert!((child.cx - grandchild.cx).abs() < f32::EPSILON);
        assert!(root.cx > child.cx && root.cx < sibling.cx);
        assert!((root.cx - (child.cx + sibling.cx) / 2.0).abs() < 1.0);
        assert!(
            grandchild.y - child.y >= LINEAGE_TREE_NODE_HEIGHT,
            "vertical gap must leave room for elbow rails"
        );
        let parent_edge = root.y + LINEAGE_TREE_NODE_HEIGHT;
        let child_edge = child.y;
        assert!(
            parent_edge <= child_edge,
            "rails must leave the caption before entering the next mark"
        );
        let gutter = child_edge - parent_edge;
        assert!(
            gutter + f32::EPSILON >= LINEAGE_TREE_V_GAP,
            "elbow bus must sit in the gutter under the caption, not through it"
        );
    }

    #[gpui::test]
    fn agent_tabs_render_the_complete_family_and_navigate_between_nodes(cx: &mut TestAppContext) {
        let fixture =
            crate::sidebar::SidebarPreviewFixture::make(crate::sidebar::PreviewScenario::Typical);
        let runtime = Arc::new(StoreRuntime::preview_from(fixture.into_store()));
        let runtime_for_view = Arc::clone(&runtime);
        let tokio = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );

        let (_pane, cx) = cx.add_window_view(move |window, cx| {
            TerminalPane::new_preview(runtime_for_view, tokio, window, cx)
        });

        let tabs = cx.debug_bounds("AGENT_TABS").expect("agent tab bar");
        assert_eq!(f32::from(tabs.size.height), Metrics::LINEAGE_STRIP);
        assert!(
            cx.debug_bounds("AGENT_TREE").is_none(),
            "tabs mode must not paint the workflow tree"
        );
        for selector in [
            "LINEAGE_TAB_preview-codex",
            "LINEAGE_TAB_preview-cursor",
            "LINEAGE_TAB_preview-spawned-deep",
            "LINEAGE_TAB_preview-spawned-review",
        ] {
            let node = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("missing tab {selector}"));
            assert_eq!(f32::from(node.size.height), LINEAGE_TAB_HEIGHT);
            assert!(node.top() >= tabs.top() && node.bottom() <= tabs.bottom());
        }

        let child = cx
            .debug_bounds("LINEAGE_TAB_preview-cursor")
            .expect("direct child tab");
        cx.simulate_click(child.center(), Modifiers::default());
        assert_eq!(
            runtime
                .store
                .read()
                .expect("session store lock poisoned")
                .selected_session_id(),
            Some(&SessionId::new("preview-cursor"))
        );
        assert!(
            cx.debug_bounds("LINEAGE_TAB_preview-spawned-deep")
                .is_some(),
            "selecting a child keeps the complete family available"
        );
    }

    #[gpui::test]
    fn agent_tree_is_a_separate_workflow_view(cx: &mut TestAppContext) {
        let fixture =
            crate::sidebar::SidebarPreviewFixture::make(crate::sidebar::PreviewScenario::Typical);
        let runtime = Arc::new(StoreRuntime::preview_from(fixture.into_store()));
        let runtime_for_view = Arc::clone(&runtime);
        let tokio = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );

        let (_pane, cx) = cx.add_window_view(move |window, cx| {
            TerminalPane::new_preview(runtime_for_view, tokio, window, cx)
        });

        let tree_mode = cx
            .debug_bounds("LINEAGE_MODE_TREE")
            .expect("tree mode switch");
        cx.simulate_click(tree_mode.center(), Modifiers::default());

        let tree = cx.debug_bounds("AGENT_TREE").expect("workflow tree");
        assert!(
            f32::from(tree.size.height) > Metrics::LINEAGE_STRIP,
            "tree view owns the pane, not the tab strip"
        );
        assert!(
            cx.debug_bounds("AGENT_TABS").is_none(),
            "tree mode replaces the tab strip with the genealogy"
        );
        let root = cx
            .debug_bounds("LINEAGE_NODE_preview-codex")
            .expect("root node");
        let child = cx
            .debug_bounds("LINEAGE_NODE_preview-cursor")
            .expect("child node");
        let sibling = cx
            .debug_bounds("LINEAGE_NODE_preview-spawned-review")
            .expect("sibling node");
        let grandchild = cx
            .debug_bounds("LINEAGE_NODE_preview-spawned-deep")
            .expect("grandchild node");
        assert!(root.bottom() <= child.top());
        assert_eq!(f32::from(child.origin.y), f32::from(sibling.origin.y));
        assert!(child.bottom() <= grandchild.top());
        assert!(root.left() < sibling.left());
        assert!((f32::from(child.origin.x) - f32::from(grandchild.origin.x)).abs() < 2.0);
        assert!(root.center().x > child.center().x && root.center().x < sibling.center().x);

        cx.simulate_click(grandchild.center(), Modifiers::default());
        assert_eq!(
            runtime
                .store
                .read()
                .expect("session store lock poisoned")
                .selected_session_id(),
            Some(&SessionId::new("preview-spawned-deep"))
        );
        assert!(
            cx.debug_bounds("AGENT_TREE").is_some(),
            "selecting a node stays in the tree view"
        );

        let tabs_mode = cx
            .debug_bounds("LINEAGE_MODE_TABS")
            .expect("tabs mode switch");
        cx.simulate_click(tabs_mode.center(), Modifiers::default());
        assert!(cx.debug_bounds("AGENT_TABS").is_some());
        assert!(cx.debug_bounds("AGENT_TREE").is_none());
        assert!(
            cx.debug_bounds("LINEAGE_TAB_preview-spawned-deep")
                .is_some(),
            "the tab bar keeps the session selected in the tree"
        );
    }

    #[gpui::test]
    fn mirrored_inspector_toggle_moves_between_the_middle_and_panel_headers(
        cx: &mut TestAppContext,
    ) {
        let runtime = Arc::new(StoreRuntime::inert());
        let session = fixture_session();
        {
            let mut store = runtime.store.write().expect("session store lock poisoned");
            store.upsert_session(session.clone());
            store.select(session.id);
        }
        let tokio = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );

        let (pane, cx) = cx.add_window_view(move |window, cx| {
            let mut pane = TerminalPane::new(runtime, tokio, window, cx);
            pane.set_shell_chrome(
                ShellChrome {
                    sidebar_visible: true,
                    inspector_open: false,
                    mirrored: true,
                    ..ShellChrome::default()
                },
                cx,
            );
            pane
        });

        let toggle = cx
            .debug_bounds("TERMINAL_INSPECTOR_TOGGLE")
            .expect("closed left inspector should reveal from the middle toolbar");
        assert!(
            toggle.left() < px(100.0),
            "the left inspector reveal belongs on the middle panel's leading edge"
        );

        pane.update(cx, |pane, cx| {
            pane.set_shell_chrome(
                ShellChrome {
                    sidebar_visible: true,
                    inspector_open: true,
                    mirrored: true,
                    ..ShellChrome::default()
                },
                cx,
            );
        });

        assert!(
            cx.debug_bounds("TERMINAL_INSPECTOR_TOGGLE").is_none(),
            "the open inspector owns the toggle instead of duplicating it in the middle"
        );
    }

    #[gpui::test]
    fn restored_session_renders_immediately_without_a_startup_canvas(cx: &mut TestAppContext) {
        let runtime = Arc::new(StoreRuntime::inert());
        let session = fixture_session();
        {
            let mut store = runtime.store.write().expect("session store lock poisoned");
            store.upsert_session(session.clone());
            store.select(session.id);
        }
        let tokio = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );

        let (_pane, cx) =
            cx.add_window_view(move |window, cx| TerminalPane::new(runtime, tokio, window, cx));

        assert!(cx.debug_bounds("workspace-welcome").is_none());
        assert!(cx.debug_bounds("WORKSPACE_EMPTY").is_none());
    }

    #[gpui::test]
    fn selecting_a_newly_spawned_session_focuses_its_terminal(cx: &mut TestAppContext) {
        let runtime = Arc::new(StoreRuntime::inert());
        let tokio = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );
        let existing = fixture_session();
        {
            let mut store = runtime.store.write().expect("session store lock poisoned");
            store.upsert_session(existing.clone());
            store.select(existing.id.clone());
        }

        let runtime_for_view = Arc::clone(&runtime);
        let (pane, cx) = cx.add_window_view(move |window, cx| {
            TerminalPane::new(runtime_for_view, tokio, window, cx)
        });
        let _picker_focus = pane.update_in(cx, |pane, window, cx| {
            let picker_focus = cx.focus_handle();
            window.focus(&picker_focus, cx);
            assert!(!pane.is_focused(window));
            picker_focus
        });
        pane.update_in(cx, |pane, window, cx| {
            pane.reconcile_store_change(window, cx);
            assert!(
                !pane.is_focused(window),
                "an unrelated store update must not steal focus from the picker"
            );
        });

        let mut spawned = fixture_session();
        spawned.id = SessionId::new("spawned");
        {
            let mut store = runtime.store.write().expect("session store lock poisoned");
            store.upsert_session(spawned.clone());
            store.select(spawned.id);
        }

        // A successful spawn selects the daemon's new id asynchronously,
        // after the picker owned focus; the follow-selection pane must take
        // focus with that production store-change reconciliation.
        pane.update_in(cx, |pane, window, cx| {
            pane.reconcile_store_change(window, cx);
            assert!(pane.is_focused(window));
        });
    }

    #[gpui::test]
    fn terminal_popovers_dismiss_on_an_outside_click(cx: &mut TestAppContext) {
        let runtime = Arc::new(StoreRuntime::inert());
        let tokio = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );
        let mut session = fixture_session();
        let url = "https://github.com/zeus/zeus/pull/42";
        session.artifacts = Some(vec![SessionArtifact {
            kind: ArtifactKind::PullRequest,
            url: url.to_owned(),
            first_seen_at: DateMillis(1.0),
        }]);
        session.pull_requests = Some(vec![pull_request(url)]);
        let checks_id = PaneChip::for_session(&session)
            .into_iter()
            .find(|chip| chip.checks.is_some())
            .expect("fixture should expose a checks chip")
            .id;
        {
            let mut store = runtime.store.write().expect("session store lock poisoned");
            store.upsert_session(session.clone());
            store.select(session.id.clone());
        }

        let (pane, cx) = cx.add_window_view(move |window, cx| {
            let mut pane = TerminalPane::new(runtime, tokio, window, cx);
            pane.open_checks_for = Some(checks_id);
            pane
        });
        let outside_panel = point(px(500.0), px(320.0));

        cx.simulate_click(outside_panel, Modifiers::default());
        assert_eq!(
            pane.read_with(cx, |pane, _| pane.open_checks_for.clone()),
            None
        );

        pane.update(cx, |pane, cx| {
            pane.overflow_open = true;
            cx.notify();
        });
        cx.simulate_click(outside_panel, Modifiers::default());
        assert!(!pane.read_with(cx, |pane, _| pane.overflow_open));
    }

    #[test]
    fn needs_input_glyph_preserves_destructive_risk() {
        let mut session = fixture_session();
        session.status = SessionStatus::NeedsInput(NeedsInputKind::Permission);
        session.needs_input = Some(NeedsInputDetail {
            kind: NeedsInputKind::Permission,
            source: NeedsInputSource::ClaudePermissionHook,
            tool_name: Some("Bash".to_owned()),
            summary: "Approve command".to_owned(),
            prompt_excerpt: None,
            options: None,
            risk_hint: RiskHint::Destructive,
            occurred_at: DateMillis(2.0),
        });
        assert_eq!(
            status_state(&session),
            StatusState::NeedsInput { destructive: true }
        );
    }

    #[test]
    fn daemon_restart_exit_copy_matches_reference() {
        let mut session = fixture_session();
        session.status = SessionStatus::Exited(ExitInfo {
            reason: ExitReason::DaemonRestart,
            code: None,
            signal: None,
        });
        assert_eq!(
            exit_description(&session),
            "Session ended when the daemon restarted"
        );
    }

    #[test]
    fn gpui_key_adapter_feeds_existing_terminal_encoder() {
        let event = KeyDownEvent {
            keystroke: Keystroke::parse("up").unwrap(),
            is_held: false,
            prefer_character_input: false,
        };
        let mapped = terminal_key_event(&event).unwrap();
        assert_eq!(
            encode_key(&mapped, TermModifiers::default(), TermInputModes::default()),
            b"\x1b[A"
        );

        let command_backspace = KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers {
                    platform: true,
                    ..Modifiers::default()
                },
                key: "backspace".to_owned(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        };
        let mapped = terminal_key_event(&command_backspace).unwrap();
        assert_eq!(
            encode_key(
                &mapped,
                TermModifiers {
                    cmd: true,
                    ..TermModifiers::default()
                },
                TermInputModes::default()
            ),
            [0x15]
        );

        let live_modes = terminal_input_modes(WireTerminalModes {
            application_cursor_keys: true,
            bracketed_paste: true,
            alternate_scroll: true,
            focus_reporting: true,
            ..WireTerminalModes::default()
        });
        assert!(live_modes.alternate_scroll);
        assert!(live_modes.focus_reporting);
        assert_eq!(
            encode_key(
                &terminal_key_event(&event).unwrap(),
                TermModifiers::default(),
                live_modes
            ),
            b"\x1bOA",
            "the pane must pass live DECCKM to the encoder"
        );
        assert_eq!(
            paste("two\nlines", live_modes.bracketed_paste),
            b"\x1b[200~two\nlines\x1b[201~",
            "the pane must pass live bracketed-paste mode"
        );

        let modifier_backspaces = [
            (Modifiers::default(), b"\x7f".as_slice()),
            (
                Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
                b"\x08".as_slice(),
            ),
            (
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
                b"\x1b\x7f".as_slice(),
            ),
            (
                Modifiers {
                    platform: true,
                    ..Modifiers::default()
                },
                b"\x15".as_slice(),
            ),
        ];
        for (gpui_modifiers, expected) in modifier_backspaces {
            let event = KeyDownEvent {
                keystroke: Keystroke {
                    modifiers: gpui_modifiers,
                    key: "backspace".to_owned(),
                    key_char: None,
                },
                is_held: true,
                prefer_character_input: false,
            };
            let mapped = terminal_key_event(&event).unwrap();
            let modifiers = TermModifiers {
                shift: gpui_modifiers.shift,
                ctrl: gpui_modifiers.control,
                alt: gpui_modifiers.alt,
                cmd: gpui_modifiers.platform,
            };
            assert_eq!(encode_key(&mapped, modifiers, live_modes), expected);
        }

        let f20 = KeyDownEvent {
            keystroke: Keystroke::parse("f20").unwrap(),
            is_held: false,
            prefer_character_input: false,
        };
        assert_eq!(
            encode_key(
                &terminal_key_event(&f20).expect("F20 adapter"),
                TermModifiers::default(),
                live_modes,
            ),
            b"\x1b[34~"
        );

        let composed = KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
                key: "a".to_owned(),
                key_char: Some("å".to_owned()),
            },
            is_held: false,
            prefer_character_input: true,
        };
        let mapped = terminal_key_event(&composed).expect("Option-composed adapter");
        let option = TermModifiers {
            alt: true,
            ..TermModifiers::default()
        };
        assert_eq!(encode_key(&mapped, option, live_modes), "å".as_bytes());
        assert_eq!(
            encode_key(
                &mapped,
                option,
                TermInputModes {
                    option_as_meta: true,
                    ..live_modes
                },
            ),
            b"\x1ba"
        );
    }

    #[test]
    fn wire_mouse_modes_drive_zed_compatible_reports() {
        let modes = terminal_mouse_modes(WireTerminalModes {
            mouse_reporting: true,
            mouse_sgr: true,
            mouse_drag: true,
            ..WireTerminalModes::default()
        });
        assert_eq!(
            press_report(
                3,
                7,
                TermMouseButton::Left,
                MouseModifiers {
                    alt: true,
                    control: true,
                    ..MouseModifiers::default()
                },
                modes,
            ),
            Some(b"\x1b[<24;4;8M".to_vec())
        );
        assert_eq!(
            motion_report(
                3,
                7,
                Some(TermMouseButton::Left),
                MouseModifiers::default(),
                modes,
            ),
            Some(b"\x1b[<32;4;8M".to_vec())
        );
    }

    #[test]
    fn clipboard_image_entries_are_detected_before_text_paste() {
        let item = ClipboardItem::new_image(&Image {
            format: ImageFormat::Png,
            bytes: b"clipboard png".to_vec(),
            id: 7,
        });

        let (bytes, extension) = clipboard_image(&item).expect("image payload");
        assert_eq!(bytes, b"clipboard png");
        assert_eq!(extension, "png");
        assert_eq!(item.text(), None);
    }

    #[test]
    fn unsupported_agents_get_an_explanation_instead_of_paths() {
        let shell = zeus_proto::AgentDescriptor {
            id: "shell".into(),
            display_name: "Shell".into(),
            ..zeus_proto::AgentDescriptor::default()
        };
        match decide_drop(Some(&shell), &[std::path::PathBuf::from("/tmp/a.png")]) {
            AttachmentDecision::Unsupported { message } => {
                assert!(message.contains("does not accept image attachments"));
            }
            other => panic!("expected unsupported, got {other:?}"),
        }
    }

    #[test]
    fn remote_upload_failure_does_not_build_a_local_paste_payload() {
        let failed: Result<Vec<String>, String> = Err("scp failed".into());
        assert!(failed.is_err());
        let success = vec!["/home/dev/.cache/zeus/sessions/s_abc/attachments/img-1.png".to_owned()];
        assert!(!paste_paths(&success).starts_with("/tmp/"));
        assert!(!paste_paths(&success).contains("/var/folders"));
    }

    #[test]
    fn unselected_terminal_damage_updates_its_buffer_without_repainting_the_window() {
        let selected = SessionId::new("selected");
        let background = SessionId::new("background");

        // Selected session damage always paints, including when the window is
        // unfocused-but-visible on another monitor. GPUI occlusion handles
        // truly hidden windows.
        assert!(terminal_damage_should_repaint(
            Some(&selected),
            &selected,
            true
        ));
        assert!(!terminal_damage_should_repaint(
            Some(&selected),
            &background,
            true
        ));
        assert!(!terminal_damage_should_repaint(
            Some(&selected),
            &selected,
            false
        ));
    }

    #[test]
    fn protocol_grid_never_exceeds_the_columns_that_can_be_painted() {
        let metrics =
            CellMetrics::from_measurements(px(7.75), px(10.0), px(3.0), px(1.0), gpui::FontId(7));
        // A fractional-width boundary where the window estimate reports ten
        // columns, but the actual grid content box is three border pixels
        // narrower and can paint only nine.
        let reported = estimated_grid_size(101.5, 100.0, 0.0, 0.0, metrics);
        let painted = metrics.cols_for_width(px(101.5
            - GRID_HORIZONTAL_PADDING
            - GRID_LAYOUT_HORIZONTAL_CHROME));

        assert!(
            reported.0 <= painted,
            "reported {} columns but only {painted} fit",
            reported.0
        );
    }

    #[test]
    fn painted_content_box_and_settled_estimate_can_disagree_by_one_cell() {
        let metrics =
            CellMetrics::from_measurements(px(8.0), px(16.0), px(0.0), px(0.0), gpui::FontId(7));
        // 14px into a 16px row: subtracting the 2px layout chrome changes the
        // row count. Driving the PTY from both this estimate and a painted-box
        // measurement therefore ping-pongs by one row every frame.
        let viewport_height = Metrics::TITLE_BAR
            + GRID_VERTICAL_PADDING
            + GRID_LAYOUT_VERTICAL_CHROME
            + 16.0 * 29.0
            + 14.0;
        let estimated = estimated_grid_size(800.0, viewport_height, 0.0, 0.0, metrics);
        let painted_rows = metrics
            .rows_for_height(px((viewport_height
                - Metrics::TITLE_BAR
                - GRID_VERTICAL_PADDING)
                .max(1.0)))
            .max(2);
        assert_eq!(estimated.1, 29);
        assert_eq!(painted_rows, 30);
    }

    #[test]
    fn pty_columns_are_derived_from_the_solver_terminal_width() {
        use crate::workbench::{
            HorizontalLayoutInput, MIN_TERMINAL_WIDTH, solve_horizontal_layout,
        };

        let metrics =
            CellMetrics::from_measurements(px(8.0), px(16.0), px(3.0), px(1.0), gpui::FontId(7));
        let layout = solve_horizontal_layout(HorizontalLayoutInput {
            window_width: 1_400.0,
            sidebar_visible: true,
            sidebar_width: 220.0,
            inspector_visible: true,
            requested_inspector_width: 400.0,
            inspector_min_width: crate::inspector::min_width(),
            terminal_min_width: MIN_TERMINAL_WIDTH,
            mirrored: false,
        });
        let from_terminal = estimated_grid_size(layout.terminal_width, 720.0, 0.0, 0.0, metrics);
        let from_window = estimated_grid_size(1_400.0, 720.0, 0.0, 0.0, metrics);
        assert!(
            from_terminal.0 < from_window.0,
            "PTY columns must follow the settled terminal card, not the window"
        );
        assert_eq!(
            from_terminal.0,
            metrics
                .cols_for_width(px((layout.terminal_width
                    - GRID_HORIZONTAL_PADDING
                    - GRID_LAYOUT_HORIZONTAL_CHROME)
                    .max(1.0)))
                .max(2)
        );
    }
}
