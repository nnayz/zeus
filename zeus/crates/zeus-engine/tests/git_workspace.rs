//! Git workspace service: real repositories, fake git/gh executables, and the
//! remote argv script. These tests do not need a developer's SSH host.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use zeus_engine::git_workspace::{
    GitExecutor, GitTools, LocalGit, SessionGit, encode_remote_git_input, owner_for_path,
    parse_pr_input, remote_git_run_script,
};
use zeus_proto::{
    DateMillis, GitCheckoutDisposition, GitCheckoutMode, GitPatchMutation, GitReviewTarget,
    ProjectId, Resumability, SessionId, SessionRecord, SessionStatus, TitleSource,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct TestRepo {
    path: PathBuf,
}

impl TestRepo {
    fn new() -> Option<Self> {
        if !Command::new("git")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
        {
            eprintln!("skipping Git workspace test: git is unavailable");
            return None;
        }
        let ordinal = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "zeus-git-workspace-{}-{ordinal}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create test repository directory");
        let repo = Self { path };
        repo.git(["init", "--quiet"]);
        repo.git(["symbolic-ref", "HEAD", "refs/heads/main"]);
        repo.git(["config", "user.name", "Zeus Test"]);
        repo.git(["config", "user.email", "zeus@example.invalid"]);
        Some(repo)
    }

    fn git<const N: usize>(&self, args: [&str; N]) -> Vec<u8> {
        self.git_args(&args)
    }

    fn git_args(&self, args: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .current_dir(&self.path)
            .args(args)
            .stdin(Stdio::null())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run test git");
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn git_expect_failure<const N: usize>(&self, args: [&str; N]) {
        let output = Command::new("git")
            .current_dir(&self.path)
            .args(args)
            .stdin(Stdio::null())
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("run test git");
        assert!(!output.status.success());
    }

    fn write(&self, path: &str, contents: &str) {
        let path = self.path.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn commit_all(&self, message: &str) {
        self.git(["add", "--all"]);
        self.git(["commit", "--quiet", "-m", message]);
    }

    fn session(&self, id: &str, status: SessionStatus) -> SessionRecord {
        test_record(id, &self.path.to_string_lossy(), status, "Codex")
    }

    fn git_session<'a>(
        &'a self,
        session: &'a SessionRecord,
        records: &'a [SessionRecord],
        tools: &'a GitTools,
        local: &'a LocalGit,
    ) -> SessionGit<'a> {
        SessionGit {
            session,
            records,
            executor: local,
            gh: tools.gh(),
            locks: &tools.locks,
        }
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        if let Ok(output) = Command::new("git")
            .current_dir(&self.path)
            .args(["worktree", "list", "--porcelain"])
            .output()
        {
            let root = self.path.to_string_lossy();
            let prefix = format!("{}-", root);
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let Some(path) = line.strip_prefix("worktree ") else {
                    continue;
                };
                if path.starts_with(&prefix) {
                    let _ = fs::remove_dir_all(path);
                }
            }
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn initialize_bare_remote(repo: &TestRepo, directory: &Path) {
    let status = Command::new("git")
        .args(["init", "--bare", "--quiet"])
        .current_dir(directory)
        .status()
        .expect("initialize bare remote");
    assert!(status.success());
    let url = format!("file://{}", directory.display());
    let rewrite = format!("url.{url}.insteadOf");
    repo.git_args(&["config", &rewrite, "https://github.com/nnayz/zeus.git"]);
    repo.git([
        "remote",
        "add",
        "origin",
        "https://github.com/nnayz/zeus.git",
    ]);
    repo.git(["push", "--quiet", "-u", "origin", "main"]);
}

fn test_record(id: &str, cwd: &str, status: SessionStatus, title: &str) -> SessionRecord {
    SessionRecord {
        id: SessionId(id.into()),
        kind: zeus_proto::AgentKind::SHELL,
        cwd: cwd.into(),
        project_id: ProjectId("p".into()),
        worktree_path: None,
        git_branch: None,
        title: title.into(),
        title_source: TitleSource::Placeholder,
        agent_session_id: None,
        transcript_path: None,
        status,
        needs_input: None,
        resumability: Resumability::NotResumable,
        parent: None,
        created_at: DateMillis(0.0),
        updated_at: DateMillis(0.0),
        last_turn_completed_at: None,
        last_seen_at: None,
        pinned: false,
        archived_at: None,
        host: None,
        remote_persistence: None,
        hibernation: None,
        memory_bytes: None,
        artifacts: None,
        pull_requests: None,
        listening_ports: None,
        foreground_agent: None,
        workbench: None,
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write fake executable");
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).unwrap();
}

fn write_fake_gh(path: &Path, json: &str) {
    write_executable(
        path,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = auth ]; then exit 0; fi\nif [ \"$1\" = pr ] && [ \"$2\" = view ]; then\ncat <<'ZEUS_JSON'\n{json}\nZEUS_JSON\nexit 0\nfi\nexit 1\n"
        ),
    );
}

#[test]
fn discovers_status_and_preserves_existing_mutations() {
    let Some(repo) = TestRepo::new() else { return };
    repo.write("tracked.txt", "base\n");
    repo.commit_all("base");
    repo.write("tracked.txt", "worktree\n");
    repo.write("both.txt", "staged\n");
    repo.git(["add", "both.txt"]);
    repo.write("both.txt", "staged and worktree\n");
    repo.write(":(glob) literal name.txt", "untracked\n");

    let tools = GitTools::new();
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [session.clone()];
    let git = repo.git_session(&session, &records, &tools, &local);
    let workspace = git.workspace(GitReviewTarget::WorkingTree, None).unwrap();
    assert_eq!(workspace.branch.name.as_deref(), Some("main"));
    assert_eq!(workspace.status.staged.len(), 1);
    assert_eq!(
        workspace.status.untracked[0].path,
        ":(glob) literal name.txt"
    );
    assert!(workspace.dirty);
    assert!(!workspace.conflicted);

    git.stage(&[":(glob) literal name.txt".into()]).unwrap();
    git.unstage(&[":(glob) literal name.txt".into()]).unwrap();
    git.discard(&["tracked.txt".into()]).unwrap();
    assert_eq!(
        fs::read_to_string(repo.path.join("tracked.txt")).unwrap(),
        "base\n"
    );
}

#[test]
fn unborn_unstage_and_conflict_grouping() {
    let Some(repo) = TestRepo::new() else { return };
    let tools = GitTools::new();
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [session.clone()];
    let git = repo.git_session(&session, &records, &tools, &local);
    repo.write("first.txt", "first\n");
    git.stage(&["first.txt".into()]).unwrap();
    git.unstage(&["first.txt".into()]).unwrap();
    assert_eq!(
        fs::read_to_string(repo.path.join("first.txt")).unwrap(),
        "first\n"
    );

    repo.write("conflict.txt", "base\n");
    repo.commit_all("base");
    repo.git(["checkout", "--quiet", "-b", "side"]);
    repo.write("conflict.txt", "side\n");
    repo.commit_all("side");
    repo.git(["checkout", "--quiet", "main"]);
    repo.write("conflict.txt", "main\n");
    repo.commit_all("main");
    repo.git_expect_failure(["merge", "--no-edit", "side"]);
    let workspace = git.workspace(GitReviewTarget::WorkingTree, None).unwrap();
    assert!(workspace.conflicted);
    assert_eq!(workspace.status.conflicted[0].path, "conflict.txt");
}

#[test]
fn dirty_or_live_checkout_is_not_switched_in_place() {
    let Some(repo) = TestRepo::new() else { return };
    repo.write("file.txt", "base\n");
    repo.commit_all("base");
    repo.git(["checkout", "--quiet", "-b", "feature"]);
    repo.git(["checkout", "--quiet", "main"]);
    repo.write("file.txt", "dirty\n");

    let tools = GitTools::new();
    let local = tools.local();
    let idle = repo.session("s_idle", SessionStatus::Idle);
    let records = [];
    let git = repo.git_session(&idle, &records, &tools, &local);
    let plan = git.checkout_plan("feature").unwrap();
    assert!(matches!(plan.disposition, GitCheckoutDisposition::Blocked));
    assert!(plan.reasons.iter().any(|reason| reason.code == "dirty"));
    assert_eq!(
        git.workspace(GitReviewTarget::WorkingTree, None)
            .unwrap()
            .branch
            .name
            .as_deref(),
        Some("main")
    );

    repo.write("file.txt", "base\n");
    repo.git(["checkout", "--quiet", "file.txt"]);
    let live = repo.session("s_live", SessionStatus::Working);
    let records = [live.clone()];
    let git = repo.git_session(&live, &records, &tools, &local);
    let plan = git.checkout_plan("feature").unwrap();
    assert!(matches!(
        plan.disposition,
        GitCheckoutDisposition::OpenNewWorktree { .. }
    ));
    assert!(plan.reasons.iter().any(|reason| reason.code == "liveOwner"));
}

#[test]
fn clean_unowned_checkout_switches_and_existing_worktree_is_focused() {
    let Some(repo) = TestRepo::new() else { return };
    repo.write("file.txt", "base\n");
    repo.commit_all("base");
    repo.git(["checkout", "--quiet", "-b", "feature"]);
    repo.write("file.txt", "feature\n");
    repo.commit_all("feature");
    repo.git(["checkout", "--quiet", "main"]);

    let tools = GitTools::new();
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [];
    let git = repo.git_session(&session, &records, &tools, &local);
    let result = git.checkout("feature", GitCheckoutMode::Switch).unwrap();
    assert!(matches!(
        result.plan.disposition,
        GitCheckoutDisposition::SwitchInPlace
    ));
    assert_eq!(result.workspace.branch.name.as_deref(), Some("feature"));

    let opened = git.checkout("main", GitCheckoutMode::Worktree).unwrap();
    let GitCheckoutDisposition::OpenNewWorktree { path } = opened.plan.disposition else {
        panic!("expected a new worktree, got {:?}", opened.plan.disposition);
    };
    let session = repo.session("s1", SessionStatus::Idle);
    let other = test_record("s2", &path, SessionStatus::Working, "other");
    let records = [session.clone(), other];
    let git = repo.git_session(&session, &records, &tools, &local);
    let plan = git.checkout_plan("main").unwrap();
    assert!(matches!(
        plan.disposition,
        GitCheckoutDisposition::FocusExisting { .. }
    ));
}

#[test]
fn compare_does_not_change_the_checkout() {
    let Some(repo) = TestRepo::new() else { return };
    repo.write("file.txt", "main\n");
    repo.commit_all("main");
    repo.git(["checkout", "--quiet", "-b", "feature"]);
    repo.write("file.txt", "feature\n");
    repo.commit_all("feature");
    let tools = GitTools::new();
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [session.clone()];
    let git = repo.git_session(&session, &records, &tools, &local);
    let compare = git
        .compare(GitReviewTarget::Branch {
            ref_name: "main".into(),
        })
        .unwrap();
    assert!(String::from_utf8_lossy(&compare.patch).contains("feature"));
    assert_eq!(
        git.workspace(GitReviewTarget::WorkingTree, None)
            .unwrap()
            .branch
            .name
            .as_deref(),
        Some("feature")
    );
}

#[test]
fn detached_and_unborn_states_are_not_invented_into_branches() {
    let Some(repo) = TestRepo::new() else { return };
    let tools = GitTools::new();
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [session.clone()];
    let git = repo.git_session(&session, &records, &tools, &local);
    let unborn = git.workspace(GitReviewTarget::WorkingTree, None).unwrap();
    assert_eq!(unborn.branch.name.as_deref(), Some("main"));
    assert!(unborn.unborn);

    repo.write("file.txt", "base\n");
    repo.commit_all("base");
    let oid = String::from_utf8_lossy(&repo.git(["rev-parse", "HEAD"]))
        .trim()
        .to_owned();
    repo.git(["checkout", "--quiet", "--detach", "HEAD"]);
    let detached = git.workspace(GitReviewTarget::WorkingTree, None).unwrap();
    assert!(detached.detached);
    assert!(detached.branch.name.is_none());
    assert!(oid.starts_with(detached.branch.oid.as_deref().unwrap_or("")));
}

#[test]
fn list_refs_puts_local_current_first_and_search_is_in_process() {
    let Some(repo) = TestRepo::new() else { return };
    repo.write("file.txt", "base\n");
    repo.commit_all("base");
    repo.git(["checkout", "--quiet", "-b", "feature/search"]);
    let tools = GitTools::new();
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [session.clone()];
    let git = repo.git_session(&session, &records, &tools, &local);
    let listed = git.list_refs(Some("search")).unwrap();
    assert!(
        listed
            .refs
            .iter()
            .any(|entry| entry.short_name == "feature/search" && entry.current)
    );
    assert!(git.list_refs(Some("nope")).unwrap().refs.is_empty());
}

#[test]
fn remote_tracking_branch_switch_creates_a_local_tracking_branch() {
    let Some(repo) = TestRepo::new() else { return };
    repo.write("file.txt", "main\n");
    repo.commit_all("main");
    let temp = tempfile::tempdir().unwrap();
    let remote = temp.path().join("remote.git");
    fs::create_dir(&remote).unwrap();
    initialize_bare_remote(&repo, &remote);
    repo.git(["checkout", "--quiet", "-b", "remote-only"]);
    repo.write("file.txt", "remote branch\n");
    repo.commit_all("remote branch");
    repo.git(["push", "--quiet", "origin", "remote-only"]);
    repo.git(["checkout", "--quiet", "main"]);
    repo.git(["branch", "-D", "remote-only"]);
    repo.git(["fetch", "--quiet", "origin"]);

    let tools = GitTools::new();
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [];
    let git = repo.git_session(&session, &records, &tools, &local);
    let listed = git.list_refs(None).unwrap();
    assert_eq!(listed.refs[0].short_name, "main");
    assert!(listed.refs[0].current);
    assert!(listed.refs.iter().any(|entry| {
        entry.short_name == "origin/remote-only" && entry.kind == zeus_proto::GitRefKind::Remote
    }));

    let switched = git
        .checkout("origin/remote-only", GitCheckoutMode::Switch)
        .unwrap();
    assert_eq!(
        switched.workspace.branch.name.as_deref(),
        Some("remote-only")
    );
    assert_eq!(
        switched.workspace.branch.upstream.as_deref(),
        Some("origin/remote-only")
    );
}

#[test]
fn unstaging_a_rename_restores_both_index_paths() {
    let Some(repo) = TestRepo::new() else { return };
    repo.write("old.txt", "content worth keeping\n");
    repo.commit_all("base");
    repo.git(["mv", "old.txt", "new.txt"]);
    let tools = GitTools::new();
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [session.clone()];
    let git = repo.git_session(&session, &records, &tools, &local);
    let status = git.workspace(GitReviewTarget::WorkingTree, None).unwrap();
    assert_eq!(status.status.staged[0].path, "new.txt");
    assert_eq!(
        status.status.staged[0].original_path.as_deref(),
        Some("old.txt")
    );
    git.unstage(&["new.txt".into()]).unwrap();
    assert!(
        git.workspace(GitReviewTarget::WorkingTree, None)
            .unwrap()
            .status
            .staged
            .is_empty()
    );
    assert!(repo.path.join("new.txt").exists());
    assert_eq!(
        String::from_utf8_lossy(&repo.git(["show", "HEAD:old.txt"])),
        "content worth keeping\n"
    );
}

#[test]
fn hunk_mutations_preserve_unselected_work() {
    let Some(repo) = TestRepo::new() else { return };
    let base = (1..=30)
        .map(|line| format!("line {line:02}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    repo.write("review.txt", &base);
    repo.commit_all("base");
    let mut changed = base.lines().map(str::to_owned).collect::<Vec<_>>();
    changed[1] = "changed early".into();
    changed[20] = "changed late".into();
    repo.write("review.txt", &(changed.join("\n") + "\n"));

    let tools = GitTools::new();
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [session.clone()];
    let git = repo.git_session(&session, &records, &tools, &local);
    let patch = first_hunk_patch(&repo.git(["diff", "--no-ext-diff", "--", "review.txt"]));
    git.apply_patch(&patch, GitPatchMutation::Stage).unwrap();
    let cached = String::from_utf8(repo.git(["diff", "--cached", "--", "review.txt"])).unwrap();
    let working = String::from_utf8(repo.git(["diff", "--", "review.txt"])).unwrap();
    assert!(cached.contains("changed early"));
    assert!(!cached.contains("changed late"));
    assert!(!working.contains("changed early"));
    assert!(working.contains("changed late"));

    let staged_patch = repo.git(["diff", "--cached", "--", "review.txt"]);
    git.apply_patch(&staged_patch, GitPatchMutation::Unstage)
        .unwrap();
    assert!(repo.git(["diff", "--cached"]).is_empty());

    let patch = first_hunk_patch(&repo.git(["diff", "--no-ext-diff", "--", "review.txt"]));
    git.apply_patch(&patch, GitPatchMutation::Discard).unwrap();
    let contents = fs::read_to_string(repo.path.join("review.txt")).unwrap();
    assert!(contents.contains("line 02"));
    assert!(!contents.contains("changed early"));
    assert!(contents.contains("changed late"));
}

#[test]
fn commit_validation_and_identity_match_existing_review_behavior() {
    let Some(repo) = TestRepo::new() else { return };
    repo.write("first.txt", "hello\n");
    let tools = GitTools::new();
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [session.clone()];
    let git = repo.git_session(&session, &records, &tools, &local);
    git.stage(&["first.txt".into()]).unwrap();
    assert_eq!(git.commit(" \n\t").unwrap_err().code(), "empty_commit");
    let commit = git.commit("Review cockpit foundation\n").unwrap();
    assert_eq!(commit.oid.len(), 40);
    assert_eq!(commit.summary, "Review cockpit foundation");
    assert!(commit.workspace.status.staged.is_empty());
}

#[test]
fn untracked_patch_can_stage_but_never_discard_the_file() {
    let Some(repo) = TestRepo::new() else { return };
    repo.write("base.txt", "base\n");
    repo.commit_all("base");
    repo.write("new.txt", "first\nsecond\n");
    let patch = Command::new("git")
        .args(["diff", "--no-index", "--", "/dev/null", "new.txt"])
        .current_dir(&repo.path)
        .output()
        .unwrap()
        .stdout;
    let tools = GitTools::new();
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [session.clone()];
    let git = repo.git_session(&session, &records, &tools, &local);
    assert_eq!(
        git.apply_patch(&patch, GitPatchMutation::Discard)
            .unwrap_err()
            .code(),
        "invalid_patch"
    );
    assert_eq!(
        fs::read_to_string(repo.path.join("new.txt")).unwrap(),
        "first\nsecond\n"
    );
    git.apply_patch(&patch, GitPatchMutation::Stage).unwrap();
    assert_eq!(
        String::from_utf8(repo.git(["show", ":new.txt"])).unwrap(),
        "first\nsecond\n"
    );
}

#[test]
fn ownership_is_scoped_to_the_execution_host() {
    let Some(repo) = TestRepo::new() else { return };
    repo.write("file.txt", "base\n");
    repo.commit_all("base");
    fs::create_dir(repo.path.join("nested")).unwrap();
    let tools = GitTools::new();
    let local = tools.local();
    let mut local_record = repo.session("s_local", SessionStatus::Working);
    local_record.title = "local".into();
    let mut remote_record = repo.session("s_remote", SessionStatus::Idle);
    remote_record.cwd = repo
        .path
        .canonicalize()
        .unwrap()
        .join("nested")
        .to_string_lossy()
        .into_owned();
    remote_record.host = Some("forge".into());
    remote_record.title = "remote".into();
    let records = [local_record, remote_record.clone()];
    let git = repo.git_session(&remote_record, &records, &tools, &local);
    let owner = git
        .workspace(GitReviewTarget::WorkingTree, None)
        .unwrap()
        .owner
        .unwrap();
    assert_eq!(owner.session_id.0, "s_remote");
    assert_eq!(owner.title, "remote");
}

fn first_hunk_patch(patch: &[u8]) -> Vec<u8> {
    let patch = String::from_utf8_lossy(patch);
    let mut result = String::new();
    let mut saw_hunk = false;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("@@") {
            if saw_hunk {
                break;
            }
            saw_hunk = true;
        }
        result.push_str(line);
    }
    assert!(saw_hunk, "test patch did not contain a hunk");
    result.into_bytes()
}

#[test]
fn fake_git_and_remote_script_use_structured_argv() {
    let temp = tempfile::tempdir().unwrap();
    let fake_git = temp.path().join("git");
    write_executable(
        &fake_git,
        r#"#!/bin/sh
while [ "$1" = "--no-pager" ] || [ "$1" = "-c" ]; do
  if [ "$1" = "-c" ]; then shift; fi
  shift
done
here=$(cd "$(dirname "$0")" && pwd)
log=$(cat "$here/logpath" 2>/dev/null || printf '%s' /tmp/fake-git.log)
root=$(cat "$here/root" 2>/dev/null || pwd)
printf '%s\n' "$*" >> "$log"
case "$1" in
  rev-parse)
    if [ "$2" = "--show-toplevel" ]; then printf '%s\n' "$root"; exit 0; fi
    if [ "$2" = "--is-bare-repository" ]; then printf '%s\n' false; exit 0; fi
    if [ "$2" = "--git-dir" ]; then printf '%s\n' .git; exit 0; fi
    if [ "$2" = "--git-common-dir" ]; then printf '%s\n' .git; exit 0; fi
    exit 1
    ;;
  status)
    printf '# branch.oid abc\0# branch.head fake\0'
    exit 0
    ;;
  remote)
    if [ "$2" = "get-url" ]; then printf '%s\n' 'https://github.com/nnayz/zeus.git'; exit 0; fi
    printf '%s\n' origin
    exit 0
    ;;
  *)
    exit 0
    ;;
