//! Region extraction from a screen snapshot.
//!
//! All pure and cheap: they operate on the plain-text grid lines the emulator
//! produced. Ported from the Swift `Regions`.

use std::sync::LazyLock;

use regex::Regex;

use super::ScreenSnapshot;
use super::manifest::RegionKind;

/// Box-drawing glyphs used to detect and strip prompt boxes.
const BORDER_CHARS: &[char] = &[
    '╭', '╮', '╰', '╯', '│', '─', '┌', '┐', '└', '┘', '┃', '┏', '┓', '┗', '┛', '━', '┤', '├', '┬',
    '┴', '┼',
];

/// Markers that can begin a (possibly unboxed) prompt line.
const PROMPT_START_CHARS: &[char] = &['╭', '│', '╰', '❯', '›', '»'];

/// How many trailing non-blank lines `whole_recent` keeps.
const WHOLE_RECENT_LINES: usize = 60;

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn is_border(character: char) -> bool {
    BORDER_CHARS.contains(&character)
}

/// Non-blank lines, order preserved.
pub fn non_blank(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter(|line| !is_blank(line))
        .cloned()
        .collect()
}

/// `whole_recent`: the last 60 non-blank lines.
pub fn whole_recent(lines: &[String]) -> Vec<String> {
    let filtered = non_blank(lines);
    let start = filtered.len().saturating_sub(WHOLE_RECENT_LINES);
    filtered[start..].to_vec()
}

/// `bottom_non_empty_lines(n)`: the last `n` non-blank lines.
pub fn bottom_non_empty(lines: &[String], n: usize) -> Vec<String> {
    if n == 0 {
        return Vec::new();
    }
    let filtered = non_blank(lines);
    let start = filtered.len().saturating_sub(n);
    filtered[start..].to_vec()
}

fn is_box_line(line: &str) -> bool {
    line.chars().any(is_border)
}

/// Strips leading and trailing border glyphs from one box line, returning the
/// interior text — or `None` when the line is a pure border with nothing
/// meaningful inside.
pub fn strip_box(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let inner = trimmed
        .trim_start_matches(is_border)
        .trim_end_matches(is_border);
    if inner.trim().is_empty() {
        return None;
    }
    Some(inner.to_string())
}

/// `prompt_box_body`: the interior of the bottom-most bordered box, or — absent
/// a box — the tail starting at the last prompt-marker line.
pub fn prompt_box_body(lines: &[String]) -> Vec<String> {
    if let Some(last_bar) = lines.iter().rposition(|line| line.contains('│')) {
        let mut start = last_bar;
        let mut end = last_bar;
        while start > 0 && is_box_line(&lines[start - 1]) {
            start -= 1;
        }
        while end + 1 < lines.len() && is_box_line(&lines[end + 1]) {
            end += 1;
        }
        return lines[start..=end]
            .iter()
            .filter_map(|line| strip_box(line))
            .collect();
    }

    if let Some(index) = lines.iter().rposition(|line| {
        line.trim()
            .chars()
            .next()
            .is_some_and(|c| PROMPT_START_CHARS.contains(&c))
    }) {
        return non_blank(&lines[index..]);
    }
    Vec::new()
}

static OPTION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*[❯>›»]?\s*(\d+)[.)]\s+(.+)$").expect("option pattern compiles")
});

/// Extracts numbered options (`1. Yes`, `❯ 2. No …`) from blocker lines,
/// returning the label text.
pub fn numbered_options(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|line| {
            OPTION_REGEX
                .captures(line)
                .and_then(|captures| captures.get(2))
                .map(|label| label.as_str().trim().to_string())
        })
        .collect()
}

/// Extracts the raw lines for `region`.
pub fn extract(region: RegionKind, region_lines: usize, snapshot: &ScreenSnapshot) -> Vec<String> {
    match region {
        RegionKind::OscTitle => snapshot.osc_title.clone().into_iter().collect(),
        // The `progress` predicate reads the state directly; the region itself
        // carries no text.
        RegionKind::OscProgress => Vec::new(),
        RegionKind::WholeRecent => whole_recent(&snapshot.lines),
        RegionKind::BottomNonEmptyLines => bottom_non_empty(&snapshot.lines, region_lines),
        RegionKind::PromptBoxBody => prompt_box_body(&snapshot.lines),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &[&str]) -> Vec<String> {
        input.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn whole_recent_drops_blanks_and_caps_the_tail() {
        let mut input: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
        input.push("   ".into());
        let recent = whole_recent(&input);
        assert_eq!(recent.len(), WHOLE_RECENT_LINES);
        assert_eq!(recent.last().unwrap(), "line 99", "blank lines are dropped");
    }

    #[test]
    fn bottom_non_empty_returns_the_last_n() {
        let input = lines(&["a", "", "b", "   ", "c"]);
        assert_eq!(bottom_non_empty(&input, 2), vec!["b", "c"]);
        assert!(bottom_non_empty(&input, 0).is_empty());
    }

    #[test]
    fn strip_box_removes_borders_and_rejects_pure_separators() {
        assert_eq!(strip_box("│ hello │").as_deref(), Some(" hello "));
        assert_eq!(strip_box("╭──────╮"), None);
        assert_eq!(strip_box("   "), None);
    }

    #[test]
    fn prompt_box_body_takes_the_bottom_most_box() {
        let input = lines(&[
            "chatter above",
            "╭────────────╮",
            "│ first box  │",
            "╰────────────╯",
            "more chatter",
            "╭────────────╮",
            "│ second box │",
            "╰────────────╯",
        ]);
        let body = prompt_box_body(&input);
        assert_eq!(
            body,
            vec![" second box "],
            "only the last box is the prompt"
        );
    }

    #[test]
    fn prompt_box_body_falls_back_to_the_last_prompt_marker() {
        let input = lines(&["output", "❯ pick one", "  1. yes"]);
        assert_eq!(prompt_box_body(&input), vec!["❯ pick one", "  1. yes"]);
    }

    #[test]
    fn numbered_options_extracts_labels() {
        let input = lines(&[
            "❯ 1. Yes",
            "  2) No, and tell Claude what to do",
            "not an option",
        ]);
        assert_eq!(
            numbered_options(&input),
            vec!["Yes", "No, and tell Claude what to do"]
        );
    }
}
