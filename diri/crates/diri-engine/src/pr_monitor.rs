//! Polls GitHub (via the `gh` CLI) for the state of every PR URL captured as
//! a session artifact: open/merged/closed, draft, review decision,
//! mergeability, CI checks, comment counts, and +/- line stats.
//!
//! Ported from `PullRequestMonitor`. Results land on
//! `SessionRecord.pullRequests`; one shared per-URL cache dedupes PRs that
//! appear in several sessions; fetches per sweep are capped so a screen full
//! of PR links can't turn one sweep into a minute of serial gh calls.
//! Silently inert when `gh` isn't installed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use diri_proto::{
    ArtifactKind, DateMillis, PrCheck, PrDiscussionItem, PullRequestStatus,
};
use serde_json::Value;

use crate::attach::AttachHub;
use crate::events::EventBus;
use crate::registry::Registry;

/// Poll cadence: PR state moves at human speed.
const SWEEP_INTERVAL: Duration = Duration::from_secs(120);
/// A cached URL is not refetched within this window.
const REFRESH_TTL: Duration = Duration::from_secs(115);
/// Review-thread resolution costs a separate GraphQL subprocess; refresh it
/// much less often than the main PR state.
const THREAD_REFRESH_TTL: Duration = Duration::from_secs(1800);
/// Network fetches per sweep.
const MAX_FETCHES_PER_SWEEP: usize = 2;
/// Recently-seen window: records viewed within this qualify for polling even
/// when no client is attached right now.
const RECENTLY_SEEN: Duration = Duration::from_secs(600);

