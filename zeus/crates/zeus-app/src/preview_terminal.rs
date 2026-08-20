use std::cell::Cell;

use zeus_proto::grid::{ChangedRow, GridCell, GridUpdate, TermColor, TermStyle};
use zeus_proto::{AgentKind, SessionRecord, SessionStatus};
use zeus_term::buffer::GridBuffer;

const PREVIEW_GRID_COLS: u16 = 64;
const PREVIEW_GRID_ROWS: u16 = 36;

#[derive(Clone, Copy)]
struct PreviewLayout {
    cols: u16,
    rows: u16,
    wrap: usize,
    header_inner: usize,
}

impl PreviewLayout {
    const DEFAULT: Self = Self {
        cols: PREVIEW_GRID_COLS,
        rows: PREVIEW_GRID_ROWS,
        wrap: 58,
        header_inner: 50,
    };

    fn from_size(cols: u16, rows: u16) -> Self {
        let cols = cols.clamp(40, 160);
        let rows = rows.clamp(16, 80);
        let wrap = usize::from(cols).saturating_sub(2).max(32);
        let header_inner = wrap.saturating_sub(4).min(50);
        Self {
            cols,
            rows,
            wrap,
            header_inner,
        }
    }
}

thread_local! {
    static LAYOUT: Cell<PreviewLayout> = const { Cell::new(PreviewLayout::DEFAULT) };
}

fn layout() -> PreviewLayout {
    LAYOUT.with(Cell::get)
}

fn wrap_width() -> usize {
    layout().wrap
}

fn header_inner() -> usize {
    layout().header_inner
}
const CODEX_VERSION: &str = "0.148.0";
const CODEX_MODEL: &str = "gpt-5.6-sol";
const CODEX_EFFORT: &str = "xhigh";
const DEL_BG: TermColor = TermColor::Rgb(0x3a, 0x1e, 0x22);
const ADD_BG: TermColor = TermColor::Rgb(0x1a, 0x34, 0x28);
const DEL_FG: TermColor = TermColor::Rgb(0xd0, 0xb4, 0xb6);
const ADD_FG: TermColor = TermColor::Rgb(0xb6, 0xd6, 0xc2);

#[derive(Clone)]
struct Span {
    text: String,
    fg: TermColor,
    bg: TermColor,
    style: TermStyle,
}

type Line = Vec<Span>;

pub(crate) fn preview_session_grid(session: &SessionRecord) -> GridBuffer {
    preview_session_grid_sized(session, PREVIEW_GRID_COLS, PREVIEW_GRID_ROWS)
}

pub(crate) fn preview_session_grid_sized(
    session: &SessionRecord,
    cols: u16,
    rows: u16,
) -> GridBuffer {
    let next = PreviewLayout::from_size(cols, rows);
    LAYOUT.with(|slot| {
        let previous = slot.replace(next);
        let grid = paint_preview_lines(&transcript(session));
        slot.set(previous);
        grid
    })
}

fn transcript(session: &SessionRecord) -> Vec<Line> {
    match session.id.0.as_str() {
        "preview-codex" => codex_sidebar(session),
        "preview-claude" => claude_release(session),
        "preview-cursor" => cursor_focus(session),
        "preview-spawned-review" => claude_review(session),
        "preview-spawned-deep" => codex_rail(session),
        "preview-shell" => shell_dev_server(session),
        "preview-gemini" => gemini_pdf(session),
        "preview-question" => claude_question(session),
        "preview-pharos" => codex_benchmarks(session),
        "preview-sleeping" => codex_hibernated(session),
        "preview-archived" => claude_archived(session),
        "preview-long" => claude_starting(session),
        "preview-ended" => cursor_exited(session),
        "preview-memory" => codex_profile(session),
        _ => generic_transcript(session),
    }
}

fn codex_sidebar(session: &SessionRecord) -> Vec<Line> {
    let mut lines = codex_chrome(session);
    if session.title.contains("cloning") {
        lines.push(blank());
        lines.extend(codex_user(&session.title));
        lines.push(blank());
        lines.extend(codex_ran(
            "rg clone_repository zeus/crates",
            &["workspace.rs", "store/mod.rs"],
            11,
        ));
        lines.push(blank());
        lines.extend(codex_patch(
            "crates/zeus-engine/src/workspace.rs",
            8,
            3,
            &[
                HunkRow::ctx(40, "    let checkout = request.repo.clone();"),
                HunkRow::del(41, "    clone_repository(&checkout).await?;"),
                HunkRow::add(41, "    tokio::spawn(async move {"),
                HunkRow::add(42, "        clone_repository(&checkout).await"),
                HunkRow::add(43, "    });"),
                HunkRow::ctx(44, "    Ok(())"),
            ],
        ));
        lines.push(blank());
        lines.extend(codex_say_path(
            "Moved repository cloning onto a background worker in ",
            "workspace.rs",
        ));
        return finish_codex(lines);
    }
    lines.push(blank());
    lines.extend(codex_user(&session.title));
    lines.push(blank());
    lines.extend(codex_ran(
        "rg project_header crates/zeus-app/src/sidebar",
        &["view.rs", "state.rs"],
        13,
    ));
    lines.push(blank());
    lines.extend(codex_patch(
        "crates/zeus-app/src/sidebar/view.rs",
        12,
        7,
        &[
            HunkRow::ctx(214, "    let title = session.title.clone();"),
            HunkRow::del(215, "    let height = 32.0;"),
            HunkRow::del(216, "    let actions = project_actions(session);"),
            HunkRow::add(215, "    let height = 28.0;"),
            HunkRow::add(
                216,
                "    let actions = hover.then(|| row_actions(session));",
            ),
            HunkRow::ctx(217, "    Row::new()"),
        ],
    ));
    lines.push(blank());
    lines.extend(codex_ran(
        "cargo test -p zeus-app -- sidebar",
        &["running 14 tests", "test sidebar::view ... ok"],
        8,
    ));
    lines.push(dim("─".repeat(wrap_width().min(56))));
    lines.extend(codex_say_path(
        "Tightened the left sidebar: 28pt rows and hover-only actions in ",
        "view.rs",
    ));
    finish_codex(lines)
}