esac
"#,
    );
    let log = temp.path().join("git.log");
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    fs::write(temp.path().join("root"), root.display().to_string()).unwrap();
    fs::write(temp.path().join("logpath"), log.display().to_string()).unwrap();
    let local = LocalGit {
        program: fake_git.clone(),
    };
    let output = local
        .run(
            &root.to_string_lossy(),
            &["rev-parse", "--show-toplevel"],
            None,
            "finding repository",
            Duration::from_secs(5),
        )
        .unwrap();
    assert!(output.status.success());

    let payload =
        encode_remote_git_input(&root.to_string_lossy(), &["status", "--porcelain=v2"], None);
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(remote_git_run_script())
        .env("PATH", format!("{}:/usr/bin:/bin", temp.path().display()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child.stdin.as_mut().unwrap().write_all(&payload).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log_text = fs::read_to_string(&log).unwrap();
    assert!(log_text.contains("status --porcelain=v2"));
}

#[test]
fn fake_git_timeout_and_output_are_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let sleeping = temp.path().join("sleeping-git");
    write_executable(&sleeping, "#!/bin/sh\nexec /bin/sleep 2\n");
    let local = LocalGit { program: sleeping };
    let started = std::time::Instant::now();
    let error = local
        .run(
            &temp.path().to_string_lossy(),
            &["status"],
            None,
            "testing timeout",
            Duration::from_millis(50),
        )
        .unwrap_err();
    assert_eq!(error.code(), "timeout");
    assert!(started.elapsed() < Duration::from_secs(1));

    let noisy = temp.path().join("noisy-git");
    write_executable(
        &noisy,
        "#!/bin/sh\nexec /usr/bin/head -c 9000000 /dev/zero\n",
    );
    let local = LocalGit { program: noisy };
    let error = local
        .run(
            &temp.path().to_string_lossy(),
            &["status"],
            None,
            "testing output bound",
            Duration::from_secs(5),
        )
        .unwrap_err();
    assert_eq!(error.code(), "output_too_large");
}

