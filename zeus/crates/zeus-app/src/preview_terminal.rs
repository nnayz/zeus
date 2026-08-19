use zeus_proto::grid::{ChangedRow, GridCell, GridUpdate, TermColor, TermStyle};
use zeus_proto::{AgentKind, SessionRecord, SessionStatus};
use zeus_term::buffer::GridBuffer;

const PREVIEW_GRID_COLS: u16 = 108;
const PREVIEW_GRID_ROWS: u16 = 40;
const WRAP: usize = 88;

#[derive(Clone)]
struct Span {
    text: String,
    fg: TermColor,
    style: TermStyle,
}

type Line = Vec<Span>;

pub(crate) fn preview_session_grid(session: &SessionRecord) -> GridBuffer {
    paint_preview_lines(&transcript(session))
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
    let mut lines = codex_chrome(session, "gpt-5.3-codex");
    if session.title.contains("cloning") {
        lines.extend([
        blank(),
        user_prompt(&session.title),
        blank(),
        dim("■ Thinking"),
        wrap_body("Clone should leave the UI immediately. Progress and failures stay attached to the session, not the foreground path."),
            blank(),
            tool("Read", "zeus/crates/zeus-engine/src/workspace.rs"),
            tool("Edit", "zeus/crates/zeus-engine/src/workspace.rs"),
            tool("Edit", "zeus/crates/zeus-app/src/store/mod.rs"),
            blank(),
            wrap_body("Moved repository cloning onto a background worker. The active session still sees progress, and a failed fetch stays retryable."),
            blank(),
            dim("• 8 files  +431 −381"),
            prompt(),
        ]);
        return lines;
    }
    lines.extend([
        blank(),
        user_prompt(&session.title),
        blank(),
        dim("■ Thinking"),
        wrap_body("Pinned sessions belong above project groups. The archive bucket is a drop target, not a peer of live rows."),
        blank(),
        tool("Read", "zeus/crates/zeus-app/src/sidebar/view.rs"),
        tool("Read", "zeus/crates/zeus-app/src/sidebar/state.rs"),
        tool("Edit", "zeus/crates/zeus-app/src/sidebar/view.rs"),
        tool("Test", "zeus/crates/zeus-app/src/sidebar/view.rs"),
        blank(),
        wrap_body("Tightened the left sidebar: 28pt rows, hover-only project actions, and a live drag preview that keeps ⌘n hints aligned."),
        blank(),
        color(6, "fn project_header(name: &str, hover: bool) -> Row {"),
        color(6, "    Row::new().height(18.0).child(folder_badge(name))"),
        color(6, "}"),
        blank(),
        dim("• 3 files  +86 −41"),
        prompt(),
    ]);
    lines
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
    let mut lines = codex_chrome(session, "gpt-5.3-codex");
    lines.extend([
        blank(),
        user_prompt(&session.title),
        blank(),
        dim("■ Thinking"),
        wrap_body("A grandchild must inherit the continuing rail from its parent and terminate on the last sibling."),
        blank(),
        tool("Read", "zeus/crates/zeus-app/src/sidebar/view.rs"),
        tool("Edit", "zeus/crates/zeus-app/src/sidebar/view.rs"),
        blank(),
        wrap_body("Rail geometry checks out. Continuing segments stay 10pt inset; the terminator is only painted on the last child at each depth."),
        blank(),
        dim("idle · last turn 1m ago"),
    ]);
    lines
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
    let mut lines = codex_chrome(session, "gpt-5.3-codex");
    lines.extend([
        blank(),
        user_prompt(&session.title),
        blank(),
        dim("■ Thinking"),
        wrap_body("Need the latest retrieval numbers before calling the regression. Compare nDCG and recall against last week's baseline."),
        blank(),
        tool("Read", "benchmarks/retrieval/latest.json"),
        tool("Read", "benchmarks/retrieval/baseline.json"),
        blank(),
        color(6, "nDCG@10     0.812  →  0.847   +4.3%"),
        color(6, "Recall@50   0.901  →  0.918   +1.9%"),
        color(3, "p95 latency 184ms  →  211ms   +14.7%"),
        blank(),
        wrap_body("Quality is up, but the p95 regression is real. The extra reranker pass is the likely cause — checking the batch size next."),
        prompt(),
    ]);
    lines
}

fn codex_hibernated(session: &SessionRecord) -> Vec<Line> {
    let mut lines = codex_chrome(session, "gpt-5.3-codex");
    lines.extend([
        blank(),
        user_prompt(&session.title),
        blank(),
        wrap_body("Input handlers now ignore events until hydration finishes. The previous race could commit a half-mounted field."),
        blank(),
        tool("Edit", "packages/forms/src/HydrationGuard.ts"),
        tool("Test", "packages/forms/src/HydrationGuard.test.ts"),
        blank(),
        dim("• 2 files  +41 −18"),
        blank(),
        dim("hibernated · idle 35m"),
    ]);
    lines
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
    let mut lines = codex_chrome(session, "gpt-5.3-codex");
    lines.extend([
        blank(),
        user_prompt(&session.title),
        blank(),
        dim("■ Thinking"),
        wrap_body("The dense list is paying for a full projection rebuild on every status tick. Glyphs should not invalidate the row layout."),
        blank(),
        tool("Read", "zeus/crates/zeus-app/src/sidebar/view.rs"),
        tool("Profile", "sidebar render · 1.8k rows"),
        blank(),
        color(3, format!("rss {memory}   layout 4.2ms   glyphs 11.4ms")),
        wrap_body("Status glyphs are the hot path. Caching them per (id, state) drops the list below 1ms on the stress fixture."),
        prompt(),
    ]);
    lines
}

