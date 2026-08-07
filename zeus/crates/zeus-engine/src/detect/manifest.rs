//! Manifest schema: the data an agent's detection rules are written in.
//!
//! Deserialization mirrors the Swift `ManifestSchema` field for field, since
//! both read the same files.

use std::fmt;

use regex::Regex;
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RegionKind {
    OscTitle,
    OscProgress,
    WholeRecent,
    PromptBoxBody,
    BottomNonEmptyLines,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum ManifestState {
    Working,
    Idle,
    BlockedPermission,
    BlockedQuestion,
    /// The screen shows a transient view (transcript viewer, model picker) —
    /// hold the current state rather than transitioning.
    Skip,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StatusModel {
    Full,
    ProcessOnly,
}

/// What a predicate is evaluated against.
pub struct PredicateContext<'a> {
    pub text: &'a str,
    /// Case-folded once per region per evaluation; `Contains` needles are
    /// folded at load, so the substring check itself allocates nothing.
    pub text_lower: &'a str,
    pub lines: &'a [String],
    pub progress_state: Option<i64>,
}

/// A recursive detection predicate. Patterns compile once, at load.
#[derive(Debug)]
pub enum Predicate {
    /// Case-insensitive substring over the region's joined text. The stored
    /// needle is lowercased at deserialize.
    Contains(String),
    /// Matches anywhere in the region's joined text.
    Regex(Regex),
    /// Matches if any single line matches.
    LineRegex(Regex),
    /// Matches the OSC progress state.
    Progress(i64),
    Any(Vec<Predicate>),
    All(Vec<Predicate>),
    Not(Box<Predicate>),
}

impl Predicate {
    pub fn evaluate(&self, context: &PredicateContext) -> bool {
        match self {
            Predicate::Contains(needle) => context.text_lower.contains(needle),
            Predicate::Regex(regex) => regex.is_match(context.text),
            Predicate::LineRegex(regex) => context.lines.iter().any(|line| regex.is_match(line)),
            Predicate::Progress(state) => context.progress_state == Some(*state),
            Predicate::Any(inner) => inner.iter().any(|p| p.evaluate(context)),
            Predicate::All(inner) => inner.iter().all(|p| p.evaluate(context)),
            Predicate::Not(inner) => !inner.evaluate(context),
        }
    }
}

impl<'de> Deserialize<'de> for Predicate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PredicateVisitor;

        #[derive(Deserialize)]
        struct ProgressSpec {
            state: i64,
        }

        impl<'de> Visitor<'de> for PredicateVisitor {
            type Value = Predicate;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a predicate object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Predicate, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut found: Option<Predicate> = None;
                while let Some(key) = map.next_key::<String>()? {
                    let predicate = match key.as_str() {
                        "contains" => {
                            Predicate::Contains(map.next_value::<String>()?.to_lowercase())
                        }
                        "regex" => Predicate::Regex(compile::<M>(map.next_value()?)?),
                        "lineRegex" => Predicate::LineRegex(compile::<M>(map.next_value()?)?),
                        "progress" => {
                            let spec: ProgressSpec = map.next_value()?;
                            Predicate::Progress(spec.state)
                        }
                        "any" => Predicate::Any(map.next_value()?),
                        "all" => Predicate::All(map.next_value()?),
                        "not" => Predicate::Not(Box::new(map.next_value()?)),
                        other => {
                            return Err(de::Error::custom(format!(
                                "unrecognized predicate key {other:?}"
                            )));
                        }
                    };
                    found = Some(predicate);
                }
                found.ok_or_else(|| de::Error::custom("predicate object had no recognized key"))
            }
        }

        fn compile<'de, M: MapAccess<'de>>(pattern: String) -> Result<Regex, M::Error> {
            Regex::new(&pattern).map_err(|error| {
                de::Error::custom(format!("pattern {pattern:?} did not compile: {error}"))
            })
        }

        deserializer.deserialize_map(PredicateVisitor)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capture {
    pub region: RegionKind,
    #[serde(default = "default_region_lines")]
    pub region_lines: usize,
    #[serde(default = "default_max_chars")]
    pub max_chars: usize,
}

fn default_region_lines() -> usize {
    5
}

fn default_max_chars() -> usize {
    400
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: String,
    pub state: ManifestState,
    pub priority: i64,
    pub region: RegionKind,
    #[serde(default = "default_region_lines")]
    pub region_lines: usize,
    pub when: Predicate,
    #[serde(default)]
    pub flags: Vec<String>,
    #[serde(default)]
    pub capture: Option<Capture>,
}

impl Rule {
    pub fn is_blocker(&self) -> bool {
        matches!(
            self.state,
            ManifestState::BlockedPermission | ManifestState::BlockedQuestion
        )
    }

    pub fn has_flag(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema_version: i64,
    pub id: String,
    pub version: String,
    pub status_model: StatusModel,
    /// How to launch this agent. Absent only in hand-written test fixtures;
    /// every shipped manifest carries one.
    #[serde(default)]
    pub agent: Option<crate::agent::AgentDescriptor>,
    /// Sorted by descending priority at load, ties keeping file order, so the
    /// evaluator can stop at the first match instead of scoring every rule on a
    /// path that runs several times a second per session.
    #[serde(deserialize_with = "rules_by_priority")]
    pub rules: Vec<Rule>,
}

fn rules_by_priority<'de, D>(deserializer: D) -> Result<Vec<Rule>, D::Error>
where
    D: Deserializer<'de>,
{
    let mut rules = Vec::<Rule>::deserialize(deserializer)?;
    // Stable, so equal priorities keep the order the file declared them in.
    rules.sort_by_key(|rule| std::cmp::Reverse(rule.priority));
    Ok(rules)
}