#[test]
fn fake_gh_resolves_hash_and_url_without_network() {
    let temp = tempfile::tempdir().unwrap();
    let fake_gh = temp.path().join("gh");
    write_executable(
        &fake_gh,
        r#"#!/bin/sh
if [ "$1" = "auth" ]; then exit 0; fi
if [ "$1" = "pr" ] && [ "$2" = "view" ]; then
  cat <<'JSON'
{"number":12,"title":"Add the thing","author":{"login":"n"},"body":"n","baseRefName":"main","headRefName":"feature","state":"OPEN","isDraft":false,"reviewDecision":"","mergeable":"MERGEABLE","mergeStateStatus":"CLEAN","additions":1,"deletions":0,"changedFiles":1,"comments":[],"reviews":[],"statusCheckRollup":[],"headRefOid":"aaa","baseRefOid":"bbb","isCrossRepository":false,"headRepository":{"name":"zeus"},"headRepositoryOwner":{"login":"nnayz"},"url":"https://github.com/nnayz/zeus/pull/12"}
JSON
  exit 0
fi
exit 1
"#,
    );
    let Some(repo) = TestRepo::new() else { return };
    repo.write("file.txt", "base\n");
    repo.commit_all("base");
    let remote = temp.path().join("remote.git");
    fs::create_dir(&remote).unwrap();
    initialize_bare_remote(&repo, &remote);
    repo.git(["checkout", "--quiet", "-b", "feature"]);
    repo.write("file.txt", "pull request\n");
    repo.commit_all("pull request");
    repo.git(["push", "--quiet", "origin", "HEAD:refs/pull/12/head"]);
    repo.git(["checkout", "--quiet", "main"]);
    let tools = GitTools {
        git: PathBuf::from("git"),
        gh: fake_gh,
        locks: Default::default(),
    };
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [session.clone()];
    let git = repo.git_session(&session, &records, &tools, &local);
    let resolved = git.pr_resolve("#12").unwrap();
    assert_eq!(resolved.status.number, 12);
    assert!(resolved.same_repository);
    assert!(!resolved.is_cross_repository);
    let compared = git
        .compare(GitReviewTarget::PullRequest {
            url: resolved.status.url.clone(),
            number: resolved.status.number,
        })
        .unwrap();
    assert!(String::from_utf8_lossy(&compared.patch).contains("pull request"));
    assert_eq!(compared.base_ref.as_deref(), Some("refs/zeus/pr/12/base"));
    assert_eq!(compared.head_ref.as_deref(), Some("refs/zeus/pr/12/head"));
    let url = parse_pr_input(
        "https://github.com/nnayz/zeus/pull/12/files",
        Some("nnayz/zeus"),
    )
    .unwrap();
    assert_eq!(url, "https://github.com/nnayz/zeus/pull/12");
}