pub fn spawn_pr_monitor(
    registry: Arc<Mutex<Registry>>,
    events: EventBus,
    attach: AttachHub,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("diri-pr-monitor".into())
        .spawn(move || {
            let Some(gh) = resolve_gh() else {
                eprintln!("dirijord-rs: pull-request monitor idle: gh not on PATH");
                return;
            };
            let mut cache: HashMap<String, PullRequestStatus> = HashMap::new();
            let mut last_attempt: HashMap<String, Instant> = HashMap::new();
            let mut last_thread_attempt: HashMap<String, Instant> = HashMap::new();
            loop {
                let waited = Instant::now();
                while waited.elapsed() < SWEEP_INTERVAL {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
                sweep(
                    &registry,
                    &events,
                    &attach,
                    &gh,
                    &mut cache,
                    &mut last_attempt,
                    &mut last_thread_attempt,
                );
            }
        })
        .expect("spawn pr monitor")
}

#[allow(clippy::too_many_arguments)]
fn sweep(
    registry: &Arc<Mutex<Registry>>,
    events: &EventBus,
    attach: &AttachHub,
    gh: &str,
    cache: &mut HashMap<String, PullRequestStatus>,
    last_attempt: &mut HashMap<String, Instant>,
    last_thread_attempt: &mut HashMap<String, Instant>,
) {
    // PR URLs worth polling: sessions currently attached, or viewed recently.
    // Merely being a live restored process is not evidence anyone is looking
    // at its PR pill.
    let records = {
        let Ok(guard) = registry.lock() else { return };
        guard.records()
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64;
    let mut wanted: Vec<(String, Vec<String>)> = Vec::new();
    for record in &records {
        let recently_seen = record
            .last_seen_at
            .as_ref()
            .is_some_and(|seen| now_ms - seen.0 < RECENTLY_SEEN.as_millis() as f64);
        if !(attach.has_sinks(&record.id.0) || recently_seen) {
            continue;
        }
        let urls: Vec<String> = record
            .artifacts
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|artifact| artifact.kind == ArtifactKind::PullRequest)
            .map(|artifact| artifact.url.clone())
            .collect();
        if !urls.is_empty() {
            wanted.push((record.id.0.clone(), urls));
        }
    }
    if wanted.is_empty() {
        return;
    }

    // Refresh stale cache entries, oldest-attempt first under the cap.
    let mut stale: Vec<String> = wanted
        .iter()
        .flat_map(|(_, urls)| urls.iter().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .filter(|url| {
            last_attempt
                .get(url)
                .is_none_or(|at| at.elapsed() >= REFRESH_TTL)
        })
        .collect();
    stale.sort_by_key(|url| last_attempt.get(url).copied());
    for url in stale.into_iter().take(MAX_FETCHES_PER_SWEEP) {
        last_attempt.insert(url.clone(), Instant::now());
        let refresh_threads = last_thread_attempt
            .get(&url)
            .is_none_or(|at| at.elapsed() >= THREAD_REFRESH_TTL);
        if refresh_threads {
            last_thread_attempt.insert(url.clone(), Instant::now());
        }
        if let Some(mut status) = fetch(&url, gh, refresh_threads) {
            if !refresh_threads
                && let Some(previous) = cache.get(&url)
            {
                status.resolved_threads = previous.resolved_threads;
                status.total_threads = previous.total_threads;
            }
            cache.insert(url, status);
        }
    }

    for (id, urls) in wanted {
        let statuses: Vec<PullRequestStatus> =
            urls.iter().filter_map(|url| cache.get(url).cloned()).collect();
        let record = {
            let Ok(mut guard) = registry.lock() else { return };
            let changed = guard.apply_pull_request_statuses(&id, statuses);
            if changed {
                let _ = guard.persist();
                guard.records().into_iter().find(|record| record.id.0 == id)
            } else {
                None
            }
        };
        if let Some(record) = record {
            events.publish_encoded(diri_proto::EventName::SESSION_UPDATED, &record, Some(&id));
        }
    }
}

fn resolve_gh() -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    path.split(':')
        .map(|dir| std::path::Path::new(dir).join("gh"))
        .find(|candidate| candidate.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

/// `gh pr view <url> --json …` plus a GraphQL round trip for review-thread
/// resolution (which `pr view` can't report). None on any failure — the last
/// cached status stays in effect.
pub fn fetch(url: &str, gh: &str, include_threads: bool) -> Option<PullRequestStatus> {
    const FIELDS: &str = "number,title,author,body,baseRefName,headRefName,state,isDraft,\
        reviewDecision,mergeable,mergeStateStatus,additions,deletions,changedFiles,\
        comments,reviews,statusCheckRollup";
    let data = run_gh(gh, &["pr", "view", url, "--json", FIELDS], Duration::from_secs(15))?;
    let mut status = parse(&data, url, now())?;

    if include_threads
        && let Some((owner, repo, number)) = pr_coordinates(url)
    {
        let query = "query=query($owner:String!,$name:String!,$number:Int!){\
            repository(owner:$owner,name:$name){pullRequest(number:$number){\
            reviewThreads(first:100){totalCount nodes{isResolved}}}}}";
        if let Some(thread_data) = run_gh(
            gh,
            &[
                "api",
                "graphql",
                "-f",
                query,
                "-f",
                &format!("owner={owner}"),
                "-f",
                &format!("name={repo}"),
                "-F",
                &format!("number={number}"),
            ],
            Duration::from_secs(15),
        ) && let Some((resolved, total)) = parse_threads(&thread_data)
        {
            status.resolved_threads = Some(resolved);
            status.total_threads = Some(total);
        }
    }
    Some(status)
}

/// Runs gh with a watchdog so a hung network call can't wedge the sweep.
fn run_gh(gh: &str, args: &[&str], timeout: Duration) -> Option<Vec<u8>> {
    let mut child = std::process::Command::new(gh)
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(exit)) if exit.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return None,
        }
    }
    let mut output = Vec::new();
    use std::io::Read;
    child.stdout.take()?.read_to_end(&mut output).ok()?;
    Some(output)
}

/// `github.com/owner/repo/pull/123` → (owner, repo, 123).
pub fn pr_coordinates(url: &str) -> Option<(String, String, i64)> {
    let parts: Vec<&str> = url.split('/').collect();
    let pull = parts.iter().position(|part| *part == "pull")?;
    if pull < 2 || pull + 1 >= parts.len() {
        return None;
    }
    let number: i64 = parts[pull + 1]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    Some((parts[pull - 2].to_string(), parts[pull - 1].to_string(), number))
}

