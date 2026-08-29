//! Shared PTY primitives used by the local Engine and remote Holder.

pub use zeus_pty::*;

/// Make terminal color capabilities explicit for a PTY child.
///
/// Zeus can be launched from a GUI or long-lived daemon without `TERM`, while
/// an inherited shell environment can carry values that describe a different
/// terminal. Remove those inputs before asserting the capabilities provided by
/// Zeus's terminal renderer.
pub(crate) fn assert_color_environment(environment: &mut Vec<(String, String)>) {
    environment.retain(|(name, _)| !matches!(name.as_str(), "NO_COLOR" | "TERM" | "COLORTERM"));
    environment.push(("TERM".into(), "xterm-256color".into()));
    environment.push(("COLORTERM".into(), "truecolor".into()));
}

#[cfg(test)]
mod tests {
    use super::assert_color_environment;

    #[test]
    fn color_environment_is_complete_without_inherited_terminal_values() {
        let mut environment = vec![("PATH".into(), "/usr/bin:/bin".into())];

        assert_color_environment(&mut environment);

        assert_eq!(
            environment,
            [
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("TERM".into(), "xterm-256color".into()),
                ("COLORTERM".into(), "truecolor".into()),
            ]
        );
    }

    #[test]
    fn color_environment_replaces_stale_values_and_removes_opt_outs() {
        let mut environment = vec![
            ("TERM".into(), "dumb".into()),
            ("COLORTERM".into(), "24bit".into()),
            ("NO_COLOR".into(), "1".into()),
            ("TERM".into(), "vt100".into()),
        ];

        assert_color_environment(&mut environment);

        assert_eq!(
            environment,
            [
                ("TERM".into(), "xterm-256color".into()),
                ("COLORTERM".into(), "truecolor".into()),
            ]
        );
    }
}