fn claude_release(session: &SessionRecord) -> Vec<Line> {
    let summary = session
        .needs_input
        .as_ref()
        .map(|detail| detail.summary.as_str())
        .unwrap_or("Wants to publish the release tag");
    let mut lines = claude_chrome(session);
    lines.extend([
        blank(),
        user_prompt(&session.title),
        blank(),
        claude_say("I'll prepare the signed 0.4.4 release. Checking the current version and the commits since the last tag."),
        blank(),
        claude_tool("Read", "zeus/Cargo.toml"),
        claude_result("{ version = \"0.4.3\" }"),
        claude_tool("Bash", "git log --oneline v0.4.3.."),
        claude_result("14 commits since last tag"),
        claude_tool("Bash", "git tag -s v0.4.4 -m \"0.4.4\""),
        claude_result("Permission required"),
        blank(),
        box_top("Bash"),
        box_body("git tag -s v0.4.4 -m \"0.4.4\""),
        box_body(summary),
        box_body(""),
        box_body("Do you want to proceed?"),
        box_choice(true, "1. Yes"),
        box_choice(false, "2. No"),
        box_bottom(),
    ]);
    lines
}

fn cursor_focus(session: &SessionRecord) -> Vec<Line> {
    let mut lines = cursor_chrome(session, "composer-1.5");
    lines.extend([
        blank(),
        user_prompt(&session.title),
        blank(),
        ok("Fixed project switching focus."),
        blank(),
        wrap_body("The neighbor row now keeps keyboard focus after a project switch. The list no longer jumps to the first session in the destination group."),
        blank(),
        dim("  src/sidebar/view.rs          +24 −11"),
        dim("  src/sidebar/state.rs         +9  −2"),
        dim("  src/sidebar/view.rs (test)   +37 −0"),
        blank(),
        color(2, "✓ 3 files changed"),
        blank(),
        dim("idle · last turn 1m ago"),
    ]);
    lines
}

fn claude_review(session: &SessionRecord) -> Vec<Line> {
    let mut lines = claude_chrome(session);
    lines.extend([
        blank(),
        user_prompt(&session.title),
        blank(),
        claude_say("Reviewing the projection tests against the new two-level spawn tree."),
        blank(),
        claude_tool("Read", "zeus/crates/zeus-app/src/store/projection.rs"),
        claude_tool("Read", "zeus/crates/zeus-app/src/sidebar/fixture.rs"),
        claude_tool("Grep", "row.depth"),
        blank(),
        claude_say("The indent rail needs a terminating segment on the last child. Writing a case for a grandchild under the first spawn."),
        blank(),
        claude_tool("Edit", "zeus/crates/zeus-app/src/store/tests.rs"),
        dim("  ⎿  Added typical_fixture_carries_a_two_level_spawn_tree"),
        prompt(),
    ]);
    lines
}

fn codex_rail(session: &SessionRecord) -> Vec<Line> {
    let mut lines = codex_chrome(session);
    lines.push(blank());
    lines.extend(codex_user(&session.title));
    lines.push(blank());
    lines.extend(codex_thought(
        "A grandchild must inherit the continuing rail from its parent and terminate on the last sibling.",
    ));
    lines.push(blank());
    lines.extend(codex_explored(&["Read view.rs", "Search row.depth"]));
    lines.push(blank());
    lines.extend(codex_edited(
        "+24 −6",
        &["crates/zeus-app/src/sidebar/view.rs"],
    ));
    lines.push(blank());
    lines.extend(codex_say(
        "Rail geometry checks out. Continuing segments stay 10pt inset; the terminator is only painted on the last child at each depth.",
    ));
    finish_codex(lines)
}

