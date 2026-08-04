//! Dotted numeric versions, compared the way a release feed needs them.
//!
//! Deliberately not a semver implementation: diri ships `MAJOR.MINOR.PATCH`
//! and macOS reports `15`, `15.5`, or `15.5.1`. Anything after a `-` or `+`
//! (prerelease/build metadata) is ignored rather than ordered, because the
//! feed never carries one and guessing at prerelease precedence would be a
//! silent source of wrong update decisions.

use std::cmp::Ordering;
use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    // Field order is the comparison order: derived Ord compares major, then
    // minor, then patch.
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parses `"0.4.2"`, `"15.5"`, or `"3"`. Returns `None` for anything with a
    /// non-numeric or empty component, so a malformed feed entry is skipped
    /// instead of being treated as version 0.
    pub fn parse(text: &str) -> Option<Self> {
        let core = text
            .trim()
            .trim_start_matches('v')
            .split(['-', '+'])
            .next()?
            .trim();
        if core.is_empty() {
            return None;
        }
        let mut parts = core.split('.');
        let mut component = || -> Option<u32> {
            match parts.next() {
                Some(part) => part.trim().parse().ok(),
                None => Some(0),
            }
        };
        let major = component()?;
        let minor = component()?;
        let patch = component()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }

    pub fn is_newer_than(self, other: Self) -> bool {
        self.cmp(&other) == Ordering::Greater
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_to_three_components() {
        assert_eq!(Version::parse("0.4.2"), Some(Version::new(0, 4, 2)));
        assert_eq!(Version::parse("15.5"), Some(Version::new(15, 5, 0)));
        assert_eq!(Version::parse("16"), Some(Version::new(16, 0, 0)));
        assert_eq!(Version::parse(" v1.2.3 "), Some(Version::new(1, 2, 3)));
    }

    #[test]
    fn ignores_prerelease_and_build_metadata() {
        assert_eq!(Version::parse("1.2.3-beta.1"), Some(Version::new(1, 2, 3)));
        assert_eq!(Version::parse("1.2.3+197"), Some(Version::new(1, 2, 3)));
    }

    #[test]
    fn rejects_garbage_instead_of_defaulting_to_zero() {
        assert_eq!(Version::parse(""), None);
        assert_eq!(Version::parse("latest"), None);
        assert_eq!(Version::parse("1.x.3"), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version::parse("-1.0.0"), None);
    }

    #[test]
    fn orders_by_component_significance() {
        assert!(Version::new(0, 5, 0).is_newer_than(Version::new(0, 4, 9)));
        assert!(Version::new(1, 0, 0).is_newer_than(Version::new(0, 99, 99)));
        assert!(Version::new(0, 4, 10).is_newer_than(Version::new(0, 4, 9)));
        assert!(!Version::new(0, 4, 2).is_newer_than(Version::new(0, 4, 2)));
        assert!(!Version::new(0, 4, 1).is_newer_than(Version::new(0, 4, 2)));
    }

    #[test]
    fn displays_all_three_components() {
        assert_eq!(Version::new(15, 5, 0).to_string(), "15.5.0");
    }
}
