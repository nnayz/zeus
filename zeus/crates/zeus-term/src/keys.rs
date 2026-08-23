//! Platform-independent keyboard and paste encoding for terminal input.
//!
//! UI adapters put the logical, modifier-free value in [`Key::Character`] and
//! the platform-produced text in [`KeyEvent::text`]. Keeping both values lets
//! the caller choose Zed's macOS Option behavior: native composed characters
//! by default, or ESC-prefixed logical input when `option_as_meta` is enabled.

const ESC: u8 = 0x1b;

/// A keyboard event's logical key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Key {
    /// Characters with layout modifiers removed (AppKit's
    /// `charactersIgnoringModifiers`).
    Character(String),
    Named(NamedKey),
}

/// Non-text keys understood by the terminal encoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NamedKey {
    ArrowUp,
    ArrowDown,
    ArrowRight,
    ArrowLeft,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Tab,
    Enter,
    Escape,
    Backspace,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
}

/// A logical key plus the text produced by the platform keyboard layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyEvent {
    pub key: Key,
    /// AppKit's `characters`, including Option-composed characters. Named-key
    /// constructors fill this with the key's ordinary control character.
    pub text: Option<String>,
}

impl KeyEvent {
    /// Creates a text event whose logical and produced text are identical.
    pub fn character(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            key: Key::Character(text.clone()),
            text: Some(text),
        }
    }

    /// Creates a text event with distinct logical and platform-produced text.
    /// This is normally used for an Option-composed character.
    pub fn composed(logical: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            key: Key::Character(logical.into()),
            text: Some(text.into()),
        }
    }

    pub fn named(key: NamedKey) -> Self {
        Self {
            key: Key::Named(key),
            text: named_text(key).map(str::to_owned),
        }
    }
}

/// Keyboard modifiers relevant to terminal input.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub cmd: bool,
}

/// Terminal modes and input preferences that change emitted bytes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TermInputModes {
    /// DEC application cursor keys (DECCKM). Only unmodified arrows, Home,
    /// and End use SS3; modified keys retain xterm's CSI `1;mod` form.
    pub application_cursor_keys: bool,
    /// DEC bracketed-paste mode (DECSET 2004). Pass this to [`paste`].
    pub bracketed_paste: bool,
    /// Treat macOS Option as terminal Meta. When disabled, Option-composed
    /// printable input uses the platform-produced text unchanged. When
    /// enabled, a single logical ASCII/control key is prefixed with ESC.
    pub option_as_meta: bool,
    /// Translate wheel events in the alternate screen into application-cursor
    /// up/down keys (DEC private mode 1007).
    pub alternate_scroll: bool,
    /// Send CSI I / CSI O when the terminal pane gains or loses focus (DEC
    /// private mode 1004).
    pub focus_reporting: bool,
}

/// Encodes one key event as bytes to write to the PTY.
///
/// An empty vector means the event had no produced text and was not a known
/// named key. Command shortcuts are otherwise left for the UI adapter, except
/// Command-Backspace, which the Swift terminal explicitly maps to Ctrl-U.
pub fn encode_key(event: &KeyEvent, modifiers: Modifiers, modes: TermInputModes) -> Vec<u8> {
    // `performKeyEquivalent` handles Command-Backspace before `encode(event:)`.
    // Shift is allowed, but Option or Control makes it a different chord.
    if event.key == Key::Named(NamedKey::Backspace)
        && modifiers.cmd
        && !modifiers.alt
        && !modifiers.ctrl
    {
        return vec![0x15];
    }

    if let Key::Named(key) = event.key
        && let Some(bytes) = encode_csi_key(key, modifiers, modes.application_cursor_keys)
    {
        return bytes;
    }

    if event.key == Key::Named(NamedKey::Backspace) {
        // Zed/Alacritty's xterm mapping is the source of truth: ordinary and
        // Shift-Backspace send DEL, Ctrl-Backspace sends BS, and Option sends
        // Meta-DEL. Command-Backspace remains Zeus's Ctrl-U shortcut above.
        // Unsupported Command combinations are left to the UI shortcut layer.
        if modifiers.cmd {
            return Vec::new();
        }
        if modifiers.ctrl && !modifiers.alt {
            return vec![0x08];
        }
        return if modifiers.alt {
            vec![ESC, 0x7f]
        } else {
            vec![0x7f]
        };
    }

    let produced = event.text.as_deref().unwrap_or_default();

    // Special Option chords are terminal keys regardless of the printable
    // Option policy. This preserves Option-Enter/Tab/Escape and all named CSI
    // mappings while Option-composed text remains native by default.
    if modifiers.alt && matches!(event.key, Key::Named(_)) && !produced.is_empty() {
        let mut bytes = Vec::with_capacity(1 + produced.len());
        bytes.push(ESC);
        bytes.extend_from_slice(produced.as_bytes());
        return bytes;
    }

    if let Key::Character(logical) = &event.key {
        if modifiers.alt
            && modes.option_as_meta
            && let Some(bytes) = encode_meta_text(logical, modifiers)
        {
            return bytes;
        }
        if modifiers.ctrl
            && !modifiers.alt
            && let Some(bytes) = encode_control_text(logical)
        {
            return bytes;
        }
    }

    produced.as_bytes().to_vec()
}

