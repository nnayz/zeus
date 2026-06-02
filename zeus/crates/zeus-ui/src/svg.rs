use std::{error::Error, fmt};

/// A parsed SVG path operation in the source view-box coordinate space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathCommand {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadTo {
        control: (f32, f32),
        to: (f32, f32),
    },
    CubicTo {
        control_a: (f32, f32),
        control_b: (f32, f32),
        to: (f32, f32),
    },
    Close,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SvgPath {
    commands: Vec<PathCommand>,
}

impl SvgPath {
    /// Parses M/L/H/V/C/S/Q/T/A/Z, including relative and implicit commands.
    /// SVG elliptical arcs are converted to cubic Bézier segments.
    pub fn parse(data: &str) -> Result<Self, SvgPathError> {
        Parser::new(data).parse()
    }

    pub fn commands(&self) -> &[PathCommand] {
        &self.commands
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SvgPathError {
    offset: usize,
    message: &'static str,
}

impl SvgPathError {
    fn new(offset: usize, message: &'static str) -> Self {
        Self { offset, message }
    }
}

impl fmt::Display for SvgPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "SVG path error at byte {}: {}",
            self.offset, self.message
        )
    }
}

impl Error for SvgPathError {}

struct Parser<'a> {
    scanner: Scanner<'a>,
    commands: Vec<PathCommand>,
    current: (f64, f64),
    subpath_start: (f64, f64),
    previous_cubic_control: Option<(f64, f64)>,
    previous_quad_control: Option<(f64, f64)>,
    command: Option<u8>,
}

impl<'a> Parser<'a> {
    fn new(data: &'a str) -> Self {
        Self {
            scanner: Scanner::new(data),
            commands: Vec::new(),
            current: (0.0, 0.0),
            subpath_start: (0.0, 0.0),
            previous_cubic_control: None,
            previous_quad_control: None,
            command: None,
        }
    }

    fn parse(mut self) -> Result<SvgPath, SvgPathError> {
        loop {
            self.scanner.skip_separators();
            if self.scanner.is_done() {
                break;
            }

            if let Some(letter) = self.scanner.command_letter() {
                self.command = Some(letter);
            } else if !self.scanner.has_number() {
                return Err(self.scanner.error("expected a path command or number"));
            } else {
                self.command = match self.command {
                    Some(b'M') => Some(b'L'),
                    Some(b'm') => Some(b'l'),
                    repeated => repeated,
                };
            }

            let command = self
                .command
                .ok_or_else(|| self.scanner.error("path must start with a command"))?;
            let relative = command.is_ascii_lowercase();

            match command.to_ascii_lowercase() {
                b'm' => {
                    let point = self.point(relative)?;
                    self.current = point;
                    self.subpath_start = point;
                    self.commands
                        .push(PathCommand::MoveTo(f32p(point).0, f32p(point).1));
                    self.reset_controls();
                }
                b'l' => {
                    let point = self.point(relative)?;
                    self.commands
                        .push(PathCommand::LineTo(f32p(point).0, f32p(point).1));
                    self.current = point;
                    self.reset_controls();
                }
                b'h' => {
                    let x = self.scanner.number()?;
                    self.current.0 = if relative { self.current.0 + x } else { x };
                    self.commands.push(PathCommand::LineTo(
                        self.current.0 as f32,
                        self.current.1 as f32,
                    ));
                    self.reset_controls();
                }
                b'v' => {
                    let y = self.scanner.number()?;
                    self.current.1 = if relative { self.current.1 + y } else { y };
                    self.commands.push(PathCommand::LineTo(
                        self.current.0 as f32,
                        self.current.1 as f32,
                    ));
                    self.reset_controls();
                }
                b'c' => {
                    let control_a = self.point(relative)?;
                    let control_b = self.point(relative)?;
                    let to = self.point(relative)?;
                    self.commands.push(PathCommand::CubicTo {
                        control_a: f32p(control_a),
                        control_b: f32p(control_b),
                        to: f32p(to),
                    });
                    self.current = to;
                    self.previous_cubic_control = Some(control_b);
                    self.previous_quad_control = None;
                }
                b's' => {
                    let control_a = self
                        .previous_cubic_control
                        .map(|control| reflect(control, self.current))
                        .unwrap_or(self.current);
                    let control_b = self.point(relative)?;
                    let to = self.point(relative)?;
                    self.commands.push(PathCommand::CubicTo {
                        control_a: f32p(control_a),
                        control_b: f32p(control_b),
                        to: f32p(to),
                    });
                    self.current = to;
                    self.previous_cubic_control = Some(control_b);
                    self.previous_quad_control = None;
                }
                b'q' => {
                    let control = self.point(relative)?;
                    let to = self.point(relative)?;
                    self.commands.push(PathCommand::QuadTo {
                        control: f32p(control),
                        to: f32p(to),
                    });
                    self.current = to;
                    self.previous_quad_control = Some(control);
                    self.previous_cubic_control = None;
                }
                b't' => {
                    let control = self
                        .previous_quad_control
                        .map(|control| reflect(control, self.current))
                        .unwrap_or(self.current);
                    let to = self.point(relative)?;
                    self.commands.push(PathCommand::QuadTo {
                        control: f32p(control),
                        to: f32p(to),
                    });
                    self.current = to;
                    self.previous_quad_control = Some(control);
                    self.previous_cubic_control = None;
                }
                b'a' => {
                    let rx = self.scanner.number()?.abs();
                    let ry = self.scanner.number()?.abs();
                    let rotation = self.scanner.number()?;
                    let large_arc = self.scanner.flag()?;
                    let sweep = self.scanner.flag()?;
                    let to = self.point(relative)?;
                    add_arc(
                        &mut self.commands,
                        self.current,
                        to,
                        rx,
                        ry,
                        rotation,
                        large_arc,
                        sweep,
                    );
                    self.current = to;
                    self.reset_controls();
                }
                b'z' => {
                    self.commands.push(PathCommand::Close);
                    self.current = self.subpath_start;
                    self.command = None;
                    self.reset_controls();
                }
                _ => return Err(self.scanner.error("unsupported path command")),
            }
        }

        Ok(SvgPath {
            commands: self.commands,
        })
    }

