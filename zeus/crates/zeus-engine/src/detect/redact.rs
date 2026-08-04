//! Masks obvious secrets and strips control sequences before text leaves the
//! engine.
//!
//! Prompt excerpts are shown in the sidebar, sent to the phone, and returned
//! through the MCP tools, so anything captured off a screen passes through
//! here first. Ported from the Swift `Redaction`.

use std::sync::LazyLock;

use regex::Regex;

static SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(token|secret|password|api[_-]?key|authorization)\s*[=:]\s*\S+")
        .expect("secret pattern compiles")
});

static ANSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[a-zA-Z]").expect("ansi pattern compiles"));

pub fn redact(text: &str) -> String {
    let masked = SECRET.replace_all(text, "$1=•••");
    ANSI.replace_all(&masked, "").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_secret_assignments_case_insensitively() {
        assert_eq!(redact("API_KEY: sk-abc123"), "API_KEY=•••");
        assert_eq!(redact("password=hunter2"), "password=•••");
        assert_eq!(
            redact("Authorization: Bearer xyz"),
            "Authorization=••• xyz",
            "only the first token after the separator is masked, as in Swift"
        );
    }

    #[test]
    fn strips_ansi_control_sequences() {
        assert_eq!(redact("\x1b[31mred\x1b[0m"), "red");
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        assert_eq!(redact("do you want to proceed?"), "do you want to proceed?");
    }
}