/// Encodes pasted UTF-8 text, optionally using DEC bracketed-paste framing.
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    if !bracketed {
        // Zed normalizes pasted lines to the terminal's carriage-return input
        // convention without doubling CRLF into two submissions.
        return text.replace("\r\n", "\r").replace('\n', "\r").into_bytes();
    }

    // Never let pasted content inject a control sequence that can terminate
    // or escape its bracketed-paste envelope.
    let sanitized = text.replace('\x1b', "");
    let mut bytes = Vec::with_capacity(6 + text.len() + 6);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(sanitized.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

/// Encodes alternate-screen wheel steps as SS3 cursor keys.
#[must_use]
pub fn alternate_scroll(up: bool, lines: usize) -> Vec<u8> {
    let final_byte = if up { b'A' } else { b'B' };
    let mut bytes = Vec::with_capacity(lines.saturating_mul(3));
    for _ in 0..lines {
        bytes.extend_from_slice(&[ESC, b'O', final_byte]);
    }
    bytes
}

fn encode_csi_key(
    key: NamedKey,
    modifiers: Modifiers,
    application_cursor_keys: bool,
) -> Option<Vec<u8>> {
    let modifier = xterm_modifier(modifiers);
    let cursor = |final_byte| {
        if modifier == 1 {
            // Application-cursor support is an intentional extension: the
            // Swift encoder currently always emits CSI. Default modes preserve
            // its output exactly.
            if application_cursor_keys {
                ss3(final_byte)
            } else {
                csi_final(final_byte)
            }
        } else {
            csi_modified(modifier, final_byte)
        }
    };
    let tilde = |number| csi_tilde(number, modifier);
    let fkey = |final_byte| {
        if modifier == 1 {
            ss3(final_byte)
        } else {
            csi_modified(modifier, final_byte)
        }
    };

    // Ordered to match TerminalKeyEncoding.csiKey's Swift switch.
    match key {
        NamedKey::ArrowUp => Some(cursor(b'A')),
        NamedKey::ArrowDown => Some(cursor(b'B')),
        NamedKey::ArrowRight => Some(cursor(b'C')),
        NamedKey::ArrowLeft => Some(cursor(b'D')),
        NamedKey::Home => Some(cursor(b'H')),
        NamedKey::End => Some(cursor(b'F')),
        NamedKey::PageUp => Some(tilde(5)),
        NamedKey::PageDown => Some(tilde(6)),
        NamedKey::Insert => Some(tilde(2)),
        NamedKey::Delete => Some(tilde(3)),
        NamedKey::Tab if modifiers.shift => Some(b"\x1b[Z".to_vec()),
        NamedKey::Tab => None,
        NamedKey::Enter if modifiers == shift_only() => Some(vec![b'\n']),
        NamedKey::Enter => None,
        NamedKey::F1 => Some(fkey(b'P')),
        NamedKey::F2 => Some(fkey(b'Q')),
        NamedKey::F3 => Some(fkey(b'R')),
        NamedKey::F4 => Some(fkey(b'S')),
        NamedKey::F5 => Some(tilde(15)),
        NamedKey::F6 => Some(tilde(17)),
        NamedKey::F7 => Some(tilde(18)),
        NamedKey::F8 => Some(tilde(19)),
        NamedKey::F9 => Some(tilde(20)),
        NamedKey::F10 => Some(tilde(21)),
        NamedKey::F11 => Some(tilde(23)),
        NamedKey::F12 => Some(tilde(24)),
        NamedKey::F13 => Some(tilde(25)),
        NamedKey::F14 => Some(tilde(26)),
        NamedKey::F15 => Some(tilde(28)),
        NamedKey::F16 => Some(tilde(29)),
        NamedKey::F17 => Some(tilde(31)),
        NamedKey::F18 => Some(tilde(32)),
        NamedKey::F19 => Some(tilde(33)),
        NamedKey::F20 => Some(tilde(34)),
        NamedKey::Escape | NamedKey::Backspace => None,
    }
}

fn encode_meta_text(logical: &str, modifiers: Modifiers) -> Option<Vec<u8>> {
    let mut scalars = logical.chars();
    let scalar = scalars.next()?;
    if scalars.next().is_some() || !scalar.is_ascii() {
        return None;
    }

    let payload = if modifiers.ctrl {
        encode_control_text(&scalar.to_string()).unwrap_or_else(|| vec![scalar as u8])
    } else {
        let scalar = if modifiers.shift {
            scalar.to_ascii_uppercase()
        } else {
            scalar
        };
        vec![scalar as u8]
    };
    let mut bytes = Vec::with_capacity(1 + payload.len());
    bytes.push(ESC);
    bytes.extend_from_slice(&payload);
    Some(bytes)
}

const fn shift_only() -> Modifiers {
    Modifiers {
        shift: true,
        ctrl: false,
        alt: false,
        cmd: false,
    }
}

fn encode_control_text(logical: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut mapped_any = false;

    for scalar in logical.chars() {
        match scalar {
            'a'..='z' => bytes.push(scalar as u8 - b'a' + 1),
            'A'..='Z' => bytes.push(scalar as u8 - b'A' + 1),
            ' ' | '@' => bytes.push(0x00),
            '[' => bytes.push(0x1b),
            '\\' => bytes.push(0x1c),
            ']' => bytes.push(0x1d),
            // Swift's comment promises Ctrl-A...Ctrl-_ but its implementation
            // only spells out Space, `[`, `\\`, and `]`. T6 asks for the full
            // conventional punctuation set, so Ctrl-@/^/_ and Ctrl-? are
            // intentional extensions (NUL/RS/US/DEL respectively).
            '^' => bytes.push(0x1e),
            '_' => bytes.push(0x1f),
            '?' => bytes.push(0x7f),
            _ => {
                let mut encoded = [0; 4];
                bytes.extend_from_slice(scalar.encode_utf8(&mut encoded).as_bytes());
                continue;
            }
        }
        mapped_any = true;
    }

    mapped_any.then_some(bytes)
}

fn named_text(key: NamedKey) -> Option<&'static str> {
    match key {
        NamedKey::Tab => Some("\t"),
        NamedKey::Enter => Some("\r"),
        NamedKey::Escape => Some("\x1b"),
        NamedKey::Backspace => Some("\x7f"),
        _ => None,
    }
}