    fn point(&mut self, relative: bool) -> Result<(f64, f64), SvgPathError> {
        let x = self.scanner.number()?;
        let y = self.scanner.number()?;
        Ok(if relative {
            (self.current.0 + x, self.current.1 + y)
        } else {
            (x, y)
        })
    }

    fn reset_controls(&mut self) {
        self.previous_cubic_control = None;
        self.previous_quad_control = None;
    }
}

fn f32p(point: (f64, f64)) -> (f32, f32) {
    (point.0 as f32, point.1 as f32)
}

fn reflect(point: (f64, f64), around: (f64, f64)) -> (f64, f64) {
    (2.0 * around.0 - point.0, 2.0 * around.1 - point.1)
}

#[allow(clippy::too_many_arguments)]
fn add_arc(
    commands: &mut Vec<PathCommand>,
    from: (f64, f64),
    to: (f64, f64),
    mut rx: f64,
    mut ry: f64,
    rotation_degrees: f64,
    large_arc: bool,
    sweep: bool,
) {
    if from == to {
        return;
    }
    if rx == 0.0 || ry == 0.0 {
        commands.push(PathCommand::LineTo(to.0 as f32, to.1 as f32));
        return;
    }

    let phi = rotation_degrees.to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();
    let dx = (from.0 - to.0) / 2.0;
    let dy = (from.1 - to.1) / 2.0;
    let x1p = cos_phi * dx + sin_phi * dy;
    let y1p = -sin_phi * dx + cos_phi * dy;

    let lambda = x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry);
    if lambda > 1.0 {
        let scale = lambda.sqrt();
        rx *= scale;
        ry *= scale;
    }

    let numerator = (rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p).max(0.0);
    let denominator = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    if denominator == 0.0 {
        commands.push(PathCommand::LineTo(to.0 as f32, to.1 as f32));
        return;
    }
    let sign = if large_arc != sweep { 1.0 } else { -1.0 };
    let coefficient = sign * (numerator / denominator).sqrt();
    let cxp = coefficient * rx * y1p / ry;
    let cyp = coefficient * -ry * x1p / rx;
    let cx = cos_phi * cxp - sin_phi * cyp + (from.0 + to.0) / 2.0;
    let cy = sin_phi * cxp + cos_phi * cyp + (from.1 + to.1) / 2.0;

    let angle = |u: (f64, f64), v: (f64, f64)| {
        let dot = u.0 * v.0 + u.1 * v.1;
        let length = ((u.0 * u.0 + u.1 * u.1) * (v.0 * v.0 + v.1 * v.1)).sqrt();
        let mut value = (dot / length).clamp(-1.0, 1.0).acos();
        if u.0 * v.1 - u.1 * v.0 < 0.0 {
            value = -value;
        }
        value
    };

    let start_vector = ((x1p - cxp) / rx, (y1p - cyp) / ry);
    let end_vector = ((-x1p - cxp) / rx, (-y1p - cyp) / ry);
    let theta_start = angle((1.0, 0.0), start_vector);
    let mut theta_delta = angle(start_vector, end_vector);
    if !sweep && theta_delta > 0.0 {
        theta_delta -= std::f64::consts::TAU;
    }
    if sweep && theta_delta < 0.0 {
        theta_delta += std::f64::consts::TAU;
    }

    let segment_count = (theta_delta.abs() / std::f64::consts::FRAC_PI_2)
        .ceil()
        .max(1.0) as usize;
    let delta = theta_delta / segment_count as f64;
    let tangent = 4.0 / 3.0 * (delta / 4.0).tan();

    let map = |x: f64, y: f64| {
        (
            cx + cos_phi * rx * x - sin_phi * ry * y,
            cy + sin_phi * rx * x + cos_phi * ry * y,
        )
    };

    let mut segment_start = theta_start;
    for _ in 0..segment_count {
        let segment_end = segment_start + delta;
        let (sin_a, cos_a) = segment_start.sin_cos();
        let (sin_b, cos_b) = segment_end.sin_cos();
        let control_a = map(cos_a - tangent * sin_a, sin_a + tangent * cos_a);
        let control_b = map(cos_b + tangent * sin_b, sin_b - tangent * cos_b);
        let end = map(cos_b, sin_b);
        commands.push(PathCommand::CubicTo {
            control_a: f32p(control_a),
            control_b: f32p(control_b),
            to: f32p(end),
        });
        segment_start = segment_end;
    }
}