fn shell_dev_server(session: &SessionRecord) -> Vec<Line> {
    let port = session
        .listening_ports
        .as_deref()
        .and_then(|ports| ports.first())
        .map(|port| port.port)
        .unwrap_or(3000);
    vec![
        color(
            2,
            format!("nayz@zeus {} % npm run dev", project_name(session)),
        ),
        blank(),
        dim("> zeus-web@0.4.4 dev"),
        dim("> vite --port 3000"),
        blank(),
        line([
            color_span(2, "  VITE v6.2.1"),
            dim_span("  ready in 312 ms"),
        ]),
        blank(),
        line([
            color_span(2, "  ➜  Local:   "),
            color_span(6, format!("http://localhost:{port}/")),
        ]),
        line([
            color_span(2, "  ➜  Network: "),
            dim_span("use --host to expose"),
        ]),
        line([
            color_span(2, "  ➜  press "),
            bold("h"),
            dim_span(" + enter to show help"),
        ]),
        blank(),
        dim("11:42:18  hmr update  /src/sidebar/Sidebar.tsx"),
        dim("11:44:03  hmr update  /src/sidebar/SessionRow.tsx"),
        dim("11:46:21  page reload  /src/App.tsx"),
    ]
}

fn gemini_pdf(session: &SessionRecord) -> Vec<Line> {
    let mut lines = gemini_chrome(session, "gemini-2.5-pro");
    lines.extend([
        blank(),
        box_top("You"),
        box_body(&session.title),
        box_bottom(),
        blank(),
        line([color_span(5, "✦ "), text("The selection origin is in viewport space, but the page transform is still document-space. Tracing the hit-test path.")]),
        blank(),
        gemini_tool("Read", "src/pdf/selection.ts"),
        gemini_tool("Grep", "pageToViewport"),
        gemini_tool("Read", "src/pdf/overlay.ts"),
        blank(),
        line([color_span(5, "✦ "), text("pageToViewport applies scale but drops the page origin. The drag rect is therefore shifted by the crop box.")]),
        blank(),
        dim("⣾  Edit src/pdf/selection.ts"),
        prompt(),
    ]);
    lines
}

fn claude_question(session: &SessionRecord) -> Vec<Line> {
    let detail = session.needs_input.as_ref();
    let summary = detail
        .map(|detail| detail.summary.as_str())
        .unwrap_or("Which empty-state direction should I use?");
    let fallback_options = ["Editorial".into(), "Compact".into()];
    let options = detail
        .and_then(|detail| detail.options.as_deref())
        .unwrap_or(fallback_options.as_slice());
    let mut lines = claude_chrome(session);
    lines.extend([
        blank(),
        user_prompt(&session.title),
        blank(),
        claude_say("The import empty state can go two ways. Editorial keeps the explainer; Compact gets the user to a file picker faster."),
        blank(),
        claude_tool("Read", "src/import/EmptyState.tsx"),
        claude_tool("Read", "src/import/EmptyState.stories.tsx"),
        blank(),
        box_top("Question"),
        box_body(summary),
        box_body(""),
    ]);
    for (index, option) in options.iter().enumerate() {
        lines.push(box_choice(index == 0, format!("{}. {option}", index + 1)));
    }
    lines.push(box_bottom());
    lines
}

fn codex_benchmarks(session: &SessionRecord) -> Vec<Line> {
    let mut lines = codex_chrome(session);
    lines.push(blank());
    lines.extend(codex_user(&session.title));
    lines.push(blank());
    lines.extend(codex_thought(
        "Need the latest retrieval numbers before calling the regression. Compare nDCG and recall against last week's baseline.",
    ));
    lines.push(blank());
    lines.extend(codex_explored(&["Read latest.json", "Read baseline.json"]));
    lines.push(blank());
    lines.extend(codex_ran(
        "python benches/retrieval.py --compare",
        &[
            "nDCG@10     0.812  →  0.847   +4.3%",
            "Recall@50   0.901  →  0.918   +1.9%",
            "p95 latency 184ms  →  211ms   +14.7%",
        ],
        0,
    ));
    lines.push(blank());
    lines.extend(codex_say(
        "Quality is up, but the p95 regression is real. The extra reranker pass is the likely cause — checking the batch size next.",
    ));
    finish_codex(lines)
}

fn codex_hibernated(session: &SessionRecord) -> Vec<Line> {
    let mut lines = codex_chrome(session);
    lines.push(blank());
    lines.extend(codex_user(&session.title));
    lines.push(blank());
    lines.extend(codex_say(
        "Input handlers now ignore events until hydration finishes. The previous race could commit a half-mounted field.",
    ));
    lines.push(blank());
    lines.extend(codex_edited(
        "+41 −18",
        &[
            "packages/forms/src/HydrationGuard.ts",
            "packages/forms/src/HydrationGuard.test.ts",
        ],
    ));
    lines.push(blank());
    lines.push(dim("  hibernated · idle 35m"));
    finish_codex(lines)
}