#[test]
fn fake_gh_unauthenticated_is_actionable() {
    let temp = tempfile::tempdir().unwrap();
    let fake_gh = temp.path().join("gh");
    write_executable(&fake_gh, "#!/bin/sh\nexit 1\n");
    let Some(repo) = TestRepo::new() else { return };
    repo.write("file.txt", "x\n");
    repo.commit_all("base");
    repo.git([
        "remote",
        "add",
        "origin",
        "https://github.com/nnayz/zeus.git",
    ]);
    let tools = GitTools {
        git: PathBuf::from("git"),
        gh: fake_gh,
        locks: Default::default(),
    };
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [session.clone()];
    let git = repo.git_session(&session, &records, &tools, &local);
    let error = git.pr_resolve("12").unwrap_err();
    assert_eq!(error.code(), "unauthenticated");
}

#[test]
fn fork_pr_head_opens_from_the_base_repository_without_cloning() {
    let temp = tempfile::tempdir().unwrap();
    let fake_gh = temp.path().join("gh");
    write_fake_gh(
        &fake_gh,
        r#"{"number":12,"title":"Fork change","author":{"login":"forker"},"body":"body","baseRefName":"main","headRefName":"feature","state":"OPEN","isDraft":false,"reviewDecision":"","mergeable":"MERGEABLE","mergeStateStatus":"CLEAN","additions":1,"deletions":0,"changedFiles":1,"comments":[],"reviews":[],"statusCheckRollup":[],"headRefOid":"aaa","baseRefOid":"bbb","isCrossRepository":true,"headRepository":{"name":"zeus"},"headRepositoryOwner":{"login":"forker"},"url":"https://github.com/nnayz/zeus/pull/12"}"#,
    );
    let Some(repo) = TestRepo::new() else { return };
    repo.write("file.txt", "base\n");
    repo.commit_all("base");
    let remote = temp.path().join("remote.git");
    fs::create_dir(&remote).unwrap();
    initialize_bare_remote(&repo, &remote);
    repo.git(["checkout", "--quiet", "-b", "fork-head"]);
    repo.write("file.txt", "fork\n");
    repo.commit_all("fork");
    repo.git(["push", "--quiet", "origin", "HEAD:refs/pull/12/head"]);
    repo.git(["checkout", "--quiet", "main"]);

    let tools = GitTools {
        git: PathBuf::from("git"),
        gh: fake_gh,
        locks: Default::default(),
    };
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [];
    let git = repo.git_session(&session, &records, &tools, &local);
    let resolved = git.pr_resolve("#12").unwrap();
    assert!(resolved.same_repository);
    assert!(resolved.is_cross_repository);
    assert_eq!(resolved.head_repository.as_deref(), Some("forker/zeus"));
    let opened = git.pr_open("#12").unwrap();
    assert_eq!(opened.plan.ref_name, "pr/12");
    let GitCheckoutDisposition::OpenNewWorktree { path } = opened.plan.disposition else {
        panic!("fork PR did not open an isolated worktree");
    };
    assert!(Path::new(&path).is_dir());
}

