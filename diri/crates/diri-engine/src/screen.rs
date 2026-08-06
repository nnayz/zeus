//! Headless terminal emulation for status detection.
//!
//! The daemon has to know what an agent *painted*, not just what bytes it
//! wrote: "do you want to proceed?" only means a blocker if it is still on the
//! visible screen after all the cursor movement, erases and redraws that
//! preceded it. So every session runs a real VT emulator with no renderer
//! attached, and detection reads plain text off its grid.
//!
//! The Swift version wrapped SwiftTerm. This wraps `alacritty_terminal`, which
//! is the same headless-core arrangement and is portable to every platform diri
//! targets.
//!
//! One gap is filled by hand: OSC 9;4 (progress) is a ConEmu extension that the
//! emulator does not model, so it is scanned out of the byte stream directly —
//! see [`scan_progress`].

use std::sync::mpsc::{self, Receiver, Sender};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;

use crate::detect::ScreenSnapshot;

/// Fixed screen geometry handed to the emulator.
#[derive(Clone, Copy, Debug)]
struct Geometry {
    cols: usize,
    rows: usize,
}

impl Dimensions for Geometry {
    fn total_lines(&self) -> usize {
        // History beyond the visible screen is not useful for detection: rules
        // read the current screen. Keeping scrollback at zero also bounds the
        // memory a runaway session can cost the daemon.
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// Collects the events the emulator emits. Only the title is interesting.
#[derive(Clone)]
struct Collector(Sender<Event>);

impl EventListener for Collector {
    fn send_event(&self, event: Event) {
        // A full channel means nobody is draining it, which is not worth
        // failing a session over.
        let _ = self.0.send(event);
    }
}

pub struct HeadlessScreen {
    term: Term<Collector>,
    parser: Processor,
    events: Receiver<Event>,
    geometry: Geometry,

    title: Option<String>,
    progress_state: Option<i64>,
    progress_value: Option<i64>,

    content_seq: u64,
    last_digest: u64,
    /// Trailing bytes of the previous chunk, so an OSC split across a read
    /// boundary is still recognized.
    progress_carry: Vec<u8>,
}

impl HeadlessScreen {
    pub fn new(cols: usize, rows: usize) -> Self {
        let geometry = Geometry {
            cols: cols.max(1),
            rows: rows.max(1),
        };
        let (sender, events) = mpsc::channel();
        let term = Term::new(Config::default(), &geometry, Collector(sender));
        Self {
            term,
            parser: Processor::new(),
            events,
            geometry,
            title: None,
            progress_state: None,
            progress_value: None,
            content_seq: 0,
            last_digest: 0,
            progress_carry: Vec::new(),
        }
    }

    /// Feeds raw PTY output into the emulator.
    ///
    /// The whole chunk goes to the parser in one call — vte has a batched
    /// fast path for plain text that byte-at-a-time feeding defeats, and the
    /// difference is multi-x on heavy output like build logs.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.scan_progress(bytes);
        let title_before = self.title.clone();
        self.parser.advance(&mut self.term, bytes);
        self.drain_events();

        // Damage is the cheap gate: when the emulator reports nothing
        // touched, skip fingerprinting entirely. When it does (which includes
        // invisible changes like a cursor toggle), a direct cell hash — no
        // per-line String allocation — decides whether the *content* changed.
        let damaged = match self.term.damage() {
            alacritty_terminal::term::TermDamage::Full => true,
            alacritty_terminal::term::TermDamage::Partial(mut lines) => lines.next().is_some(),
        };
        self.term.reset_damage();
        if damaged || self.title != title_before {
            let digest = self.digest_cells();
            if digest != self.last_digest {
                self.last_digest = digest;
                self.content_seq += 1;
            }
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.geometry = Geometry {
            cols: cols.max(1),
            rows: rows.max(1),
        };
        self.term.resize(self.geometry);
    }

    pub fn content_seq(&self) -> u64 {
        self.content_seq
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// True while the child is on the alternate screen (a full-screen program
    /// like an editor or pager).
    pub fn is_alt_screen(&self) -> bool {
        self.term
            .mode()
            .contains(alacritty_terminal::term::TermMode::ALT_SCREEN)
    }

    /// Whether the child has bracketed-paste mode on — submitted prompts are
    /// then framed as a paste so embedded newlines don't submit early.
    pub fn bracketed_paste(&self) -> bool {
        self.term
            .mode()
            .contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE)
    }

    /// The current grid geometry.
    pub fn size(&self) -> (usize, usize) {
        (self.geometry.cols, self.geometry.rows)
    }

    /// The visible grid as plain text, trailing blank lines removed.
    pub fn lines(&self) -> Vec<String> {
        let grid = self.term.grid();
        let mut lines: Vec<String> = Vec::with_capacity(self.geometry.rows);
        for row in 0..self.geometry.rows {
            let line = Line(row as i32);
            let mut text = String::with_capacity(self.geometry.cols);
            for column in 0..self.geometry.cols {
                text.push(grid[line][Column(column)].c);
            }
            lines.push(text.trim_end().to_string());
        }
        while lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }
        lines
    }

    /// A snapshot for the detection engine.
    pub fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            lines: self.lines(),
            osc_title: self.title.clone(),
            osc_progress_state: self.progress_state,
            content_seq: self.content_seq,
        }
    }

    pub fn progress(&self) -> Option<(i64, i64)> {
        Some((self.progress_state?, self.progress_value.unwrap_or(0)))
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            if let Event::Title(title) = event {
                self.title = Some(title);
            } else if matches!(event, Event::ResetTitle) {
                self.title = None;
            }
        }
    }

    /// Content fingerprint hashed straight off the grid cells, so
    /// `content_seq` only advances when the visible screen actually changed.
    /// Detection uses that to skip re-evaluating a frame it has already
    /// judged.
    fn digest_cells(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let grid = self.term.grid();
        for row in 0..self.geometry.rows {
            let line = Line(row as i32);
            for column in 0..self.geometry.cols {
                grid[line][Column(column)].c.hash(&mut hasher);
            }
        }
        self.title.hash(&mut hasher);
        hasher.finish()
    }


    /// Extracts `ESC ] 9 ; 4 ; state ; value` progress reports.
    ///
    /// Agents use this to say "I am working, 40% through"; the emulator has no
    /// concept of it and would silently drop the sequence.
    fn scan_progress(&mut self, bytes: &[u8]) {
        const PREFIX: &[u8] = b"\x1b]9;4;";
        let mut haystack = std::mem::take(&mut self.progress_carry);
        haystack.extend_from_slice(bytes);

        let mut search_from = 0;
        while let Some(found) = find(&haystack[search_from..], PREFIX) {
            let start = search_from + found + PREFIX.len();
            // Terminated by BEL or ST (ESC \).
            let Some(end) = haystack[start..]
                .iter()
                .position(|&b| b == 0x07 || b == 0x1b)
                .map(|offset| start + offset)
            else {
                // Truncated: keep it for the next chunk.
                break;
            };
            let payload = String::from_utf8_lossy(&haystack[start..end]);
            let mut parts = payload.split(';');
            self.progress_state = parts.next().and_then(|value| value.trim().parse().ok());
            self.progress_value = parts.next().and_then(|value| value.trim().parse().ok());
            search_from = end;
        }

        // Keep a tail long enough to rejoin a sequence split across reads.
        let keep = haystack.len().saturating_sub(64);
        self.progress_carry = haystack[keep..].to_vec();
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_with(input: &[u8]) -> HeadlessScreen {
        let mut screen = HeadlessScreen::new(80, 24);
        screen.feed(input);
        screen
    }

    #[test]
    fn plain_output_lands_on_the_grid() {
        let screen = screen_with(b"hello world\r\nsecond line\r\n");
        assert_eq!(screen.lines(), vec!["hello world", "second line"]);
    }

    #[test]
    fn an_erase_actually_erases() {
        // The whole point of emulating rather than grepping the byte stream:
        // text that was overwritten must not still read as present.
        let screen = screen_with(b"do you want to proceed?\r\n\x1b[2J\x1b[Hall clear\r\n");
        let text = screen.lines().join("\n");
        assert!(
            !text.contains("proceed"),
            "erased text still visible: {text:?}"
        );
        assert!(text.contains("all clear"));
    }

    #[test]
    fn cursor_movement_overwrites_in_place() {
        let screen = screen_with(b"aaaa\rbb");
        assert_eq!(screen.lines(), vec!["bbaa"]);
    }

    #[test]
    fn an_osc_title_is_captured() {
        let screen = screen_with(b"\x1b]0;my-session\x07ready\r\n");
        assert_eq!(screen.title(), Some("my-session"));
    }

    #[test]
    fn osc_9_4_progress_is_scanned_out_of_the_stream() {
        // alacritty does not model this ConEmu extension, so the engine parses
        // it directly. State 1, 40 percent.
        let screen = screen_with(b"\x1b]9;4;1;40\x07working\r\n");
        assert_eq!(screen.progress(), Some((1, 40)));
        assert_eq!(screen.snapshot().osc_progress_state, Some(1));
    }

    #[test]
    fn a_progress_sequence_split_across_reads_is_still_found() {
        let mut screen = HeadlessScreen::new(80, 24);
        screen.feed(b"\x1b]9;4;1");
        screen.feed(b";75\x07");
        assert_eq!(screen.progress(), Some((1, 75)));
    }

    #[test]
    fn content_seq_advances_only_when_the_screen_changes() {
        let mut screen = HeadlessScreen::new(80, 24);
        screen.feed(b"hello\r\n");
        let after_first = screen.content_seq();
        assert!(after_first > 0);

        // A no-op sequence paints nothing.
        screen.feed(b"\x1b[?25l");
        assert_eq!(
            screen.content_seq(),
            after_first,
            "an invisible change must not look like new content"
        );

        screen.feed(b"world\r\n");
        assert!(screen.content_seq() > after_first);
    }

    #[test]
    fn the_alternate_screen_is_detected() {
        let mut screen = HeadlessScreen::new(80, 24);
        assert!(!screen.is_alt_screen());
        screen.feed(b"\x1b[?1049h");
        assert!(screen.is_alt_screen(), "a pager or editor took the screen");
        screen.feed(b"\x1b[?1049l");
        assert!(!screen.is_alt_screen());
    }

    #[test]
    fn a_resize_reflows_to_the_new_width() {
        let mut screen = HeadlessScreen::new(80, 24);
        screen.feed(b"hello\r\n");
        screen.resize(40, 10);
        assert_eq!(screen.lines(), vec!["hello"]);
    }

    #[test]
    fn a_boxed_prompt_survives_emulation_intact() {
        // What detection actually consumes, end to end.
        let mut screen = HeadlessScreen::new(80, 24);
        screen.feed("╭──────────────────────────╮\r\n".as_bytes());
        screen.feed("│ Do you want to proceed?  │\r\n".as_bytes());
        screen.feed("│ ❯ 1. Yes                 │\r\n".as_bytes());
        screen.feed("╰──────────────────────────╯\r\n".as_bytes());

        let snapshot = screen.snapshot();
        let text = snapshot.lines.join("\n");
        assert!(text.contains("Do you want to proceed?"));
        assert!(text.contains("❯ 1. Yes"), "wide glyphs survive: {text:?}");
    }
}