fn claude_archived(session: &SessionRecord) -> Vec<Line> {
    let mut lines = claude_chrome(session);
    lines.extend([
        blank(),
        user_prompt(&session.title),
        blank(),
        claude_say("Compound components win when the parent owns the selection state. Render-prop bags get awkward once a trigger and a list both need the same value."),
        blank(),
        claude_tool("Read", "packages/ui/src/Select.tsx"),
        claude_tool("Read", "packages/ui/src/ComboBox.tsx"),
        blank(),
        claude_say("Wrote the comparison in composition-notes.md. Recommend the compound API for anything with a trigger + panel."),
        blank(),
        dim("exited · archived"),
    ]);
    lines
}

fn claude_starting(session: &SessionRecord) -> Vec<Line> {
    let mut lines = claude_chrome(session);
    lines.extend([
        blank(),
        user_prompt(&session.title),
        blank(),
        dim("Starting Claude Code…"),
        dim("Loading MCP servers · 2 / 4"),
        prompt(),
    ]);
    lines
}

fn cursor_exited(session: &SessionRecord) -> Vec<Line> {
    let mut lines = cursor_chrome(session, "composer-1.5");
    lines.extend([
        blank(),
        user_prompt(&session.title),
        blank(),
        ok("Accessibility pass complete."),
        blank(),
        dim("  src/pdf/Reader.tsx           +18 −6"),
        dim("  src/pdf/Toolbar.tsx          +11 −4"),
        color(2, "✓ 2 files changed"),
        blank(),
        dim("exited · code 0"),
    ]);
    lines
}

fn codex_profile(session: &SessionRecord) -> Vec<Line> {
    let memory = session
        .memory_bytes
        .map(|bytes| format!("{:.1} GB", bytes as f64 / 1_000_000_000.0))
        .unwrap_or_else(|| "7.9 GB".into());
    let mut lines = codex_chrome(session);
    lines.push(blank());
    lines.extend(codex_user(&session.title));
    lines.push(blank());
    lines.extend(codex_thought(
        "The dense list is paying for a full projection rebuild on every status tick. Glyphs should not invalidate the row layout.",
    ));
    lines.push(blank());
    lines.extend(codex_explored(&[
        "Read view.rs",
        "Profile sidebar render · 1.8k rows",
    ]));
    lines.push(blank());
    lines.push(color(
        3,
        format!("  rss {memory}   layout 4.2ms   glyphs 11.4ms"),
    ));
    lines.push(blank());
    lines.extend(codex_say(
        "Status glyphs are the hot path. Caching them per (id, state) drops the list below 1ms on the stress fixture.",
    ));
    finish_codex(lines)
}

fn generic_transcript(session: &SessionRecord) -> Vec<Line> {
    let mut lines = match session.kind.id() {
        AgentKind::CLAUDE_CODE_ID => claude_chrome(session),
        AgentKind::CURSOR_ID => cursor_chrome(session, "composer-1.5"),
        AgentKind::GEMINI_ID => gemini_chrome(session, "gemini-2.5-pro"),
        AgentKind::SHELL_ID => vec![color(2, format!("nayz@zeus {} %", project_name(session)))],
        _ => {
            let mut lines = codex_chrome(session);
            lines.push(blank());
            lines.extend(codex_user(&session.title));
            lines.push(blank());
            match &session.status {
                SessionStatus::Starting => lines.push(dim("  Starting…")),
                SessionStatus::Working => {
                    lines.extend(codex_say("Working through the current turn."));
                    return finish_codex(lines);
                }
                SessionStatus::NeedsInput(_) => {
                    if let Some(detail) = &session.needs_input {
                        lines.push(box_top("Input"));
                        lines.push(box_body(&detail.summary));
                        lines.push(box_bottom());
                    }
                }
                SessionStatus::Idle => lines.push(dim("  idle")),
                SessionStatus::Exited(_) => lines.push(dim("  exited")),
                SessionStatus::Unknown => {}
            }
            return finish_codex(lines);
        }
    };
    lines.push(blank());
    lines.push(user_prompt(&session.title));
    lines.push(blank());
    match &session.status {
        SessionStatus::Starting => lines.push(dim("Starting…")),
        SessionStatus::Working => {
            lines.push(wrap_body("Working through the current turn."));
            lines.push(prompt());
        }
        SessionStatus::NeedsInput(_) => {
            if let Some(detail) = &session.needs_input {
                lines.push(box_top("Input"));
                lines.push(box_body(&detail.summary));
                lines.push(box_bottom());
            }
        }
        SessionStatus::Idle => lines.push(dim("idle")),
        SessionStatus::Exited(_) => lines.push(dim("exited")),
        SessionStatus::Unknown => {}
    }
    lines
}

fn codex_chrome(session: &SessionRecord) -> Vec<Line> {
    let directory = home_path(&session.cwd);
    vec![
        dim(format!("╭{}╮", "─".repeat(header_inner() + 2))),
        padded_box_row(
            header_inner(),
            vec![
                dim_span(">_ "),
                bold("OpenAI Codex"),
                dim_span(format!(" (v{CODEX_VERSION})")),
            ],
        ),
        padded_box_row(header_inner(), Vec::new()),
        padded_box_row(
            header_inner(),
            vec![
                dim_span("model:     "),
                text(CODEX_MODEL),
                dim_span(" "),
                color_span(3, CODEX_EFFORT),
                dim_span("   "),
                color_span(6, "/model"),
                dim_span(" to change"),
            ],
        ),
        padded_box_row(
            header_inner(),
            vec![dim_span("directory: "), text(directory)],
        ),
        dim(format!("╰{}╯", "─".repeat(header_inner() + 2))),
        blank(),
        line([
            dim_span("  "),
            span("Tip:", TermColor::Default, TermStyle::BOLD),
            dim_span(" "),
            span("New", TermColor::Ansi(6), TermStyle::BOLD),
            text(" Use "),
            color_span(6, "/fast"),
            text(" to enable our fastest inference with increased plan usage."),
        ]),
    ]
}