fn generic_transcript(session: &SessionRecord) -> Vec<Line> {
    let mut lines = match session.kind.id() {
        AgentKind::CLAUDE_CODE_ID => claude_chrome(session),
        AgentKind::CURSOR_ID => cursor_chrome(session, "composer-1.5"),
        AgentKind::GEMINI_ID => gemini_chrome(session, "gemini-2.5-pro"),
        AgentKind::SHELL_ID => vec![color(2, format!("nayz@zeus {} %", project_name(session)))],
        _ => codex_chrome(session, "gpt-5.3-codex"),
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

fn codex_chrome(session: &SessionRecord, model: &str) -> Vec<Line> {
    vec![
        line([bold(">_  Codex"), dim_span(format!("  {model}"))]),
        dim(format!(
            "    {}{}",
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

fn paint_preview_lines(lines: &[Line]) -> GridBuffer {
    let cols = PREVIEW_GRID_COLS;
    let rows = PREVIEW_GRID_ROWS;
    let width = usize::from(cols);
    let mut expanded = Vec::new();
    for line in lines {
        let mut cells = Vec::new();
        for span in line {
            for ch in span.text.chars() {
                cells.push(GridCell::new(
                    u32::from(ch),
                    span.fg,
                    TermColor::DefaultInverted,
                    span.style,
                ));
            }
        }
        if cells.is_empty() {
            expanded.push(Vec::new());
            continue;
        }
        while cells.len() > width {
            let rest = cells.split_off(width);
            expanded.push(std::mem::take(&mut cells));
            cells = rest;
        }
        expanded.push(cells);
    }
    let visible = if expanded.len() > usize::from(rows) {
        &expanded[expanded.len() - usize::from(rows)..]
    } else {
        expanded.as_slice()
    };
    let start = rows.saturating_sub(u16::try_from(visible.len()).unwrap_or(rows));
    let mut cursor_col = 1;
    let changed_rows = visible
        .iter()
        .enumerate()
        .map(|(index, cells)| {
            if index + 1 == visible.len() {
                cursor_col = u16::try_from(cells.len())
                    .unwrap_or(0)
                    .min(cols.saturating_sub(1));
            }
            ChangedRow::new(start + u16::try_from(index).unwrap_or(0), cells.clone())
        })
        .collect();
    let mut buffer = GridBuffer::new(cols, rows);
    buffer.apply(GridUpdate {
        cols,
        rows,
        cursor_col,
        cursor_row: rows.saturating_sub(1),
        cursor_visible: true,
        is_full_snapshot: true,
        changed_rows,
    });
    buffer
}

fn span(text: impl Into<String>, fg: TermColor, style: TermStyle) -> Span {
    Span {
        text: text.into(),
        fg,
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
    dim("─".repeat(72))
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

fn tool(kind: &str, path: &str) -> Line {
    line([
        span("■ ", TermColor::Ansi(4), TermStyle::empty()),
        span(format!("{kind:<8}"), TermColor::Ansi(4), TermStyle::BOLD),
        span(path, TermColor::Default, TermStyle::DIM),
    ])
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
    // Single visual line for the painter; wrapping is pre-split by callers
    // that need multiple rows. Keep short bodies on one row.
    if value.len() <= WRAP {
        return vec![text(value)];
    }
    vec![text(value)]
}

fn box_top(title: &str) -> Line {
    let pad = WRAP.saturating_sub(title.chars().count() + 5);
    line([
        color_span(8, "╭─ "),
        span(title, TermColor::Ansi(6), TermStyle::BOLD),
        color_span(8, format!(" {}", "─".repeat(pad.max(1)))),
        color_span(8, "╮"),
    ])
}

fn box_body(value: impl Into<String>) -> Line {
    let value = value.into();
    let pad = WRAP.saturating_sub(value.chars().count() + 4);
    line([
        color_span(8, "│ "),
        text(value),
        color_span(8, format!("{} │", " ".repeat(pad))),
    ])
}

fn box_choice(selected: bool, value: impl Into<String>) -> Line {
    let marker = if selected { "❯ " } else { "  " };
    let value = value.into();
    let pad = WRAP.saturating_sub(value.chars().count() + 6);
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
        color_span(8, "─".repeat(WRAP.saturating_sub(2))),
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
            ("preview-codex", "Codex"),
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
}