/// Decodes the reviewThreads GraphQL response into (resolved, total).
pub fn parse_threads(data: &[u8]) -> Option<(i64, i64)> {
    let value: Value = serde_json::from_slice(data).ok()?;
    let threads = &value["data"]["repository"]["pullRequest"]["reviewThreads"];
    let total = threads["totalCount"].as_i64()?;
    let resolved = threads["nodes"]
        .as_array()?
        .iter()
        .filter(|node| node["isResolved"].as_bool() == Some(true))
        .count() as i64;
    Some((resolved, total))
}

/// Decodes one `gh pr view --json` payload. Input-driven so tests feed
/// canned JSON without a subprocess.
pub fn parse(data: &[u8], url: &str, fetched_at: DateMillis) -> Option<PullRequestStatus> {
    let view: Value = serde_json::from_slice(data).ok()?;
    let number = view["number"].as_i64()?;
    let string = |value: &Value| value.as_str().map(str::to_string);
    let nonempty = |value: &Value| value.as_str().filter(|s| !s.is_empty()).map(str::to_string);

    let checks: Vec<PrCheck> = view["statusCheckRollup"]
        .as_array()
        .map(|rollup| {
            rollup
                .iter()
                .map(|check| {
                    // CheckRun reports conclusion once COMPLETED; StatusContext
                    // only has state. One word decides the bucket; an empty
                    // conclusion means still running.
                    let verdict = ["conclusion", "state", "status"]
                        .iter()
                        .filter_map(|key| check[*key].as_str())
                        .find(|value| !value.is_empty())
                        .unwrap_or("");
                    let result = match verdict {
                        "SUCCESS" | "NEUTRAL" | "SKIPPED" => "pass",
                        "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED"
                        | "STARTUP_FAILURE" => "fail",
                        _ => "pending",
                    };
                    let base = check["name"]
                        .as_str()
                        .or_else(|| check["context"].as_str())
                        .unwrap_or("check");
                    let name = match check["workflowName"].as_str() {
                        Some(workflow) => format!("{workflow} / {base}"),
                        None => base.to_string(),
                    };
                    PrCheck {
                        name,
                        result: result.to_string(),
                        detail: (!verdict.is_empty()).then(|| verdict.to_string()),
                        url: string(&check["detailsUrl"]).or_else(|| string(&check["targetUrl"])),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let count = |result: &str| checks.iter().filter(|check| check.result == result).count() as i64;

    let discussion_item = |kind: &str, entry: &Value, created_key: &str| PrDiscussionItem {
        kind: kind.to_string(),
        author: entry["author"]["login"]
            .as_str()
            .unwrap_or("ghost")
            .to_string(),
        body: entry["body"].as_str().unwrap_or("").to_string(),
        state: if kind == "review" {
            string(&entry["state"])
        } else {
            None
        },
        created_at: entry[created_key]
            .as_str()
            .and_then(parse_github_date),
        url: string(&entry["url"]),
    };
    let comments: Vec<PrDiscussionItem> = view["comments"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|comment| discussion_item("comment", comment, "createdAt"))
                .collect()
        })
        .unwrap_or_default();
    let reviews: Vec<PrDiscussionItem> = view["reviews"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|review| discussion_item("review", review, "submittedAt"))
                .collect()
        })
        .unwrap_or_default();
    let comment_count = comments.len() as i64;
    let review_count = reviews.len() as i64;
    let mut discussion: Vec<PrDiscussionItem> = comments.into_iter().chain(reviews).collect();
    discussion.sort_by(|a, b| {
        let time = |item: &PrDiscussionItem| item.created_at.as_ref().map_or(f64::MIN, |at| at.0);
        time(a)
            .partial_cmp(&time(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Some(PullRequestStatus {
        url: url.to_string(),
        number,
        title: string(&view["title"]),
        author: string(&view["author"]["login"]),
        body: string(&view["body"]),
        base_ref_name: string(&view["baseRefName"]),
        head_ref_name: string(&view["headRefName"]),
        state: view["state"].as_str().unwrap_or("OPEN").to_string(),
        is_draft: view["isDraft"].as_bool().unwrap_or(false),
        review_decision: nonempty(&view["reviewDecision"]),
        mergeable: string(&view["mergeable"]),
        merge_state_status: string(&view["mergeStateStatus"]),
        additions: view["additions"].as_i64().unwrap_or(0),
        deletions: view["deletions"].as_i64().unwrap_or(0),
        changed_files: view["changedFiles"].as_i64().unwrap_or(0),
        comment_count,
        review_count,
        resolved_threads: None,
        total_threads: None,
        checks_passed: count("pass"),
        checks_failed: count("fail"),
        checks_pending: count("pending"),
        checks: (!checks.is_empty()).then_some(checks),
        discussion: (!discussion.is_empty()).then_some(discussion),
        fetched_at,
    })
}

fn parse_github_date(value: &str) -> Option<DateMillis> {
    // ISO 8601 `2026-08-07T12:34:56Z`; a hand parser avoids a chrono
    // dependency for one field the client only sorts and displays by.
    let bytes = value.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        value.get(range)?.parse().ok()
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    // Days since epoch via the civil-days algorithm.
    let years = if month <= 2 { year - 1 } else { year };
    let era = years.div_euclid(400);
    let year_of_era = years - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(DateMillis(
        ((days * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000) as f64,
    ))
}

fn now() -> DateMillis {
    DateMillis::from(std::time::SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_come_out_of_a_pr_url() {
        assert_eq!(
            pr_coordinates("https://github.com/cristicretu/diri/pull/7"),
            Some(("cristicretu".into(), "diri".into(), 7))
        );
        assert_eq!(pr_coordinates("https://github.com/x/pull"), None);
    }

    #[test]
    fn a_gh_view_payload_parses_into_the_wire_status() {
        let payload = serde_json::json!({
            "number": 12,
            "title": "Add the thing",
            "author": {"login": "shawn"},
            "state": "OPEN",
            "isDraft": false,
            "reviewDecision": "",
            "additions": 10, "deletions": 2, "changedFiles": 3,
            "comments": [{"author": {"login": "giga"}, "body": "nice", "createdAt": "2026-08-07T10:00:00Z"}],
            "reviews": [{"author": {"login": "bot"}, "body": "lgtm", "state": "APPROVED", "submittedAt": "2026-08-07T11:00:00Z"}],
            "statusCheckRollup": [
                {"name": "test", "workflowName": "CI", "status": "COMPLETED", "conclusion": "SUCCESS", "detailsUrl": "https://x"},
                {"context": "lint", "state": "FAILURE"},
                {"name": "build", "status": "IN_PROGRESS", "conclusion": ""}
            ],
        });
        let status = parse(
            payload.to_string().as_bytes(),
            "https://github.com/o/r/pull/12",
            DateMillis(0.0),
        )
        .expect("parse");
        assert_eq!(status.number, 12);
        assert_eq!(status.author.as_deref(), Some("shawn"));
        assert_eq!(status.review_decision, None, "empty string means none");
        assert_eq!(
            (status.checks_passed, status.checks_failed, status.checks_pending),
            (1, 1, 1)
        );
        let checks = status.checks.expect("checks");
        assert_eq!(checks[0].name, "CI / test");
        assert_eq!(checks[1].name, "lint");
        let discussion = status.discussion.expect("discussion");
        assert_eq!(discussion.len(), 2);
        assert_eq!(discussion[0].kind, "comment", "sorted by time");
        assert_eq!(discussion[1].state.as_deref(), Some("APPROVED"));
        assert!(
            discussion[0].created_at.expect("date").0 > 1.7e12,
            "the date parser lands in the right epoch decade"
        );
    }

    #[test]
    fn thread_counts_decode_from_graphql() {
        let payload = serde_json::json!({
            "data": {"repository": {"pullRequest": {"reviewThreads": {
                "totalCount": 5,
                "nodes": [{"isResolved": true}, {"isResolved": false}, {"isResolved": true}]
            }}}}
        });
        assert_eq!(
            parse_threads(payload.to_string().as_bytes()),
            Some((2, 5))
        );
    }
}