fn home_path(path: &str) -> String {
    path.strip_prefix("/Users/preview")
        .map(|rest| format!("~{rest}"))
        .unwrap_or_else(|| path.to_string())
}

fn finish_codex(history: Vec<Line>) -> Vec<Line> {
    pin_bottom(history, codex_composer())
}

fn pin_bottom(mut history: Vec<Line>, footer: Vec<Line>) -> Vec<Line> {
    let rows = usize::from(layout().rows);
    let footer_h = footer.len();
    let available = rows.saturating_sub(footer_h);
    if history.len() > available {
        let keep_head = 12.min(available);
        let keep_tail = available.saturating_sub(keep_head);
        let mut kept = history[..keep_head].to_vec();
        if keep_tail > 0 {
            kept.extend(history[history.len() - keep_tail..].iter().cloned());
        }
        history = kept;
    }
    history.resize_with(available, blank);
    history.extend(footer);
    history
}

fn codex_composer() -> Vec<Line> {
    vec![
        blank(),
        line([
            dim_span("> "),
            span(
                "Ask Codex to do anything",
                TermColor::Default,
                TermStyle::DIM,
            ),
        ]),
        blank(),
        line([
            dim_span("  "),
            color_span(3, format!("{CODEX_MODEL} {CODEX_EFFORT}")),
            dim_span("  ·  "),
            color_span(2, "~/Projects/zeus"),
        ]),
    ]
}

fn codex_user(title: &str) -> Vec<Line> {
    prefix_wrap(
        span("› ", TermColor::Default, TermStyle::BOLD | TermStyle::DIM),
        "  ",
        title,
        TermColor::Default,
        TermStyle::empty(),
    )
}

fn codex_thought(body: &str) -> Vec<Line> {
    prefix_wrap(
        dim_span("• "),
        "  ",
        body,
        TermColor::Default,
        TermStyle::DIM | TermStyle::ITALIC,
    )
}

fn codex_say(body: &str) -> Vec<Line> {
    prefix_wrap(
        dim_span("• "),
        "  ",
        body,
        TermColor::Default,
        TermStyle::empty(),
    )
}

fn codex_say_path(before: &str, path: &str) -> Vec<Line> {
    vec![line([dim_span("• "), text(before), color_span(6, path)])]
}

fn codex_explored(entries: &[&str]) -> Vec<Line> {
    let mut lines = vec![line([
        dim_span("• "),
        span("Explored", TermColor::Default, TermStyle::BOLD),
    ])];
    for (index, entry) in entries.iter().enumerate() {
        let branch = if index == 0 { "  └ " } else { "    " };
        lines.push(line([dim_span(branch), dim_span(*entry)]));
    }
    lines
}

fn codex_edited(stats: &str, paths: &[&str]) -> Vec<Line> {
    let mut lines = vec![line([
        dim_span("• "),
        span("Edited", TermColor::Default, TermStyle::BOLD),
        dim_span(format!(" {stats}")),
    ])];
    for (index, path) in paths.iter().enumerate() {
        let branch = if index == 0 { "  └ " } else { "    " };
        lines.push(line([dim_span(branch), dim_span(*path)]));
    }
    lines
}

fn codex_ran(command: &str, output: &[&str], extra_lines: usize) -> Vec<Line> {
    let mut lines = vec![line([
        dim_span("• "),
        span("Ran", TermColor::Default, TermStyle::BOLD),
        dim_span(" "),
        color_span(6, command),
    ])];
    for (index, row) in output.iter().enumerate() {
        let branch = if index == 0 { "  └ " } else { "    " };
        lines.push(line([dim_span(branch), dim_span(*row)]));
    }
    if extra_lines > 0 {
        lines.push(dim(format!(
            "    … +{extra_lines} lines (ctrl + t to view transcript)"
        )));
    }
    lines
}

struct HunkRow {
    number: u32,
    kind: HunkKind,
    text: &'static str,
}

enum HunkKind {
    Ctx,
    Add,
    Del,
}

impl HunkRow {
    fn ctx(number: u32, text: &'static str) -> Self {
        Self {
            number,
            kind: HunkKind::Ctx,
            text,
        }
    }

    fn add(number: u32, text: &'static str) -> Self {
        Self {
            number,
            kind: HunkKind::Add,
            text,
        }
    }

    fn del(number: u32, text: &'static str) -> Self {
        Self {
            number,
            kind: HunkKind::Del,
            text,
        }
    }
}