fn xterm_modifier(modifiers: Modifiers) -> u8 {
    1 + u8::from(modifiers.shift) + 2 * u8::from(modifiers.alt) + 4 * u8::from(modifiers.ctrl)
}

fn ss3(final_byte: u8) -> Vec<u8> {
    vec![ESC, b'O', final_byte]
}

fn csi_final(final_byte: u8) -> Vec<u8> {
    vec![ESC, b'[', final_byte]
}

fn csi_modified(modifier: u8, final_byte: u8) -> Vec<u8> {
    format!("\x1b[1;{modifier}{}", char::from(final_byte)).into_bytes()
}

fn csi_tilde(number: u8, modifier: u8) -> Vec<u8> {
    if modifier == 1 {
        format!("\x1b[{number}~").into_bytes()
    } else {
        format!("\x1b[{number};{modifier}~").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::io::{Read, Write};
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    use super::*;
    #[cfg(unix)]
    use zeus_pty::{Pty, PtySpec};

    const MODIFIER_CASES: [(&str, Modifiers, u8); 8] = [
        ("plain", modifiers(false, false, false, false), 1),
        ("shift", modifiers(true, false, false, false), 2),
        ("alt", modifiers(false, false, true, false), 3),
        ("shift-alt", modifiers(true, false, true, false), 4),
        ("ctrl", modifiers(false, true, false, false), 5),
        ("shift-ctrl", modifiers(true, true, false, false), 6),
        ("alt-ctrl", modifiers(false, true, true, false), 7),
        ("shift-alt-ctrl", modifiers(true, true, true, false), 8),
    ];

    const fn modifiers(shift: bool, ctrl: bool, alt: bool, cmd: bool) -> Modifiers {
        Modifiers {
            shift,
            ctrl,
            alt,
            cmd,
        }
    }

    fn event(key: NamedKey) -> KeyEvent {
        KeyEvent::named(key)
    }

    fn expected_csi(final_byte: u8, modifier: u8) -> Vec<u8> {
        if modifier == 1 {
            vec![ESC, b'[', final_byte]
        } else {
            format!("\x1b[1;{modifier}{}", char::from(final_byte)).into_bytes()
        }
    }

    fn expected_fkey(final_byte: u8, modifier: u8) -> Vec<u8> {
        if modifier == 1 {
            vec![ESC, b'O', final_byte]
        } else {
            format!("\x1b[1;{modifier}{}", char::from(final_byte)).into_bytes()
        }
    }

    fn expected_tilde(number: u8, modifier: u8) -> Vec<u8> {
        if modifier == 1 {
            format!("\x1b[{number}~").into_bytes()
        } else {
            format!("\x1b[{number};{modifier}~").into_bytes()
        }
    }

    #[test]
    fn xterm_modifier_matches_swift_table() {
        for (name, modifiers, expected) in MODIFIER_CASES {
            assert_eq!(xterm_modifier(modifiers), expected, "{name}");
        }
    }

    #[test]
    fn navigation_and_function_switch_matches_swift_top_to_bottom() {
        #[derive(Clone, Copy)]
        enum Sequence {
            Csi(u8),
            Tilde(u8),
            FKey(u8),
        }

        // This order is TerminalKeyEncoding.csiKey's switch order. Each row is
        // exercised with all eight xterm modifier values and with Command both
        // absent and present (Command is deliberately not an xterm modifier).
        let cases = [
            ("up", NamedKey::ArrowUp, Sequence::Csi(b'A')),
            ("down", NamedKey::ArrowDown, Sequence::Csi(b'B')),
            ("right", NamedKey::ArrowRight, Sequence::Csi(b'C')),
            ("left", NamedKey::ArrowLeft, Sequence::Csi(b'D')),
            ("home", NamedKey::Home, Sequence::Csi(b'H')),
            ("end", NamedKey::End, Sequence::Csi(b'F')),
            ("page-up", NamedKey::PageUp, Sequence::Tilde(5)),
            ("page-down", NamedKey::PageDown, Sequence::Tilde(6)),
            ("insert/help", NamedKey::Insert, Sequence::Tilde(2)),
            ("forward-delete", NamedKey::Delete, Sequence::Tilde(3)),
            ("f1", NamedKey::F1, Sequence::FKey(b'P')),
            ("f2", NamedKey::F2, Sequence::FKey(b'Q')),
            ("f3", NamedKey::F3, Sequence::FKey(b'R')),
            ("f4", NamedKey::F4, Sequence::FKey(b'S')),
            ("f5", NamedKey::F5, Sequence::Tilde(15)),
            ("f6", NamedKey::F6, Sequence::Tilde(17)),
            ("f7", NamedKey::F7, Sequence::Tilde(18)),
            ("f8", NamedKey::F8, Sequence::Tilde(19)),
            ("f9", NamedKey::F9, Sequence::Tilde(20)),
            ("f10", NamedKey::F10, Sequence::Tilde(21)),
            ("f11", NamedKey::F11, Sequence::Tilde(23)),
            ("f12", NamedKey::F12, Sequence::Tilde(24)),
            ("f13", NamedKey::F13, Sequence::Tilde(25)),
            ("f14", NamedKey::F14, Sequence::Tilde(26)),
            ("f15", NamedKey::F15, Sequence::Tilde(28)),
            ("f16", NamedKey::F16, Sequence::Tilde(29)),
            ("f17", NamedKey::F17, Sequence::Tilde(31)),
            ("f18", NamedKey::F18, Sequence::Tilde(32)),
            ("f19", NamedKey::F19, Sequence::Tilde(33)),
            ("f20", NamedKey::F20, Sequence::Tilde(34)),
        ];

        for (key_name, key, sequence) in cases {
            for (modifier_name, base_modifiers, modifier) in MODIFIER_CASES {
                for cmd in [false, true] {
                    let modifiers = Modifiers {
                        cmd,
                        ..base_modifiers
                    };
                    let expected = match sequence {
                        Sequence::Csi(final_byte) => expected_csi(final_byte, modifier),
                        Sequence::Tilde(number) => expected_tilde(number, modifier),
                        Sequence::FKey(final_byte) => expected_fkey(final_byte, modifier),
                    };
                    assert_eq!(
                        encode_key(&event(key), modifiers, TermInputModes::default()),
                        expected,
                        "{key_name}, {modifier_name}, cmd={cmd}"
                    );
                }
            }
        }
    }

    #[test]
    fn tab_and_enter_switch_cases_match_swift_for_every_modifier() {
        for (name, base_modifiers, _) in MODIFIER_CASES {
            for cmd in [false, true] {
                let modifiers = Modifiers {
                    cmd,
                    ..base_modifiers
                };

                let tab = if modifiers.shift {
                    b"\x1b[Z".to_vec()
                } else if modifiers.alt {
                    b"\x1b\t".to_vec()
                } else {
                    b"\t".to_vec()
                };
                assert_eq!(
                    encode_key(&event(NamedKey::Tab), modifiers, TermInputModes::default()),
                    tab,
                    "tab, {name}, cmd={cmd}"
                );

                let enter = if modifiers == shift_only() {
                    b"\n".to_vec()
                } else if modifiers.alt {
                    b"\x1b\r".to_vec()
                } else {
                    b"\r".to_vec()
                };
                assert_eq!(
                    encode_key(
                        &event(NamedKey::Enter),
                        modifiers,
                        TermInputModes::default()
                    ),
                    enter,
                    "enter, {name}, cmd={cmd}"
                );
            }
        }
    }

    #[test]
    fn application_cursor_mode_uses_ss3_only_for_unmodified_cursor_keys() {
        let cases = [
            (NamedKey::ArrowUp, b'A'),
            (NamedKey::ArrowDown, b'B'),
            (NamedKey::ArrowRight, b'C'),
            (NamedKey::ArrowLeft, b'D'),
            (NamedKey::Home, b'H'),
            (NamedKey::End, b'F'),
        ];

        for (key, final_byte) in cases {
            for (name, base_modifiers, modifier) in MODIFIER_CASES {
                for cmd in [false, true] {
                    let modifiers = Modifiers {
                        cmd,
                        ..base_modifiers
                    };
                    let modes = TermInputModes {
                        application_cursor_keys: true,
                        ..TermInputModes::default()
                    };
                    let expected = if modifier == 1 {
                        vec![ESC, b'O', final_byte]
                    } else {
                        expected_csi(final_byte, modifier)
                    };
                    assert_eq!(
                        encode_key(&event(key), modifiers, modes),
                        expected,
                        "{key:?}, {name}, cmd={cmd}"
                    );
                }
            }
        }
    }

    #[test]
    fn option_arrows_are_xterm_word_jump_sequences() {
        let alt = modifiers(false, false, true, false);
        assert_eq!(
            encode_key(&event(NamedKey::ArrowLeft), alt, TermInputModes::default()),
            b"\x1b[1;3D"
        );
        assert_eq!(
            encode_key(&event(NamedKey::ArrowRight), alt, TermInputModes::default()),
            b"\x1b[1;3C"
        );
    }

    #[test]
    fn named_control_keys_match_swift_for_every_modifier() {
        for (name, base_modifiers, _) in MODIFIER_CASES {
            for cmd in [false, true] {
                let modifiers = Modifiers {
                    cmd,
                    ..base_modifiers
                };
                let escape = if modifiers.alt {
                    b"\x1b\x1b".to_vec()
                } else {
                    b"\x1b".to_vec()
                };
                assert_eq!(
                    encode_key(
                        &event(NamedKey::Escape),
                        modifiers,
                        TermInputModes::default()
                    ),
                    escape,
                    "escape, {name}, cmd={cmd}"
                );

                let backspace = if modifiers.cmd && !modifiers.alt && !modifiers.ctrl {
                    vec![0x15]
                } else if modifiers.cmd {
                    Vec::new()
                } else if modifiers.ctrl && !modifiers.alt {
                    vec![0x08]
                } else if modifiers.alt {
                    vec![ESC, 0x7f]
                } else {
                    vec![0x7f]
                };
                assert_eq!(
                    encode_key(
                        &event(NamedKey::Backspace),
                        modifiers,
                        TermInputModes::default()
                    ),
                    backspace,
                    "backspace, {name}, cmd={cmd}"
                );
            }
        }
    }

    #[test]
    fn backspace_and_forward_delete_are_distinct_xterm_keys() {
        assert_eq!(
            encode_key(
                &event(NamedKey::Backspace),
                Modifiers::default(),
                TermInputModes::default()
            ),
            [0x7f]
        );
        assert_eq!(
            encode_key(
                &event(NamedKey::Delete),
                Modifiers::default(),
                TermInputModes::default()
            ),
            b"\x1b[3~"
        );
    }

    #[test]
    fn ctrl_letters_match_swift_a_through_z() {
        let ctrl = modifiers(false, true, false, false);
        let ctrl_alt = modifiers(false, true, true, false);
        let meta = TermInputModes {
            option_as_meta: true,
            ..TermInputModes::default()
        };

        for offset in 0_u8..26 {
            let lower = char::from(b'a' + offset).to_string();
            let upper = char::from(b'A' + offset).to_string();
            let expected = vec![offset + 1];
            assert_eq!(
                encode_key(&KeyEvent::character(lower), ctrl, TermInputModes::default()),
                expected,
                "lowercase offset {offset}"
            );
            assert_eq!(
                encode_key(&KeyEvent::character(upper), ctrl, TermInputModes::default()),
                expected,
                "uppercase offset {offset}"
            );
            assert_eq!(
                encode_key(
                    &KeyEvent::character(char::from(b'a' + offset).to_string()),
                    ctrl_alt,
                    TermInputModes::default()
                ),
                vec![b'a' + offset],
                "native Option policy keeps platform text at offset {offset}"
            );
            let mut meta_expected = vec![ESC];
            meta_expected.extend_from_slice(&expected);
            assert_eq!(
                encode_key(
                    &KeyEvent::character(char::from(b'a' + offset).to_string()),
                    ctrl_alt,
                    meta
                ),
                meta_expected,
                "Meta prefixes the logical control byte at offset {offset}"
            );
        }
    }

    #[test]
    fn ctrl_symbol_table_includes_all_c0_punctuation() {
        let ctrl = modifiers(false, true, false, false);
        let cases = [
            ("space", " ", 0x00),
            ("at", "@", 0x00),
            ("left-bracket", "[", 0x1b),
            ("backslash", "\\", 0x1c),
            ("right-bracket", "]", 0x1d),
            ("caret", "^", 0x1e),
            ("underscore", "_", 0x1f),
            ("question", "?", 0x7f),
        ];

        for (name, input, expected) in cases {
            assert_eq!(
                encode_key(&KeyEvent::character(input), ctrl, TermInputModes::default()),
                vec![expected],
                "{name}"
            );
        }
    }

    #[test]
    fn option_policy_selects_native_composed_text_or_meta_logical_key() {
        let alt = modifiers(false, false, true, false);
        let meta = TermInputModes {
            option_as_meta: true,
            ..TermInputModes::default()
        };

        assert_eq!(
            encode_key(
                &KeyEvent::composed("e", "é"),
                alt,
                TermInputModes::default()
            ),
            "é".as_bytes()
        );
        assert_eq!(
            encode_key(&KeyEvent::composed("e", "é"), alt, meta),
            b"\x1be"
        );
        assert_eq!(
            encode_key(&KeyEvent::composed("dead-key", "´e"), alt, meta),
            "´e".as_bytes(),
            "multi-scalar logical keys fall back to platform text"
        );
        assert_eq!(
            encode_key(
                &KeyEvent::composed("e", "É"),
                modifiers(true, false, true, false),
                meta
            ),
            b"\x1bE"
        );
    }

    #[test]
    fn printable_and_unmapped_control_text_pass_through_utf8() {
        assert_eq!(
            encode_key(
                &KeyEvent::character("λ🙂"),
                Modifiers::default(),
                TermInputModes::default()
            ),
            "λ🙂".as_bytes()
        );
        assert_eq!(
            encode_key(
                &KeyEvent::character("1"),
                modifiers(false, true, false, false),
                TermInputModes::default()
            ),
            b"1"
        );
        assert_eq!(
            encode_key(
                &KeyEvent::character("1"),
                modifiers(false, true, true, false),
                TermInputModes::default()
            ),
            b"1"
        );
        assert_eq!(
            encode_key(
                &KeyEvent::character("1"),
                modifiers(false, true, true, false),
                TermInputModes {
                    option_as_meta: true,
                    ..TermInputModes::default()
                }
            ),
            b"\x1b1"
        );
        assert_eq!(
            encode_key(
                &KeyEvent::character("a1"),
                modifiers(false, true, false, false),
                TermInputModes::default()
            ),
            b"\x011"
        );
        assert_eq!(
            encode_key(
                &KeyEvent {
                    key: Key::Character("x".into()),
                    text: None,
                },
                Modifiers::default(),
                TermInputModes::default()
            ),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn paste_matches_zed_line_normalization_and_bracket_safety() {
        let text = "one\r\ntwo\nλ";
        assert_eq!(paste(text, false), "one\rtwo\rλ".as_bytes());
        assert_eq!(
            paste("one\x1b[201~two\nλ", true),
            b"\x1b[200~one[201~two\n\xce\xbb\x1b[201~"
        );
        assert_eq!(paste("", false), b"");
        assert_eq!(paste("", true), b"\x1b[200~\x1b[201~");
    }

    #[test]
    fn alternate_scroll_repeats_ss3_cursor_keys() {
        assert_eq!(alternate_scroll(true, 3), b"\x1bOA\x1bOA\x1bOA");
        assert_eq!(alternate_scroll(false, 2), b"\x1bOB\x1bOB");
        assert!(alternate_scroll(true, 0).is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn encoded_matrix_reaches_a_raw_pty_exactly_once() {
        let plain = Modifiers::default();
        let ctrl = modifiers(false, true, false, false);
        let alt = modifiers(false, false, true, false);
        let cmd = modifiers(false, false, false, true);
        let application = TermInputModes {
            application_cursor_keys: true,
            ..TermInputModes::default()
        };
        let meta = TermInputModes {
            option_as_meta: true,
            ..TermInputModes::default()
        };
        let cases: Vec<(Vec<u8>, &[u8])> = vec![
            (
                encode_key(
                    &event(NamedKey::Backspace),
                    plain,
                    TermInputModes::default(),
                ),
                b"\x7f",
            ),
            (
                encode_key(&event(NamedKey::Backspace), ctrl, TermInputModes::default()),
                b"\x08",
            ),
            (
                encode_key(&event(NamedKey::Backspace), alt, TermInputModes::default()),
                b"\x1b\x7f",
            ),
            (
                encode_key(&event(NamedKey::Backspace), cmd, TermInputModes::default()),
                b"\x15",
            ),
            (
                encode_key(&event(NamedKey::Delete), plain, TermInputModes::default()),
                b"\x1b[3~",
            ),
            (
                encode_key(&event(NamedKey::ArrowUp), plain, TermInputModes::default()),
                b"\x1b[A",
            ),
            (
                encode_key(&event(NamedKey::ArrowUp), plain, application),
                b"\x1bOA",
            ),
            (
                encode_key(&event(NamedKey::Home), plain, application),
                b"\x1bOH",
            ),
            (
                encode_key(
                    &event(NamedKey::Tab),
                    modifiers(true, false, false, false),
                    TermInputModes::default(),
                ),
                b"\x1b[Z",
            ),
            (
                encode_key(
                    &event(NamedKey::Enter),
                    modifiers(true, false, false, false),
                    TermInputModes::default(),
                ),
                b"\n",
            ),
            (
                encode_key(&event(NamedKey::Escape), plain, TermInputModes::default()),
                b"\x1b",
            ),
            (
                encode_key(&event(NamedKey::F20), plain, TermInputModes::default()),
                b"\x1b[34~",
            ),
            (
                encode_key(&KeyEvent::character("c"), ctrl, TermInputModes::default()),
                b"\x03",
            ),
            (encode_key(&KeyEvent::character("x"), alt, meta), b"\x1bx"),
            (
                encode_key(
                    &KeyEvent::composed("e", "é"),
                    alt,
                    TermInputModes::default(),
                ),
                "é".as_bytes(),
            ),
            (
                encode_key(
                    &KeyEvent::composed("e", "é"),
                    plain,
                    TermInputModes::default(),
                ),
                "é".as_bytes(),
            ),
            // A held key is delivered as repeated key-down events. Two
            // encodes must remain two complete sequences at the PTY boundary.
            (
                encode_key(&event(NamedKey::ArrowRight), plain, application),
                b"\x1bOC",
            ),
            (
                encode_key(&event(NamedKey::ArrowRight), plain, application),
                b"\x1bOC",
            ),
            (paste("a\nb", false), b"a\rb"),
            (
                paste("pasted\x1b[31m", true),
                b"\x1b[200~pasted[31m\x1b[201~",
            ),
        ];

        let mut expected = Vec::new();
        for (actual, expected_case) in &cases {
            assert_eq!(actual, expected_case);
            expected.extend_from_slice(expected_case);
        }

        let count = expected.len().to_string();
        let script =
            "stty raw -echo; printf READY; dd bs=1 count=\"$1\" 2>/dev/null | od -An -t u1";
        let spec = PtySpec::new(
            vec![
                "/bin/sh".into(),
                "-c".into(),
                script.into(),
                "zeus-raw-input-fixture".into(),
                count,
            ],
            "/",
        )
        .env("PATH", "/usr/bin:/bin");
        let mut pty = Pty::spawn(&spec).expect("spawn raw PTY fixture");
        let mut reader = pty.reader().expect("PTY reader");
        reader.set_nonblocking(true).expect("nonblocking reader");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        while !output.ends_with(b"READY") {
            assert!(
                Instant::now() < deadline,
                "raw PTY fixture never became ready"
            );
            match reader.read_to_end(&mut output) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("read raw PTY readiness: {error}"),
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        let mut writer = pty.writer().expect("PTY writer");
        writer.write_all(&expected).expect("write encoded matrix");
        writer.flush().expect("flush encoded matrix");
        reader.set_nonblocking(false).expect("blocking reader");
        reader
            .read_to_end(&mut output)
            .expect("read child byte dump");
        assert_eq!(
            pty.wait().expect("wait for raw PTY fixture"),
            zeus_pty::Exit::Code(0)
        );

        let received: Vec<u8> = std::str::from_utf8(&output[b"READY".len()..])
            .expect("od output is ASCII")
            .split_whitespace()
            .map(|number| number.parse::<u8>().expect("od byte"))
            .collect();
        assert_eq!(received, expected);
    }
}