#[test]
fn cross_repository_pr_can_be_viewed_but_not_opened_from_this_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let fake_gh = temp.path().join("gh");
    write_fake_gh(
        &fake_gh,
        r#"{"number":7,"title":"Other repo","author":{"login":"n"},"body":"body","baseRefName":"main","headRefName":"feature","state":"OPEN","isDraft":false,"reviewDecision":"","mergeable":"MERGEABLE","mergeStateStatus":"CLEAN","additions":1,"deletions":0,"changedFiles":1,"comments":[],"reviews":[],"statusCheckRollup":[],"headRefOid":"aaa","baseRefOid":"bbb","isCrossRepository":false,"headRepository":{"name":"other"},"headRepositoryOwner":{"login":"someone"},"url":"https://github.com/someone/other/pull/7"}"#,
    );
    let Some(repo) = TestRepo::new() else { return };
    repo.write("file.txt", "base\n");
    repo.commit_all("base");
    repo.git([
        "remote",
        "add",
        "origin",
        "https://github.com/nnayz/zeus.git",
    ]);
    let tools = GitTools {
        git: PathBuf::from("git"),
        gh: fake_gh,
        locks: Default::default(),
    };
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [];
    let git = repo.git_session(&session, &records, &tools, &local);
    let input = "https://github.com/someone/other/pull/7";
    let resolved = git.pr_resolve(input).unwrap();
    assert!(!resolved.same_repository);
    assert_eq!(git.pr_open(input).unwrap_err().code(), "cross_repository");
}