fn codex_patch(path: &str, plus: u32, minus: u32, rows: &[HunkRow]) -> Vec<Line> {
    let mut lines = vec![line([
        dim_span("• "),
        span("Edited", TermColor::Default, TermStyle::BOLD),
        dim_span(" "),
        text(path),
        dim_span(" ("),
        span(format!("+{plus}"), TermColor::Ansi(2), TermStyle::empty()),
        dim_span(" "),
        span(format!("-{minus}"), TermColor::Ansi(1), TermStyle::empty()),
        dim_span(")"),
    ])];
    for row in rows {
        lines.push(hunk_line(row));
    }
    lines
}

fn hunk_line(row: &HunkRow) -> Line {
    let gutter = format!("{:>4} ", row.number);
    let marker = match row.kind {
        HunkKind::Ctx => " ",
        HunkKind::Add => "+",
        HunkKind::Del => "-",
    };
    let body = format!("{marker}{}", row.text);
    let (fg, bg) = match row.kind {
        HunkKind::Ctx => (TermColor::Default, TermColor::DefaultInverted),
        HunkKind::Add => (ADD_FG, ADD_BG),
        HunkKind::Del => (DEL_FG, DEL_BG),
    };
    let used = gutter.chars().count() + body.chars().count();
    let pad = usize::from(layout().cols).saturating_sub(used);
    line([
        dim_span(gutter),
        span_on(
            format!("{body}{}", " ".repeat(pad)),
            fg,
            bg,
            TermStyle::empty(),
        ),
    ])
}

fn prefix_wrap(
    prefix: Span,
    continuation: &str,
    body: &str,
    fg: TermColor,
    style: TermStyle,
) -> Vec<Line> {
    let prefix_width = prefix.text.chars().count();
    let width = wrap_width().saturating_sub(prefix_width).max(16);
    let wrapped = wrap_words(body, width);
    wrapped
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            if index == 0 {
                line([prefix.clone(), span(text, fg, style)])
            } else {
                line([dim_span(continuation), span(text, fg, style)])
            }
        })
        .collect()
}

fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current = word.to_string();
            continue;
        }
        if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn padded_box_row(inner: usize, spans: Vec<Span>) -> Line {
    let used: usize = spans.iter().map(|span| span.text.chars().count()).sum();
    let pad = inner.saturating_sub(used);
    let mut row = vec![dim_span("│ ")];
    row.extend(spans);
    if pad > 0 {
        row.push(dim_span(" ".repeat(pad)));
    }
    row.push(dim_span(" │"));
    row
}

fn claude_chrome(session: &SessionRecord) -> Vec<Line> {
    vec![
        line([
            color_span(3, "Claude Code"),
            dim_span("  v2.1.14"),
            dim_span(format!("  {}", session.cwd)),
        ]),
        dim(session
            .git_branch
            .as_deref()
            .map_or_else(String::new, |branch| format!("on {branch}"))),
        rule(),
    ]
}

fn cursor_chrome(session: &SessionRecord, model: &str) -> Vec<Line> {
    vec![
        line([bold("cursor agent"), dim_span(format!("  {model}"))]),
        dim(format!(
            "  {}{}",
            session.cwd,
            session
                .git_branch
                .as_deref()
                .map(|branch| format!("  ·  {branch}"))
                .unwrap_or_default()
        )),
        rule(),
    ]
}

fn gemini_chrome(session: &SessionRecord, model: &str) -> Vec<Line> {
    vec![
        line([color_span(5, "Gemini CLI"), dim_span(format!("  {model}"))]),
        dim(format!(
            "  {}{}",
            session.cwd,
            session
                .git_branch
                .as_deref()
                .map(|branch| format!("  ·  {branch}"))
                .unwrap_or_default()
        )),
        rule(),
    ]
}

fn project_name(session: &SessionRecord) -> &str {
    session.cwd.rsplit('/').next().unwrap_or(&session.cwd)
}

fn wrap_cells_wordwise(mut cells: Vec<GridCell>, width: usize) -> Vec<Vec<GridCell>> {
    if width == 0 {
        return vec![cells];
    }
    let mut lines = Vec::new();
    while cells.len() > width {
        let split = cells[..width]
            .iter()
            .rposition(|cell| cell.scalar == u32::from(' '))
            .filter(|&index| index > 0)
            .map(|index| index + 1)
            .unwrap_or(width);
        let rest = cells.split_off(split);
        lines.push(std::mem::take(&mut cells));
        cells = rest;
        let skip = cells
            .iter()
            .position(|cell| cell.scalar != u32::from(' '))
            .unwrap_or(cells.len());
        cells.drain(..skip);
    }
    lines.push(cells);
    lines
}