struct Scanner<'a> {
    data: &'a [u8],
    index: usize,
}

impl<'a> Scanner<'a> {
    fn new(data: &'a str) -> Self {
        Self {
            data: data.as_bytes(),
            index: 0,
        }
    }

    fn is_done(&self) -> bool {
        self.index >= self.data.len()
    }

    fn error(&self, message: &'static str) -> SvgPathError {
        SvgPathError::new(self.index, message)
    }

    fn skip_separators(&mut self) {
        while let Some(byte) = self.data.get(self.index)
            && (byte.is_ascii_whitespace() || *byte == b',')
        {
            self.index += 1;
        }
    }

    fn command_letter(&mut self) -> Option<u8> {
        self.skip_separators();
        let byte = *self.data.get(self.index)?;
        if byte.is_ascii_alphabetic() && byte != b'e' && byte != b'E' {
            self.index += 1;
            Some(byte)
        } else {
            None
        }
    }

    fn has_number(&self) -> bool {
        let mut index = self.index;
        while let Some(byte) = self.data.get(index)
            && (byte.is_ascii_whitespace() || *byte == b',')
        {
            index += 1;
        }
        self.data
            .get(index)
            .is_some_and(|byte| byte.is_ascii_digit() || matches!(*byte, b'+' | b'-' | b'.'))
    }

    fn flag(&mut self) -> Result<bool, SvgPathError> {
        self.skip_separators();
        match self.data.get(self.index) {
            Some(b'0') => {
                self.index += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.index += 1;
                Ok(true)
            }
            _ => Err(self.error("arc flag must be 0 or 1")),
        }
    }

    fn number(&mut self) -> Result<f64, SvgPathError> {
        self.skip_separators();
        let start = self.index;
        if matches!(self.data.get(self.index), Some(b'+') | Some(b'-')) {
            self.index += 1;
        }

        let mut digits = 0;
        while self.data.get(self.index).is_some_and(u8::is_ascii_digit) {
            self.index += 1;
            digits += 1;
        }
        if self.data.get(self.index) == Some(&b'.') {
            self.index += 1;
            while self.data.get(self.index).is_some_and(u8::is_ascii_digit) {
                self.index += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            return Err(self.error("expected a number"));
        }

        if matches!(self.data.get(self.index), Some(b'e') | Some(b'E')) {
            self.index += 1;
            if matches!(self.data.get(self.index), Some(b'+') | Some(b'-')) {
                self.index += 1;
            }
            let exponent_start = self.index;
            while self.data.get(self.index).is_some_and(u8::is_ascii_digit) {
                self.index += 1;
            }
            if exponent_start == self.index {
                return Err(self.error("expected exponent digits"));
            }
        }

        std::str::from_utf8(&self.data[start..self.index])
            .ok()
            .and_then(|number| number.parse().ok())
            .ok_or_else(|| self.error("invalid number"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relative_packed_and_implicit_commands() {
        let path = SvgPath::parse("m1 2 3-4h2v3z").unwrap();
        assert_eq!(
            path.commands(),
            &[
                PathCommand::MoveTo(1.0, 2.0),
                PathCommand::LineTo(4.0, -2.0),
                PathCommand::LineTo(6.0, -2.0),
                PathCommand::LineTo(6.0, 1.0),
                PathCommand::Close,
            ]
        );
    }

    #[test]
    fn reflects_smooth_controls() {
        let path = SvgPath::parse("M0 0 C1 0 2 0 3 0 S5 0 6 0 Q7 1 8 0 T10 0").unwrap();
        assert_eq!(
            path.commands()[2],
            PathCommand::CubicTo {
                control_a: (4.0, 0.0),
                control_b: (5.0, 0.0),
                to: (6.0, 0.0),
            }
        );
        assert_eq!(
            path.commands()[4],
            PathCommand::QuadTo {
                control: (9.0, -1.0),
                to: (10.0, 0.0),
            }
        );
    }

    #[test]
    fn converts_arcs_to_cubic_segments() {
        let path = SvgPath::parse("M0 0 A10 10 0 0 1 20 0").unwrap();
        assert!(path.commands().len() >= 3);
        assert!(
            path.commands()[1..]
                .iter()
                .all(|command| matches!(command, PathCommand::CubicTo { .. }))
        );
        match path.commands().last().unwrap() {
            PathCommand::CubicTo { to, .. } => {
                assert!((to.0 - 20.0).abs() < 0.001);
                assert!(to.1.abs() < 0.001);
            }
            _ => panic!("arc did not end with a cubic"),
        }
    }
}
