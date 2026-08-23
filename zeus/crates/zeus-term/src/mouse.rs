//! Platform-independent terminal mouse report encoding.
//!
//! Callers translate their UI toolkit's pointer events into the small Zeus
//! types here. Cell coordinates are always zero-based; terminal protocols are
//! converted to their one-based wire representation only while encoding.

use std::iter::repeat_n;

/// Mouse modes currently advertised by the foreground terminal application.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MouseModes {
    /// Whether the application requested any mouse reporting.
    pub reporting: bool,
    /// SGR extended-coordinate encoding (DECSET 1006).
    pub sgr: bool,
    /// UTF-8 extended-coordinate encoding (DECSET 1005).
    pub utf8: bool,
    /// Button-motion reporting (DECSET 1002).
    pub drag: bool,
    /// All-motion reporting (DECSET 1003).
    pub motion: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MouseModifiers {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MouseFormat {
    Sgr,
    Normal { utf8: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum ReportButton {
    Left = 0,
    Middle = 1,
    Right = 2,
    LeftMove = 32,
    MiddleMove = 33,
    RightMove = 34,
    NoneMove = 35,
    ScrollUp = 64,
    ScrollDown = 65,
}

impl MouseModes {
    const fn format(self) -> MouseFormat {
        if self.sgr {
            MouseFormat::Sgr
        } else {
            MouseFormat::Normal { utf8: self.utf8 }
        }
    }
}

impl MouseButton {
    const fn press_code(self) -> ReportButton {
        match self {
            Self::Left => ReportButton::Left,
            Self::Middle => ReportButton::Middle,
            Self::Right => ReportButton::Right,
        }
    }

    const fn motion_code(self) -> ReportButton {
        match self {
            Self::Left => ReportButton::LeftMove,
            Self::Middle => ReportButton::MiddleMove,
            Self::Right => ReportButton::RightMove,
        }
    }
}

impl MouseModifiers {
    const fn wire_bits(self) -> u8 {
        (if self.shift { 4 } else { 0 })
            + (if self.alt { 8 } else { 0 })
            + (if self.control { 16 } else { 0 })
    }
}

/// Encodes a button press at a zero-based terminal cell.
#[must_use]
pub fn press_report(
    col: usize,
    row: usize,
    button: MouseButton,
    modifiers: MouseModifiers,
    modes: MouseModes,
) -> Option<Vec<u8>> {
    report(col, row, button.press_code(), true, modifiers, modes)
}

/// Encodes a button release at a zero-based terminal cell.
///
/// SGR identifies the released button and terminates with `m`. Legacy normal
/// and UTF-8 formats use the protocol's generic release button code 3.
#[must_use]
pub fn release_report(
    col: usize,
    row: usize,
    button: MouseButton,
    modifiers: MouseModifiers,
    modes: MouseModes,
) -> Option<Vec<u8>> {
    report(col, row, button.press_code(), false, modifiers, modes)
}

/// Encodes pointer motion at a zero-based terminal cell.
///
/// `pressed_button` is the button currently held, if any. Drag mode suppresses
/// unpressed motion; all-motion mode accepts it and emits button code 35.
#[must_use]
pub fn motion_report(
    col: usize,
    row: usize,
    pressed_button: Option<MouseButton>,
    modifiers: MouseModifiers,
    modes: MouseModes,
) -> Option<Vec<u8>> {
    if !modes.reporting || !(modes.drag || modes.motion) {
        return None;
    }
    if modes.drag && pressed_button.is_none() {
        return None;
    }
    let button = pressed_button.map_or(ReportButton::NoneMove, MouseButton::motion_code);
    report(col, row, button, true, modifiers, modes)
}

/// Encodes `lines` identical wheel reports at a zero-based terminal cell.
///
/// `up` selects X11 wheel button 4; `false` selects button 5. An enabled mode
/// with zero lines returns an empty iterator, while disabled or out-of-range
/// coordinates return `None`.
#[must_use]
pub fn wheel_reports(
    col: usize,
    row: usize,
    up: bool,
    lines: usize,
    modifiers: MouseModifiers,
    modes: MouseModes,
) -> Option<impl Iterator<Item = Vec<u8>>> {
    let button = if up {
        ReportButton::ScrollUp
    } else {
        ReportButton::ScrollDown
    };
    let report = report(col, row, button, true, modifiers, modes)?;
    Some(repeat_n(report, lines))
}

fn report(
    col: usize,
    row: usize,
    button: ReportButton,
    pressed: bool,
    modifiers: MouseModifiers,
    modes: MouseModes,
) -> Option<Vec<u8>> {
    if !modes.reporting {
        return None;
    }
    let modifier_bits = modifiers.wire_bits();
    match modes.format() {
        MouseFormat::Sgr => sgr_report(
            col,
            row,
            (button as u8).checked_add(modifier_bits)?,
            pressed,
        ),
        MouseFormat::Normal { utf8 } => {
            let button = if pressed {
                (button as u8).checked_add(modifier_bits)?
            } else {
                3_u8.checked_add(modifier_bits)?
            };
            normal_report(col, row, button, utf8)
        }
    }
}

fn sgr_report(col: usize, row: usize, button: u8, pressed: bool) -> Option<Vec<u8>> {
    let col = col.checked_add(1)?;
    let row = row.checked_add(1)?;
    let terminator = if pressed { 'M' } else { 'm' };
    Some(format!("\x1b[<{button};{col};{row}{terminator}").into_bytes())
}

fn normal_report(col: usize, row: usize, button: u8, utf8: bool) -> Option<Vec<u8>> {
    let max_coordinate = if utf8 { 2015 } else { 223 };
    if col >= max_coordinate || row >= max_coordinate {
        return None;
    }

    let mut report = vec![b'\x1b', b'[', b'M', 32_u8.checked_add(button)?];
    encode_normal_coordinate(col, utf8, &mut report)?;
    encode_normal_coordinate(row, utf8, &mut report)?;
    Some(report)
}

fn encode_normal_coordinate(coordinate: usize, utf8: bool, output: &mut Vec<u8>) -> Option<()> {
    let encoded = coordinate.checked_add(33)?;
    if utf8 && coordinate >= 95 {
        output.push(u8::try_from(0xc0 + encoded / 64).ok()?);
        output.push(u8::try_from(0x80 + (encoded & 63)).ok()?);
    } else {
        output.push(u8::try_from(encoded).ok()?);
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORMAL: MouseModes = MouseModes {
        reporting: true,
        sgr: false,
        utf8: false,
        drag: false,
        motion: false,
    };
    const SGR: MouseModes = MouseModes {
        sgr: true,
        ..NORMAL
    };
    const UTF8: MouseModes = MouseModes {
        utf8: true,
        ..NORMAL
    };

    #[test]
    fn disabled_reporting_suppresses_every_report_kind() {
        let disabled = MouseModes::default();
        assert_eq!(
            press_report(0, 0, MouseButton::Left, MouseModifiers::default(), disabled),
            None
        );
        assert_eq!(
            release_report(0, 0, MouseButton::Left, MouseModifiers::default(), disabled),
            None
        );
        assert_eq!(
            motion_report(
                0,
                0,
                Some(MouseButton::Left),
                MouseModifiers::default(),
                MouseModes {
                    drag: true,
                    ..disabled
                }
            ),
            None
        );
        assert!(wheel_reports(0, 0, true, 1, MouseModifiers::default(), disabled).is_none());
    }

    #[test]
    fn sgr_press_and_release_keep_button_identity_and_use_one_based_coordinates() {
        let modifiers = MouseModifiers {
            shift: true,
            alt: true,
            control: true,
        };
        assert_eq!(
            press_report(4, 6, MouseButton::Left, modifiers, SGR),
            Some(b"\x1b[<28;5;7M".to_vec())
        );
        assert_eq!(
            release_report(4, 6, MouseButton::Right, modifiers, SGR),
            Some(b"\x1b[<30;5;7m".to_vec())
        );
        assert_eq!(
            press_report(0, 0, MouseButton::Middle, MouseModifiers::default(), SGR),
            Some(b"\x1b[<1;1;1M".to_vec())
        );
    }

    #[test]
    fn legacy_normal_uses_generic_release_and_modifier_bits() {
        assert_eq!(
            press_report(0, 0, MouseButton::Left, MouseModifiers::default(), NORMAL),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        let modifiers = MouseModifiers {
            shift: true,
            alt: true,
            control: true,
        };
        assert_eq!(
            press_report(2, 3, MouseButton::Right, modifiers, NORMAL),
            Some(vec![0x1b, b'[', b'M', 62, 35, 36])
        );
        assert_eq!(
            release_report(2, 3, MouseButton::Right, modifiers, NORMAL),
            Some(vec![0x1b, b'[', b'M', 63, 35, 36])
        );
    }

    #[test]
    fn modifier_bits_are_independent() {
        for (modifiers, expected_code) in [
            (
                MouseModifiers {
                    shift: true,
                    ..MouseModifiers::default()
                },
                4,
            ),
            (
                MouseModifiers {
                    alt: true,
                    ..MouseModifiers::default()
                },
                8,
            ),
            (
                MouseModifiers {
                    control: true,
                    ..MouseModifiers::default()
                },
                16,
            ),
        ] {
            assert_eq!(
                press_report(0, 0, MouseButton::Left, modifiers, SGR),
                Some(format!("\x1b[<{expected_code};1;1M").into_bytes())
            );
            assert_eq!(
                press_report(0, 0, MouseButton::Left, modifiers, NORMAL),
                Some(vec![0x1b, b'[', b'M', 32 + expected_code, 33, 33])
            );
        }
    }

    #[test]
    fn legacy_and_utf8_coordinate_boundaries_match_xterm() {
        let no_modifiers = MouseModifiers::default();
        assert_eq!(
            press_report(222, 222, MouseButton::Left, no_modifiers, NORMAL),
            Some(vec![0x1b, b'[', b'M', 32, 255, 255])
        );
        assert!(press_report(223, 0, MouseButton::Left, no_modifiers, NORMAL).is_none());
        assert!(press_report(0, 223, MouseButton::Left, no_modifiers, NORMAL).is_none());

        assert_eq!(
            press_report(94, 94, MouseButton::Left, no_modifiers, UTF8),
            Some(vec![0x1b, b'[', b'M', 32, 127, 127])
        );
        assert_eq!(
            press_report(95, 95, MouseButton::Left, no_modifiers, UTF8),
            Some(vec![0x1b, b'[', b'M', 32, 0xc2, 0x80, 0xc2, 0x80])
        );
        assert_eq!(
            press_report(2014, 2014, MouseButton::Left, no_modifiers, UTF8),
            Some(vec![0x1b, b'[', b'M', 32, 0xdf, 0xbf, 0xdf, 0xbf])
        );
        assert!(press_report(2015, 0, MouseButton::Left, no_modifiers, UTF8).is_none());
        assert!(press_report(0, 2015, MouseButton::Left, no_modifiers, UTF8).is_none());
    }

    #[test]
    fn sgr_takes_precedence_and_supports_extended_coordinates() {
        let modes = MouseModes { utf8: true, ..SGR };
        assert_eq!(
            press_report(
                5_000,
                6_000,
                MouseButton::Right,
                MouseModifiers::default(),
                modes
            ),
            Some(b"\x1b[<2;5001;6001M".to_vec())
        );
        assert!(
            press_report(
                usize::MAX,
                0,
                MouseButton::Left,
                MouseModifiers::default(),
                modes
            )
            .is_none()
        );
    }

    #[test]
    fn motion_distinguishes_drag_and_all_motion_modes() {
        let drag = MouseModes { drag: true, ..SGR };
        assert_eq!(
            motion_report(
                2,
                4,
                Some(MouseButton::Left),
                MouseModifiers::default(),
                drag
            ),
            Some(b"\x1b[<32;3;5M".to_vec())
        );
        assert_eq!(
            motion_report(2, 4, None, MouseModifiers::default(), drag),
            None
        );

        let motion = MouseModes {
            motion: true,
            ..SGR
        };
        assert_eq!(
            motion_report(2, 4, None, MouseModifiers::default(), motion),
            Some(b"\x1b[<35;3;5M".to_vec())
        );
        assert_eq!(
            motion_report(
                2,
                4,
                Some(MouseButton::Middle),
                MouseModifiers {
                    control: true,
                    ..MouseModifiers::default()
                },
                motion
            ),
            Some(b"\x1b[<49;3;5M".to_vec())
        );
        assert_eq!(
            motion_report(
                2,
                4,
                Some(MouseButton::Right),
                MouseModifiers::default(),
                SGR
            ),
            None
        );
    }

    #[test]
    fn drag_suppression_wins_if_drag_and_motion_are_both_set() {
        let modes = MouseModes {
            drag: true,
            motion: true,
            ..SGR
        };
        assert_eq!(
            motion_report(0, 0, None, MouseModifiers::default(), modes),
            None
        );
    }

    #[test]
    fn wheel_reports_repeat_direction_and_modifiers() {
        let modifiers = MouseModifiers {
            shift: true,
            ..MouseModifiers::default()
        };
        let up: Vec<_> = wheel_reports(3, 7, true, 3, modifiers, SGR)
            .expect("mouse reporting is enabled")
            .collect();
        assert_eq!(up, vec![b"\x1b[<68;4;8M".to_vec(); 3]);

        let down: Vec<_> = wheel_reports(0, 0, false, 2, MouseModifiers::default(), NORMAL)
            .expect("mouse reporting is enabled")
            .collect();
        assert_eq!(down, vec![vec![0x1b, b'[', b'M', 97, 33, 33]; 2]);

        assert_eq!(
            wheel_reports(0, 0, true, 0, MouseModifiers::default(), SGR)
                .expect("enabled zero-line wheel is an empty iterator")
                .count(),
            0
        );
    }
}