fn paint_preview_lines(lines: &[Line]) -> GridBuffer {
    let PreviewLayout { cols, rows, .. } = layout();
    let width = usize::from(cols);
    let mut expanded = Vec::new();
    for line in lines {
        let mut cells = Vec::new();
        for span in line {
            for ch in span.text.chars() {
                cells.push(GridCell::new(u32::from(ch), span.fg, span.bg, span.style));
            }
        }
        if cells.is_empty() {
            expanded.push(Vec::new());
            continue;
        }
        expanded.extend(wrap_cells_wordwise(cells, width));
    }
    let row_limit = usize::from(rows);
    if expanded.len() > row_limit {
        let head = 12.min(row_limit);
        let tail = row_limit.saturating_sub(head);
        let mut clipped = expanded[..head].to_vec();
        if tail > 0 {
            clipped.extend(expanded[expanded.len() - tail..].iter().cloned());
        }
        expanded = clipped;
    }
    let visible = expanded.as_slice();
    let start = rows.saturating_sub(u16::try_from(visible.len()).unwrap_or(rows));
    let (cursor_col, cursor_row) = cursor_from_visible(visible, start, cols, rows);
    let changed_rows = visible
        .iter()
        .enumerate()
        .map(|(index, cells)| {
            ChangedRow::new(start + u16::try_from(index).unwrap_or(0), cells.clone())
        })
        .collect();
    let mut buffer = GridBuffer::new(cols, rows);
    buffer.apply(GridUpdate {
        cols,
        rows,
        cursor_col,
        cursor_row,
        cursor_visible: true,
        is_full_snapshot: true,
        changed_rows,
    });
    buffer
}

fn cursor_from_visible(visible: &[Vec<GridCell>], start: u16, cols: u16, rows: u16) -> (u16, u16) {
    for (index, cells) in visible.iter().enumerate().rev() {
        let mut col = 0u16;
        for cell in cells {
            if cell.scalar == u32::from('>')
                && cells
                    .get(usize::from(col) + 1)
                    .is_some_and(|next| next.scalar == u32::from(' '))
            {
                return (
                    (col + 2).min(cols.saturating_sub(1)),
                    start + u16::try_from(index).unwrap_or(0),
                );
            }
            col = col.saturating_add(1);
        }
    }
    (
        u16::try_from(visible.last().map_or(0, Vec::len))
            .unwrap_or(0)
            .min(cols.saturating_sub(1)),
        rows.saturating_sub(1),
    )
}

fn span(text: impl Into<String>, fg: TermColor, style: TermStyle) -> Span {
    span_on(text, fg, TermColor::DefaultInverted, style)
}

fn span_on(text: impl Into<String>, fg: TermColor, bg: TermColor, style: TermStyle) -> Span {
    Span {
        text: text.into(),
        fg,
        bg,
        style,
    }
}

fn text(value: impl Into<String>) -> Span {
    span(value, TermColor::Default, TermStyle::empty())
}

fn dim_span(value: impl Into<String>) -> Span {
    span(value, TermColor::Default, TermStyle::DIM)
}

fn dim(value: impl Into<String>) -> Line {
    vec![dim_span(value)]
}

fn bold(value: impl Into<String>) -> Span {
    span(value, TermColor::Default, TermStyle::BOLD)
}

fn color(ansi: u8, value: impl Into<String>) -> Line {
    vec![color_span(ansi, value)]
}

fn line(spans: impl IntoIterator<Item = Span>) -> Line {
    spans.into_iter().collect()
}

fn blank() -> Line {
    Vec::new()
}

fn rule() -> Line {
    dim("─".repeat(wrap_width()))
}

fn prompt() -> Line {
    vec![span("▌", TermColor::Ansi(6), TermStyle::BOLD)]
}

fn user_prompt(title: &str) -> Line {
    line([color_span(6, "> "), text(title)])
}

fn ok(value: impl Into<String>) -> Line {
    line([color_span(2, "✓  "), text(value)])
}

fn claude_say(value: &str) -> Line {
    line([color_span(3, "⏺  "), text(value)])
}

fn claude_tool(kind: &str, path: &str) -> Line {
    line([
        span("⏺  ", TermColor::Ansi(3), TermStyle::empty()),
        span(format!("{kind}("), TermColor::Ansi(3), TermStyle::BOLD),
        span(path, TermColor::Default, TermStyle::empty()),
        span(")", TermColor::Ansi(3), TermStyle::BOLD),
    ])
}

fn claude_result(value: impl Into<String>) -> Line {
    dim(format!("  ⎿  {}", value.into()))
}

fn gemini_tool(kind: &str, path: &str) -> Line {
    line([
        span("  ⤷ ", TermColor::Ansi(5), TermStyle::empty()),
        span(format!("{kind}  "), TermColor::Ansi(5), TermStyle::BOLD),
        span(path, TermColor::Default, TermStyle::DIM),
    ])
}

fn wrap_body(value: &str) -> Line {
    vec![text(value)]
}

fn box_top(title: &str) -> Line {
    let pad = wrap_width().saturating_sub(title.chars().count() + 5);
    line([
        color_span(8, "╭─ "),
        span(title, TermColor::Ansi(6), TermStyle::BOLD),
        color_span(8, format!(" {}", "─".repeat(pad.max(1)))),
        color_span(8, "╮"),
    ])
}

