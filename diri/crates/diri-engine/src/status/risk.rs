//! Risk classification for a pending permission prompt.
//!
//! Drives how loudly the UI asks for attention: a destructive command should
//! not look like a file write. Ported from the Swift `RiskHint.classify`;
//! the pattern lists and their order are the behavior, so they are kept
//! verbatim — first match wins.

use diri_proto::RiskHint;

const DESTRUCTIVE: &[&str] = &[
    "rm -rf",
    "rm -fr",
    "sudo ",
    "force-push",
    "push --force",
    "push -f",
    "reset --hard",
    "clean -fd",
    "drop table",
    "mkfs",
    "> /dev/",
];

const NETWORK: &[&str] = &[
    "curl ", "wget ", "http://", "https://", "ssh ", "scp ", "nc ",
];

const FILE_WRITE: &[&str] = &[
    "write", "edit", "mv ", "cp ", "mkdir", "touch ", "chmod", "chown",
];

pub fn classify_risk(text: &str) -> RiskHint {
    let lower = text.to_lowercase();
    if DESTRUCTIVE.iter().any(|pattern| lower.contains(pattern)) {
        return RiskHint::Destructive;
    }
    if NETWORK.iter().any(|pattern| lower.contains(pattern)) {
        return RiskHint::Network;
    }
    if FILE_WRITE.iter().any(|pattern| lower.contains(pattern)) {
        return RiskHint::FileWrite;
    }
    RiskHint::Neutral
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destructive_beats_everything_else_in_the_same_string() {
        // "rm -rf" and "https://" both appear; destructive is checked first and
        // that ordering is the point.
        assert_eq!(
            classify_risk("rm -rf /tmp && curl https://example.com"),
            RiskHint::Destructive
        );
    }

    #[test]
    fn classifies_each_tier() {
        assert_eq!(classify_risk("sudo reboot"), RiskHint::Destructive);
        assert_eq!(classify_risk("curl example.com"), RiskHint::Network);
        assert_eq!(classify_risk("edit src/main.rs"), RiskHint::FileWrite);
        assert_eq!(classify_risk("list the files"), RiskHint::Neutral);
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert_eq!(classify_risk("RM -RF build"), RiskHint::Destructive);
    }
}
