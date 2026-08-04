//! The fuzzy matching primitive shared by the command palette and Quick Open.
//!
//! Backed by `nucleo-matcher` — the fzf-style Smith-Waterman matcher behind
//! Helix — instead of the greedy subsequence port this file used to hold. The
//! greedy version took the *first* place each query character fit, so it could
//! neither find the good alignment ("cc" never reached the second C of
//! `ClaudeCode` if an earlier c existed) nor rank two matches apart in any
//! meaningful way. nucleo computes the optimal alignment, gives us the matched
//! character positions for highlighting, and is dramatically faster on the
//! twenty-thousand-entry Quick Open pool because of its memchr prefilter.
//!
//! Queries go through `Pattern::parse`, so fzf syntax comes along for free:
//! space-separated words match independently and in any order (`code forge`),
//! `'word` is a literal substring, `^word` / `word$` anchor, and `!word`
//! excludes.

use std::ops::Range;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str, Utf32String};

/// Ranking score. Larger is better; magnitudes are nucleo's (a matched
/// character is worth 16, so a bonus of ~16 equals one extra matched letter).
pub type Score = u32;

/// A candidate converted once, before it enters a per-keystroke hot path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreparedText(Utf32String);

impl PreparedText {
    pub fn new(text: &str) -> Self {
        Self(Utf32String::from(text))
    }

    fn haystack(&self) -> Utf32Str<'_> {
        self.0.slice(..)
    }
}

/// Scratch state for scoring. `Matcher` eagerly allocates ~135 KB, so one is
/// built per ranking pass and reused across every candidate in it — never per
/// candidate and never per rendered row.
pub struct FuzzyMatcher {
    inner: Matcher,
}

impl FuzzyMatcher {
    /// Names, titles, and other short labels. `prefer_prefix` biases toward
    /// matches near the start, which is what "I am typing the thing I want"
    /// means in a palette.
    pub fn text() -> Self {
        let mut config = Config::DEFAULT;
        config.prefer_prefix = true;
        Self {
            inner: Matcher::new(config),
        }
    }

    /// Full paths: `/` becomes the boundary character, so `d/s/m` style queries
    /// land on segment starts.
    pub fn paths() -> Self {
        Self {
            inner: Matcher::new(Config::DEFAULT.match_paths()),
        }
    }
}

impl Default for FuzzyMatcher {
    fn default() -> Self {
        Self::text()
    }
}

/// A query parsed once per keystroke and scored against many candidates.
#[derive(Debug, Default)]
pub struct FuzzyQuery {
    pattern: Pattern,
    empty: bool,
}

impl FuzzyQuery {
    pub fn new(raw: &str) -> Self {
        let pattern = Pattern::parse(raw.trim(), CaseMatching::Smart, Normalization::Smart);
        Self {
            empty: pattern.atoms.is_empty(),
            pattern,
        }
    }

    /// True when the query has no atoms — every candidate matches with score 0.
    pub const fn is_empty(&self) -> bool {
        self.empty
    }

    pub fn score(&self, candidate: &PreparedText, matcher: &mut FuzzyMatcher) -> Option<Score> {
        self.pattern.score(candidate.haystack(), &mut matcher.inner)
    }

    /// Score plus the byte ranges of `original` that matched, merged into runs
    /// and ready for `StyledText::with_highlights`. `original` must be the text
    /// `candidate` was prepared from.
    pub fn highlights(
        &self,
        candidate: &PreparedText,
        original: &str,
        matcher: &mut FuzzyMatcher,
    ) -> Option<(Score, Vec<Range<usize>>)> {
        let mut indices = Vec::new();
        let score = self
            .pattern
            .indices(candidate.haystack(), &mut matcher.inner, &mut indices)?;
        indices.sort_unstable();
        indices.dedup();
        Some((score, byte_ranges(original, &indices)))
    }
}

/// Map nucleo's character indices to byte ranges over `text`, merging adjacent
/// characters so a run of matches becomes one highlight instead of five.
fn byte_ranges(text: &str, indices: &[u32]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut wanted = indices.iter().copied().peekable();
    for (position, (offset, character)) in text.char_indices().enumerate() {
        let Some(next) = wanted.peek().copied() else {
            break;
        };
        if next as usize != position {
            continue;
        }
        wanted.next();
        let end = offset + character.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == offset => last.end = end,
            _ => ranges.push(offset..end),
        }
    }
    ranges
}

/// One-shot scoring for small lists and tests. Allocates a matcher, so it must
/// not be called in a loop.
pub fn score(query: &str, candidate: &str) -> Option<Score> {
    FuzzyQuery::new(query).score(&PreparedText::new(candidate), &mut FuzzyMatcher::text())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranked<'a>(query: &str, candidates: &[&'a str]) -> Vec<&'a str> {
        let parsed = FuzzyQuery::new(query);
        let mut matcher = FuzzyMatcher::text();
        let mut scored: Vec<_> = candidates
            .iter()
            .filter_map(|candidate| {
                parsed
                    .score(&PreparedText::new(candidate), &mut matcher)
                    .map(|score| (*candidate, score))
            })
            .collect();
        scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        scored.into_iter().map(|(candidate, _)| candidate).collect()
    }

    #[test]
    fn empty_query_matches_everything_at_zero() {
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("   ", "anything"), Some(0));
        assert!(FuzzyQuery::new("  ").is_empty());
    }

    #[test]
    fn acronyms_find_the_optimal_alignment() {
        // The greedy predecessor consumed the leading `c` of `cachecontrol` and
        // then failed to reach a second capital, ranking it above `ClaudeCode`.
        assert_eq!(
            ranked("cc", &["cachecontrol", "ClaudeCode"]),
            ["ClaudeCode", "cachecontrol"]
        );
        assert!(score("qo", "QuickOpen").is_some());
        assert!(score("ncc", "New Claude Code Session").is_some());
    }

    #[test]
    fn words_match_independently_and_out_of_order() {
        assert!(score("forge codex", "New Codex on Forge").is_some());
        assert!(score("zz", "QuickOpen").is_none());
    }

    #[test]
    fn prefixes_and_boundaries_outrank_buried_matches() {
        assert_eq!(
            ranked("diri", &["anara-dirijor-calm-marten", "diri"]),
            ["diri", "anara-dirijor-calm-marten"]
        );
        assert_eq!(
            ranked("term", &["New Terminal", "Determinate"]),
            ["New Terminal", "Determinate"]
        );
    }

    #[test]
    fn highlights_cover_the_matched_characters_as_byte_ranges() {
        let query = FuzzyQuery::new("qo");
        let text = "QuickOpen";
        let (_, ranges) = query
            .highlights(&PreparedText::new(text), text, &mut FuzzyMatcher::text())
            .expect("match");
        assert_eq!(ranges, [0..1, 5..6]);

        // Adjacent characters merge into one run.
        let query = FuzzyQuery::new("quick");
        let (_, ranges) = query
            .highlights(&PreparedText::new(text), text, &mut FuzzyMatcher::text())
            .expect("match");
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], 0..5);
    }

    #[test]
    fn highlight_ranges_stay_on_character_boundaries() {
        let text = "naïve-café";
        let query = FuzzyQuery::new("ïé");
        let (_, ranges) = query
            .highlights(&PreparedText::new(text), text, &mut FuzzyMatcher::text())
            .expect("match");
        for range in &ranges {
            assert!(text.is_char_boundary(range.start));
            assert!(text.is_char_boundary(range.end));
        }
        assert_eq!(ranges.len(), 2);
    }
}