#[test]
fn live_owner_helper_prefers_the_running_session() {
    let live = test_record("s_live", "/repo", SessionStatus::Working, "Codex");
    let dead = test_record(
        "s_dead",
        "/repo",
        SessionStatus::Exited(zeus_proto::ExitInfo {
            reason: zeus_proto::ExitReason::Exited,
            code: Some(0),
            signal: None,
        }),
        "old",
    );
    let owner = owner_for_path(&[dead, live], "/repo").unwrap();
    assert_eq!(owner.session_id.0, "s_live");
    assert!(owner.live);
}

#[test]
fn patch_stage_rejects_stale_hunks() {
    let Some(repo) = TestRepo::new() else { return };
    repo.write("stale.txt", "before\n");
    repo.commit_all("base");
    repo.write("stale.txt", "first edit\n");
    let patch = Command::new("git")
        .args(["diff", "--no-ext-diff", "--no-color", "--", "stale.txt"])
        .current_dir(&repo.path)
        .output()
        .unwrap()
        .stdout;
    repo.write("stale.txt", "overlapping newer edit\n");
    let tools = GitTools::new();
    let local = tools.local();
    let session = repo.session("s1", SessionStatus::Idle);
    let records = [session.clone()];
    let git = repo.git_session(&session, &records, &tools, &local);
    let error = git
        .apply_patch(&patch, GitPatchMutation::Stage)
        .unwrap_err();
    assert_eq!(error.code(), "patch_does_not_apply");
    let cached = Command::new("git")
        .args(["diff", "--cached"])
        .current_dir(&repo.path)
        .output()
        .unwrap()
        .stdout;
    assert!(cached.is_empty());
}