fn box_body(value: impl Into<String>) -> Line {
    let value = value.into();
    let pad = wrap_width().saturating_sub(value.chars().count() + 4);
    line([
        color_span(8, "│ "),
        text(value),
        color_span(8, format!("{} │", " ".repeat(pad))),
    ])
}

fn box_choice(selected: bool, value: impl Into<String>) -> Line {
    let marker = if selected { "❯ " } else { "  " };
    let value = value.into();
    let pad = wrap_width().saturating_sub(value.chars().count() + 6);
    line([
        color_span(8, "│ "),
        span(
            marker,
            if selected {
                TermColor::Ansi(6)
            } else {
                TermColor::Default
            },
            TermStyle::empty(),
        ),
        span(
            value,
            TermColor::Default,
            if selected {
                TermStyle::BOLD
            } else {
                TermStyle::empty()
            },
        ),
        color_span(8, format!("{} │", " ".repeat(pad))),
    ])
}

fn box_bottom() -> Line {
    line([
        color_span(8, "╰"),
        color_span(8, "─".repeat(wrap_width().saturating_sub(2))),
        color_span(8, "╯"),
    ])
}

fn color_span(ansi: u8, value: impl Into<String>) -> Span {
    span(value, TermColor::Ansi(ansi), TermStyle::empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar::{PreviewScenario, SidebarPreviewFixture};

    fn grid_text(session: &SessionRecord) -> String {
        let grid = preview_session_grid(session);
        (0..usize::from(grid.rows))
            .filter_map(|row| grid.row_text_with_columns(row).map(|(text, _)| text))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn session<'a>(fixture: &'a SidebarPreviewFixture, id: &str) -> &'a zeus_proto::SessionRecord {
        fixture
            .list
            .sessions
            .iter()
            .find(|session| session.id.0 == id)
            .unwrap_or_else(|| panic!("{id}"))
    }

    #[test]
    fn typical_sessions_paint_distinct_agent_transcripts() {
        let fixture = SidebarPreviewFixture::make(PreviewScenario::Typical);
        let cases = [
            ("preview-codex", "OpenAI Codex"),
            ("preview-claude", "Claude Code"),
            ("preview-cursor", "cursor agent"),
            ("preview-shell", "localhost:3000"),
            ("preview-gemini", "Gemini CLI"),
            ("preview-question", "Editorial"),
            ("preview-pharos", "nDCG@10"),
        ];
        for (id, marker) in cases {
            let text = grid_text(session(&fixture, id));
            assert!(text.contains(marker), "{id} missing {marker:?}\n{text}");
            assert!(
                text.contains(&session(&fixture, id).title) || marker == "localhost:3000",
                "{id} should keep the session title visible"
            );
        }
    }

    #[test]
    fn claude_permission_prompt_lists_yes_and_no() {
        let fixture = SidebarPreviewFixture::make(PreviewScenario::Typical);
        let text = grid_text(session(&fixture, "preview-claude"));
        assert!(text.contains("Yes"));
        assert!(text.contains("No"));
        assert!(text.contains("publish the release tag"));
    }

    #[test]
    fn artifacts_codex_session_uses_the_clone_transcript() {
        let fixture = SidebarPreviewFixture::make(PreviewScenario::Artifacts);
        let text = grid_text(session(&fixture, "preview-codex"));
        assert!(text.contains("background worker"));
        assert!(text.contains(&session(&fixture, "preview-codex").title));
    }

    #[test]
    fn long_preview_lines_wrap_on_word_boundaries() {
        let cells: Vec<_> = "one two three four five six seven eight nine ten extra words here"
            .chars()
            .map(|ch| {
                GridCell::new(
                    u32::from(ch),
                    TermColor::Default,
                    TermColor::DefaultInverted,
                    TermStyle::empty(),
                )
            })
            .collect();
        let lines = wrap_cells_wordwise(cells, 24);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|line| line.len() <= 24));
        let text: String = lines
            .iter()
            .flat_map(|line| line.iter().map(|cell| char::from_u32(cell.scalar).unwrap()))
            .collect();
        assert!(text.contains("one two"));
        assert!(!text.contains("seveneight"));
    }

    #[test]
    fn typical_codex_session_matches_the_interactive_tui() {
        let fixture = SidebarPreviewFixture::make(PreviewScenario::Typical);
        let text = grid_text(session(&fixture, "preview-codex"));
        assert!(text.contains(">_ OpenAI Codex"), "{text}");
        assert!(text.contains("v0.148.0"), "{text}");
        assert!(text.contains("Tip:"), "{text}");
        assert!(text.contains("/fast"), "{text}");
        assert!(
            text.contains("› Polish the left sidebar hierarchy"),
            "{text}"
        );
        assert!(text.contains("• Ran"), "{text}");
        assert!(text.contains("• Edited"), "{text}");
        assert!(text.contains("+12"), "{text}");
        assert!(text.contains("ctrl + t to view transcript"), "{text}");
        assert!(text.contains("Ask Codex to do anything"), "{text}");
        assert!(text.contains("gpt-5.6-sol"), "{text}");
    }
}
