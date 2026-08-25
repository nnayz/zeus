//! Engine-owned Git workspace: Review mutations, branch navigation, and
//! pull-request targeting.
//!
//! Local sessions run `git` / `gh` with structured argv. Remote sessions use
//! one fixed POSIX script whose dynamic values travel on stdin, never in the
//! SSH command. Mutations are serialized per worktree. Checkout never
//! auto-stashes, force-resets, or switches under a live owner.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use zeus_proto::{
    GitBranchInfo, GitChangeKind, GitCheckoutBlock, GitCheckoutDisposition, GitCheckoutMode,
    GitCheckoutPlan, GitCheckoutResult, GitCommitResult, GitCompareResult, GitFileChange,
    GitListRefsResult, GitPatchMutation, GitPrResolveResult, GitRefEntry, GitRefKind,
    GitReviewStatus, GitReviewTarget, GitWorkspaceOwner, GitWorkspaceResult, PullRequestStatus,
    SessionRecord, SessionStatus,
};

use crate::remote::manager::RemoteManager;
use zeus_proto::HostEntry;

const GIT_TIMEOUT: Duration = Duration::from_secs(20);
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
const GH_AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const PR_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_STDOUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 128 * 1024;
const MAX_COMMIT_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_PATCH_BYTES: usize = 4 * 1024 * 1024;
const MAX_STATUS_ENTRIES: usize = 20_000;
const MAX_REFS: usize = 2_000;
const MAX_QUERY_BYTES: usize = 256;
const MAX_REF_BYTES: usize = 255;
const MAX_PR_INPUT_BYTES: usize = 512;
const MAX_GIT_ARGS: usize = 1_024;
const MAX_REMOTE_INPUT_BYTES: usize = 5 * 1024 * 1024;

/// Fixed remote Git entry point. Cwd, argc, and argv arrive as lines on
/// stdin; remaining stdin is forwarded to Git. User data is never
/// interpolated into this script.
const REMOTE_GIT_RUN_SCRIPT: &str = r#"set -e
export LC_ALL=C LANG=C LANGUAGE=C
export GIT_TERMINAL_PROMPT=0 GIT_OPTIONAL_LOCKS=0 GIT_ASKPASS=true
export SSH_ASKPASS=true GCM_INTERACTIVE=never GIT_EDITOR=true
export GIT_SEQUENCE_EDITOR=true GIT_MERGE_AUTOEDIT=no
IFS= read -r cwd
case "$cwd" in
  "~") cwd=$HOME ;;
  "~/"*) cwd=$HOME/${cwd#\~/} ;;
esac
cd "$cwd"
command -v git >/dev/null 2>&1 || { printf '%s\n' 'git is not installed on this host' >&2; exit 127; }
IFS= read -r nargs
case "$nargs" in
  ''|*[!0-9]*) printf '%s\n' 'invalid git argc' >&2; exit 2 ;;
esac
i=0
set --
while [ "$i" -lt "$nargs" ]; do
  IFS= read -r arg
  set -- "$@" "$arg"
  i=$((i + 1))
done
exec git --no-pager -c color.ui=false -c core.fsmonitor=false "$@"
"#;

#[derive(Debug)]
pub enum GitWorkspaceError {
    NotRepository(String),
    CouldNotRunGit {
        operation: &'static str,
        source: io::Error,
    },
    GitFailed {
        operation: &'static str,
        exit_code: Option<i32>,
        message: String,
    },
    TimedOut {
        operation: &'static str,
        timeout: Duration,
    },
    OutputTooLarge {
        operation: &'static str,
        limit: usize,
    },
    InvalidPath {
        path: String,
        reason: &'static str,
    },
    EmptySelection,
    EmptyPatch,
    InvalidPatch {
        reason: &'static str,
    },
    PatchTooLarge {
        size: usize,
        limit: usize,
    },
    PatchDoesNotApply {
        mutation: GitPatchMutation,
        message: String,
    },
    EmptyCommitMessage,
    CommitMessageTooLarge {
        size: usize,
        limit: usize,
    },
    MalformedStatus(String),
    Blocked {
        reasons: Vec<GitCheckoutBlock>,
    },
    MissingTool {
        tool: &'static str,
        message: String,
    },
    Unauthenticated {
        tool: &'static str,
        message: String,
    },
    StaleRef {
        name: String,
        message: String,
    },
    InvalidInput(String),
    CrossRepository {
        url: String,
    },
    NoRemote,
    TransportUnavailable,
}

impl GitWorkspaceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotRepository(_) => "not_repository",
            Self::CouldNotRunGit { .. } => "could_not_run_git",
            Self::GitFailed { .. } => "git_failed",
            Self::TimedOut { .. } => "timeout",
            Self::OutputTooLarge { .. } => "output_too_large",
            Self::InvalidPath { .. } => "invalid_path",
            Self::EmptySelection => "empty_selection",
            Self::EmptyPatch => "empty_patch",
            Self::InvalidPatch { .. } => "invalid_patch",
            Self::PatchTooLarge { .. } => "patch_too_large",
            Self::PatchDoesNotApply { .. } => "patch_does_not_apply",
            Self::EmptyCommitMessage => "empty_commit",
            Self::CommitMessageTooLarge { .. } => "commit_too_large",
            Self::MalformedStatus(_) => "malformed_status",
            Self::Blocked { .. } => "blocked",
            Self::MissingTool { .. } => "missing_tool",
            Self::Unauthenticated { .. } => "unauthenticated",
            Self::StaleRef { .. } => "stale_ref",
            Self::InvalidInput(_) => "invalid_input",
            Self::CrossRepository { .. } => "cross_repository",
            Self::NoRemote => "no_remote",
            Self::TransportUnavailable => "remote_transport_unavailable",
        }
    }

    pub fn into_control(self) -> zeus_proto::ControlError {
        zeus_proto::ControlError::new(self.code(), self.to_string())
    }
}

impl fmt::Display for GitWorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRepository(path) => {
                write!(formatter, "{path} is not inside a non-bare Git repository")
            }
            Self::CouldNotRunGit { operation, source } => {
                write!(formatter, "could not run Git while {operation}: {source}")
            }
            Self::GitFailed {
                operation,
                exit_code,
                message,
            } => {
                let exit = exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "signal".to_owned());
                if message.is_empty() {
                    write!(formatter, "Git failed while {operation} (exit {exit})")
                } else {
                    write!(
                        formatter,
                        "Git failed while {operation} (exit {exit}): {message}"
                    )
                }
            }
            Self::TimedOut { operation, timeout } => write!(
                formatter,
                "Git timed out after {:.0}s while {operation}",
                timeout.as_secs_f32()
            ),
            Self::OutputTooLarge { operation, limit } => write!(
                formatter,
                "Git produced more than {limit} bytes while {operation}"
            ),
            Self::InvalidPath { path, reason } => {
                write!(formatter, "unsafe repository path {path}: {reason}")
            }
            Self::EmptySelection => formatter.write_str("select at least one changed path"),
            Self::EmptyPatch => formatter.write_str("review patch cannot be empty"),
            Self::InvalidPatch { reason } => write!(formatter, "invalid review patch: {reason}"),
            Self::PatchTooLarge { size, limit } => {
                write!(formatter, "review patch is {size} bytes (limit {limit})")
            }
            Self::PatchDoesNotApply { mutation, message } => {
                write!(
                    formatter,
                    "review patch is stale or no longer applies while {}: {message}",
                    mutation_label(*mutation)
                )
            }
            Self::EmptyCommitMessage => formatter.write_str("commit message cannot be empty"),
            Self::CommitMessageTooLarge { size, limit } => {
                write!(formatter, "commit message is {size} bytes (limit {limit})")
            }
            Self::MalformedStatus(message) => {
                write!(formatter, "Git returned malformed status data: {message}")
            }
            Self::Blocked { reasons } => {
                let detail = reasons
                    .iter()
                    .map(|reason| reason.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                write!(formatter, "checkout blocked: {detail}")
            }
            Self::MissingTool { tool, message } => {
                write!(formatter, "{tool} is not available: {message}")
            }
            Self::Unauthenticated { tool, message } => {
                write!(formatter, "{tool} is not authenticated: {message}")
            }
            Self::StaleRef { name, message } => {
                write!(formatter, "ref {name} is missing or stale: {message}")
            }
            Self::InvalidInput(message) => formatter.write_str(message),
            Self::CrossRepository { url } => write!(
                formatter,
                "pull request {url} belongs to another repository; open it only from a checkout of that repository"
            ),
            Self::NoRemote => formatter.write_str("this repository has no remotes to fetch"),
            Self::TransportUnavailable => formatter
                .write_str("remote Git operations need the packaged Remote Helper transport"),
        }
    }
}

impl std::error::Error for GitWorkspaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CouldNotRunGit { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn mutation_label(mutation: GitPatchMutation) -> &'static str {
    match mutation {
        GitPatchMutation::Stage => "staging",
        GitPatchMutation::Unstage => "unstaging",
        GitPatchMutation::Discard => "discarding",
    }
}

#[derive(Default)]
struct MutationLockState {
    held: Mutex<HashSet<String>>,
    ready: Condvar,
}

#[derive(Clone, Default)]
pub struct MutationLocks {
    inner: Arc<MutationLockState>,
}

impl MutationLocks {
    fn acquire(&self, key: &str) -> Result<MutationGuard, GitWorkspaceError> {
        let mut held =
            self.inner.held.lock().map_err(|_| {
                GitWorkspaceError::InvalidInput("git mutation lock poisoned".into())
            })?;
        while held.contains(key) {
            held = self.inner.ready.wait(held).map_err(|_| {
                GitWorkspaceError::InvalidInput("git mutation lock poisoned".into())
            })?;
        }
        held.insert(key.to_owned());
        drop(held);
        Ok(MutationGuard {
            locks: self.clone(),
            key: key.to_owned(),
        })
    }
}

struct MutationGuard {
    locks: MutationLocks,
    key: String,
}

impl Drop for MutationGuard {
    fn drop(&mut self) {
        if let Ok(mut held) = self.locks.inner.held.lock() {
            held.remove(&self.key);
            drop(held);
            self.locks.inner.ready.notify_all();
        }
    }
}

pub struct LocalGit {
    pub program: PathBuf,
}

impl Default for LocalGit {
    fn default() -> Self {
        Self {
            program: PathBuf::from("git"),
        }
    }
}

pub struct GhClient {
    pub program: PathBuf,
}

impl Default for GhClient {
    fn default() -> Self {
        Self {
            program: PathBuf::from("gh"),
        }
    }
}

#[derive(Debug)]
pub struct GitOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl GitOutput {
    fn stderr_message(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_owned()
    }

    fn failure(&self, operation: &'static str) -> GitWorkspaceError {
        GitWorkspaceError::GitFailed {
            operation,
            exit_code: self.status.code(),
            message: self.stderr_message(),
        }
    }
}

fn ensure_success(
    output: GitOutput,
    operation: &'static str,
) -> Result<GitOutput, GitWorkspaceError> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(output.failure(operation))
    }
}

pub trait GitExecutor {
    fn run(
        &self,
        cwd: &str,
        args: &[&str],
        input: Option<&[u8]>,
        operation: &'static str,
        timeout: Duration,
    ) -> Result<GitOutput, GitWorkspaceError>;
}

impl GitExecutor for LocalGit {
    fn run(
        &self,
        cwd: &str,
        args: &[&str],
        input: Option<&[u8]>,
        operation: &'static str,
        timeout: Duration,
    ) -> Result<GitOutput, GitWorkspaceError> {
        if args.len() > MAX_GIT_ARGS {
            return Err(GitWorkspaceError::InvalidInput(format!(
                "too many Git arguments while {operation}"
            )));
        }
        let mut command = Command::new(&self.program);
        command
            .current_dir(cwd)
            .arg("--no-pager")
            .arg("-c")
            .arg("color.ui=false")
            .arg("-c")
            .arg("core.fsmonitor=false")
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "true")
            .env("SSH_ASKPASS", "true")
            .env("GCM_INTERACTIVE", "never")
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .env("LC_ALL", "C")
            .env("GIT_OPTIONAL_LOCKS", "0");
        spawn_bounded(command, input, operation, timeout).map_err(|error| match error {
            GitWorkspaceError::CouldNotRunGit { source, .. }
                if source.kind() == io::ErrorKind::NotFound =>
            {
                GitWorkspaceError::MissingTool {
                    tool: "git",
                    message: "install Git and ensure it is available on PATH".into(),
                }
            }
            other => other,
        })
    }
}

pub struct RemoteGit<'a> {
    pub manager: &'a RemoteManager,
    pub host: &'a HostEntry,
}

impl GitExecutor for RemoteGit<'_> {
    fn run(
        &self,
        cwd: &str,
        args: &[&str],
        input: Option<&[u8]>,
        operation: &'static str,
        timeout: Duration,
    ) -> Result<GitOutput, GitWorkspaceError> {
        if cwd.is_empty() || cwd.contains(['\r', '\n', '\0']) {
            return Err(GitWorkspaceError::InvalidInput(
                "session cwd is not representable by remote Git".into(),
            ));
        }
        if args.len() > MAX_GIT_ARGS {
            return Err(GitWorkspaceError::InvalidInput(format!(
                "too many Git arguments while {operation}"
            )));
        }
        for arg in args {
            if arg.contains(['\r', '\n', '\0']) {
                return Err(GitWorkspaceError::InvalidInput(format!(
                    "Git argument is not representable while {operation}"
                )));
            }
        }
        let payload_size = cwd.len()
            + args.iter().map(|arg| arg.len() + 1).sum::<usize>()
            + input.map_or(0, <[u8]>::len)
            + 32;
        if payload_size > MAX_REMOTE_INPUT_BYTES {
            return Err(GitWorkspaceError::InvalidInput(format!(
                "remote Git input is too large while {operation}"
            )));
        }
        let mut payload = Vec::new();
        payload.extend_from_slice(cwd.as_bytes());
        payload.push(b'\n');
        payload.extend_from_slice(args.len().to_string().as_bytes());
        payload.push(b'\n');
        for arg in args {
            payload.extend_from_slice(arg.as_bytes());
            payload.push(b'\n');
        }
        if let Some(input) = input {
            payload.extend_from_slice(input);
        }
        let output = self
            .manager
            .run_fixed_script(
                self.host,
                REMOTE_GIT_RUN_SCRIPT,
                payload,
                timeout,
                MAX_STDOUT_BYTES + MAX_STDERR_BYTES,
            )
            .map_err(|source| GitWorkspaceError::CouldNotRunGit { operation, source })?;
        if output.stdout_truncated || output.stdout.len() > MAX_STDOUT_BYTES {
            return Err(GitWorkspaceError::OutputTooLarge {
                operation,
                limit: MAX_STDOUT_BYTES,
            });
        }
        let mut stderr = output.stderr;
        if output.stderr_truncated {
            stderr.extend_from_slice(b"\n[stderr truncated]");
        }
        Ok(GitOutput {
            status: output.status,
            stdout: output.stdout,
            stderr,
        })
    }
}

fn spawn_bounded(
    mut command: Command,
    input: Option<&[u8]>,
    operation: &'static str,
    timeout: Duration,
) -> Result<GitOutput, GitWorkspaceError> {
    let mut child = command
        .spawn()
        .map_err(|source| GitWorkspaceError::CouldNotRunGit { operation, source })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stdout_reader = thread::spawn(move || read_bounded(stdout, MAX_STDOUT_BYTES));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, MAX_STDERR_BYTES));
    let stdin_writer = input.map(|input| {
        let input = input.to_vec();
        let mut stdin = child.stdin.take().expect("piped stdin");
        thread::spawn(move || {
            let result = stdin.write_all(&input);
            drop(stdin);
            result
        })
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(GitWorkspaceError::TimedOut { operation, timeout });
            }
            Err(source) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(GitWorkspaceError::CouldNotRunGit { operation, source });
            }
        }
    };
    if let Some(writer) = stdin_writer {
        match writer.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) if error.kind() == io::ErrorKind::BrokenPipe => {}
            Ok(Err(source)) => {
                return Err(GitWorkspaceError::CouldNotRunGit { operation, source });
            }
            Err(_) => {
                return Err(GitWorkspaceError::CouldNotRunGit {
                    operation,
                    source: io::Error::other("Git stdin writer panicked"),
                });
            }
        }
    }
    let (stdout, stdout_truncated) = join_reader(stdout_reader, operation)?;
    let (stderr, stderr_truncated) = join_reader(stderr_reader, operation)?;
    if stdout_truncated {
        return Err(GitWorkspaceError::OutputTooLarge {
            operation,
            limit: MAX_STDOUT_BYTES,
        });
    }
    let stderr = if stderr_truncated {
        let mut stderr = stderr;
        stderr.extend_from_slice(b"\n[stderr truncated]");
        stderr
    } else {
        stderr
    };
    Ok(GitOutput {
        status,
        stdout,
        stderr,
    })
}

fn read_bounded<R: Read>(mut reader: R, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut truncated = false;
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..count.min(remaining)]);
        truncated |= count > remaining;
    }
    Ok((bytes, truncated))
}

fn join_reader(
    reader: thread::JoinHandle<io::Result<(Vec<u8>, bool)>>,
    operation: &'static str,
) -> Result<(Vec<u8>, bool), GitWorkspaceError> {
    match reader.join() {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(source)) => Err(GitWorkspaceError::CouldNotRunGit { operation, source }),
        Err(_) => Err(GitWorkspaceError::CouldNotRunGit {
            operation,
            source: io::Error::other("Git output reader panicked"),
        }),
    }
}

#[derive(Clone, Default)]
pub struct GitTools {
    pub git: PathBuf,
    pub gh: PathBuf,
    pub locks: MutationLocks,
}

impl GitTools {
    pub fn new() -> Self {
        Self {
            git: PathBuf::from("git"),
            gh: PathBuf::from("gh"),
            locks: MutationLocks::default(),
        }
    }

    pub fn local(&self) -> LocalGit {
        LocalGit {
            program: self.git.clone(),
        }
    }

    pub fn gh(&self) -> GhClient {
        GhClient {
            program: self.gh.clone(),
        }
    }
}

pub struct SessionGit<'a> {
    pub session: &'a SessionRecord,
    pub records: &'a [SessionRecord],
    pub executor: &'a dyn GitExecutor,
    pub gh: GhClient,
    pub locks: &'a MutationLocks,
}

struct BranchTarget {
    local_name: String,
    start_point: Option<String>,
}

impl SessionGit<'_> {
    fn acquire_mutation(&self, root: &str) -> Result<MutationGuard, GitWorkspaceError> {
        let key = match self.session.host.as_deref() {
            Some(host) => format!("remote\0{host}\0{root}"),
            None => format!("local\0{root}"),
        };
        self.locks.acquire(&key)
    }

    pub fn workspace(
        &self,
        target: GitReviewTarget,
        pull_request: Option<PullRequestStatus>,
    ) -> Result<GitWorkspaceResult, GitWorkspaceError> {
        let root = self.discover_root()?;
        let status = self.status_at(&root)?;
        let origin_url = self.origin_url(&root).map(|url| sanitize_remote_url(&url));
        let repository = origin_url.as_deref().and_then(github_repository);
        let owner = owner_for_location(self.records, &root, self.session.host.as_deref());
        let dirty = !status.unstaged.is_empty()
            || !status.untracked.is_empty()
            || !status.staged.is_empty()
            || !status.conflicted.is_empty();
        Ok(GitWorkspaceResult {
            session_id: self.session.id.clone(),
            worktree_path: root.clone(),
            linked_worktree: self.is_linked_worktree(&root)?,
            origin_url,
            repository,
            branch: status.branch.clone(),
            dirty,
            conflicted: !status.conflicted.is_empty(),
            unborn: status.branch.oid.is_none() && status.branch.name.is_some(),
            detached: status.branch.name.is_none(),
            owner,
            target,
            status: GitReviewStatus {
                repo_root: root.clone(),
                ..status
            },
            repo_root: root,
            pull_request,
        })
    }

    pub fn stage(&self, paths: &[String]) -> Result<GitWorkspaceResult, GitWorkspaceError> {
        let root = self.discover_root()?;
        let _guard = self.acquire_mutation(&root)?;
        let paths = validate_paths(paths)?;
        let mut args = vec!["--literal-pathspecs", "add", "--"];
        let owned: Vec<&str> = paths.iter().map(String::as_str).collect();
        args.extend(owned);
        self.mutate(&root, &args, None, "staging paths")?;
        self.workspace(GitReviewTarget::WorkingTree, None)
    }

    pub fn unstage(&self, paths: &[String]) -> Result<GitWorkspaceResult, GitWorkspaceError> {
        let root = self.discover_root()?;
        let _guard = self.acquire_mutation(&root)?;
        let paths = self.with_rename_sources(&root, paths)?;
        let paths = validate_paths(&paths)?;
        let head = self.executor.run(
            &root,
            &["rev-parse", "--verify", "--quiet", "HEAD"],
            None,
            "checking HEAD",
            GIT_TIMEOUT,
        )?;
        let mut prefix: Vec<&str> = if head.status.success() {
            vec!["--literal-pathspecs", "reset", "--quiet", "HEAD", "--"]
        } else if head.status.code() == Some(1) {
            vec![
                "--literal-pathspecs",
                "rm",
                "--quiet",
                "-r",
                "-f",
                "--cached",
                "--ignore-unmatch",
                "--",
            ]
        } else {
            return Err(head.failure("checking HEAD"));
        };
        let owned: Vec<&str> = paths.iter().map(String::as_str).collect();
        prefix.extend(owned);
        self.mutate(&root, &prefix, None, "unstaging paths")?;
        self.workspace(GitReviewTarget::WorkingTree, None)
    }

    pub fn discard(&self, paths: &[String]) -> Result<GitWorkspaceResult, GitWorkspaceError> {
        let root = self.discover_root()?;
        let _guard = self.acquire_mutation(&root)?;
        let paths = validate_paths(paths)?;
        let mut args = vec!["--literal-pathspecs", "restore", "--worktree", "--"];
        let owned: Vec<&str> = paths.iter().map(String::as_str).collect();
        args.extend(owned);
        self.mutate(&root, &args, None, "discarding unstaged changes")?;
        self.workspace(GitReviewTarget::WorkingTree, None)
    }

    pub fn apply_patch(
        &self,
        patch: &[u8],
        mutation: GitPatchMutation,
    ) -> Result<GitWorkspaceResult, GitWorkspaceError> {
        let root = self.discover_root()?;
        let _guard = self.acquire_mutation(&root)?;
        if patch.iter().all(u8::is_ascii_whitespace) {
            return Err(GitWorkspaceError::EmptyPatch);
        }
        if patch.len() > MAX_PATCH_BYTES {
            return Err(GitWorkspaceError::PatchTooLarge {
                size: patch.len(),
                limit: MAX_PATCH_BYTES,
            });
        }
        if patch.contains(&0) {
            return Err(GitWorkspaceError::InvalidPatch {
                reason: "patch contains a NUL byte",
            });
        }
        if mutation == GitPatchMutation::Discard && patch_creates_file(patch) {
            return Err(GitWorkspaceError::InvalidPatch {
                reason: "discard cannot delete an untracked or newly added file",
            });
        }
        let mut options = vec!["apply", "--recount", "--whitespace=nowarn"];
        match mutation {
            GitPatchMutation::Stage => options.push("--cached"),
            GitPatchMutation::Unstage => {
                options.push("--cached");
                options.push("--reverse");
            }
            GitPatchMutation::Discard => options.push("--reverse"),
        }
        if mutation == GitPatchMutation::Stage {
            let worktree_check = self.executor.run(
                &root,
                &[
                    "apply",
                    "--recount",
                    "--whitespace=nowarn",
                    "--reverse",
                    "--check",
                ],
                Some(patch),
                "checking review patch against worktree",
                GIT_TIMEOUT,
            )?;
            if !worktree_check.status.success() {
                return Err(patch_rejected(worktree_check, mutation));
            }
        }
        let mut check_options = options.clone();
        check_options.push("--check");
        let preflight = self.executor.run(
            &root,
            &check_options,
            Some(patch),
            "checking review patch",
            GIT_TIMEOUT,
        )?;
        if !preflight.status.success() {
            return Err(patch_rejected(preflight, mutation));
        }
        let applied = self.executor.run(
            &root,
            &options,
            Some(patch),
            "applying review patch",
            GIT_TIMEOUT,
        )?;
        if !applied.status.success() {
            return Err(patch_rejected(applied, mutation));
        }
        self.workspace(GitReviewTarget::WorkingTree, None)
    }

    pub fn commit(&self, message: &str) -> Result<GitCommitResult, GitWorkspaceError> {
        let root = self.discover_root()?;
        let _guard = self.acquire_mutation(&root)?;
        if message.trim().is_empty() {
            return Err(GitWorkspaceError::EmptyCommitMessage);
        }
        if message.len() > MAX_COMMIT_MESSAGE_BYTES {
            return Err(GitWorkspaceError::CommitMessageTooLarge {
                size: message.len(),
                limit: MAX_COMMIT_MESSAGE_BYTES,
            });
        }
        if message.as_bytes().contains(&0) {
            return Err(GitWorkspaceError::GitFailed {
                operation: "validating commit message",
                exit_code: None,
                message: "commit message contains a NUL byte".to_owned(),
            });
        }
        let output = self.executor.run(
            &root,
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgSign=false",
                "commit",
                "--quiet",
                "--file=-",
                "--cleanup=strip",
            ],
            Some(message.as_bytes()),
            "creating commit",
            GIT_TIMEOUT,
        )?;
        ensure_success(output, "creating commit")?;
        let identity = self.executor.run(
            &root,
            &["show", "-s", "--format=%H%x00%s", "HEAD"],
            None,
            "reading new commit",
            GIT_TIMEOUT,
        )?;
        let identity = ensure_success(identity, "reading new commit")?;
        let identity = trim_line_ending(&identity.stdout);
        let separator = identity.iter().position(|byte| *byte == 0).ok_or_else(|| {
            GitWorkspaceError::MalformedStatus("new commit identity has no separator".to_owned())
        })?;
        let oid = String::from_utf8_lossy(&identity[..separator]).into_owned();
        if oid.is_empty() {
            return Err(GitWorkspaceError::MalformedStatus(
                "new commit object id is empty".to_owned(),
            ));
        }
        let summary = String::from_utf8_lossy(&identity[separator + 1..]).into_owned();
        Ok(GitCommitResult {
            oid,
            summary,
            workspace: self.workspace(GitReviewTarget::WorkingTree, None)?,
        })
    }

    pub fn list_refs(&self, query: Option<&str>) -> Result<GitListRefsResult, GitWorkspaceError> {
        let root = self.discover_root()?;
        if let Some(query) = query
            && query.len() > MAX_QUERY_BYTES
        {
            return Err(GitWorkspaceError::InvalidInput(
                "ref search is too long".into(),
            ));
        }
        let worktrees = self.list_worktrees(&root)?;
        let output = self.executor.run(
            &root,
            &[
                "for-each-ref",
                "--format=%(refname)%00%(refname:short)%00%(objectname:short)%00%(upstream:short)%00%(HEAD)%00%(upstream:track)%00%(symref)",
                "refs/heads",
                "refs/remotes",
            ],
            None,
            "listing refs",
            GIT_TIMEOUT,
        )?;
        let output = ensure_success(output, "listing refs")?;
        let query = query.map(str::to_ascii_lowercase);
        let mut refs = Vec::new();
        let mut truncated = false;
        for record in output.stdout.split(|byte| *byte == b'\n') {
            if record.is_empty() {
                continue;
            }
            let fields: Vec<&[u8]> = record.split(|byte| *byte == 0).collect();
            if fields.len() < 7 || !fields[6].is_empty() {
                continue;
            }
            let name = String::from_utf8_lossy(fields[0]).into_owned();
            let short_name = String::from_utf8_lossy(fields[1]).into_owned();
            if short_name.is_empty() {
                continue;
            }
            if let Some(query) = &query
                && !short_name.to_ascii_lowercase().contains(query)
                && !name.to_ascii_lowercase().contains(query)
            {
                continue;
            }
            if refs.len() >= MAX_REFS {
                truncated = true;
                break;
            }
            let kind = if name.starts_with("refs/heads/") {
                GitRefKind::Local
            } else {
                GitRefKind::Remote
            };
            let (ahead, behind) = parse_track(fields[5]);
            let worktree_path = worktrees.iter().find_map(|(path, branch)| {
                (branch.as_deref() == Some(short_name.as_str())).then(|| path.clone())
            });
            let owner = worktree_path.as_deref().and_then(|path| {
                owner_for_location(self.records, path, self.session.host.as_deref())
            });
            refs.push(GitRefEntry {
                name,
                short_name,
                kind,
                oid: String::from_utf8_lossy(fields[2]).into_owned(),
                current: fields[4] == b"*",
                upstream: nonempty_lossy(fields[3]),
                ahead,
                behind,
                worktree_path,
                owner,
            });
        }
        refs.sort_by(|left, right| {
            (left.kind as u8, !left.current, left.short_name.as_str()).cmp(&(
                right.kind as u8,
                !right.current,
                right.short_name.as_str(),
            ))
        });
        Ok(GitListRefsResult { refs, truncated })
    }

    pub fn fetch(
        &self,
        remote: Option<&str>,
    ) -> Result<zeus_proto::GitFetchResult, GitWorkspaceError> {
        let root = self.discover_root()?;
        let _guard = self.acquire_mutation(&root)?;
        let remote = match remote {
            Some(remote) => {
                validate_ref(remote, "remote")?;
                remote.to_owned()
            }
            None => self.default_remote(&root)?,
        };
        let output = self.executor.run(
            &root,
            &["fetch", "--no-tags", "--prune", "--", &remote],
            None,
            "fetching refs",
            FETCH_TIMEOUT,
        )?;
        if !output.status.success() {
            return Err(GitWorkspaceError::GitFailed {
                operation: "fetching refs",
                exit_code: output.status.code(),
                message: "fetch failed; verify the remote and its authentication".into(),
            });
        }
        Ok(zeus_proto::GitFetchResult {
            remote,
            summary: if output.stderr.is_empty() {
                "Already up to date.".to_owned()
            } else {
                "Fetched the latest refs.".to_owned()
            },
        })
    }

    pub fn compare(&self, target: GitReviewTarget) -> Result<GitCompareResult, GitWorkspaceError> {
        let root = self.discover_root()?;
        match target {
            GitReviewTarget::WorkingTree => {
                let output = self.executor.run(
                    &root,
                    &[
                        "diff",
                        "--no-ext-diff",
                        "--no-color",
                        "--unified=3",
                        "HEAD",
                        "--",
                    ],
                    None,
                    "comparing working tree",
                    GIT_TIMEOUT,
                )?;
                if !output.status.success() && output.status.code() != Some(1) {
                    return Err(output.failure("comparing working tree"));
                }
                let truncated = output.stdout.len() > MAX_PATCH_BYTES;
                Ok(GitCompareResult {
                    patch: output.stdout[..output.stdout.len().min(MAX_PATCH_BYTES)].to_vec(),
                    repo_root: root,
                    truncated,
                    base_ref: Some("HEAD".into()),
                    head_ref: None,
                })
            }
            GitReviewTarget::Branch { ref_name } => {
                validate_ref(&ref_name, "branch")?;
                self.ensure_ref(&root, &ref_name)?;
                let output = self.executor.run(
                    &root,
                    &[
                        "diff",
                        "--no-ext-diff",
                        "--no-color",
                        "--unified=3",
                        &ref_name,
                        "--",
                    ],
                    None,
                    "comparing branch",
                    GIT_TIMEOUT,
                )?;
                if !output.status.success() && output.status.code() != Some(1) {
                    return Err(output.failure("comparing branch"));
                }
                let truncated = output.stdout.len() > MAX_PATCH_BYTES;
                Ok(GitCompareResult {
                    patch: output.stdout[..output.stdout.len().min(MAX_PATCH_BYTES)].to_vec(),
                    repo_root: root,
                    truncated,
                    base_ref: Some(ref_name),
                    head_ref: None,
                })
            }
            GitReviewTarget::PullRequest { url, number } => {
                let head = format!("refs/zeus/pr/{number}/head");
                if !self.ref_exists(&root, &head)? {
                    return Err(GitWorkspaceError::StaleRef {
                        name: url,
                        message: "fetch the pull request before comparing its diff".into(),
                    });
                }
                let base_ref = format!("refs/zeus/pr/{number}/base");
                let base = if self.ref_exists(&root, &base_ref)? {
                    base_ref
                } else {
                    let merge_base = self.executor.run(
                        &root,
                        &["merge-base", "HEAD", &head],
                        None,
                        "finding pull-request merge base",
                        GIT_TIMEOUT,
                    );
                    match merge_base {
                        Ok(output) if output.status.success() => {
                            String::from_utf8_lossy(trim_line_ending(&output.stdout)).into_owned()
                        }
                        _ => "HEAD".to_owned(),
                    }
                };
                let output = self.executor.run(
                    &root,
                    &[
                        "diff",
                        "--no-ext-diff",
                        "--no-color",
                        "--unified=3",
                        &format!("{base}...{head}"),
                        "--",
                    ],
                    None,
                    "comparing pull request",
                    GIT_TIMEOUT,
                )?;
                if !output.status.success() && output.status.code() != Some(1) {
                    return Err(output.failure("comparing pull request"));
                }
                let truncated = output.stdout.len() > MAX_PATCH_BYTES;
                Ok(GitCompareResult {
                    patch: output.stdout[..output.stdout.len().min(MAX_PATCH_BYTES)].to_vec(),
                    repo_root: root,
                    truncated,
                    base_ref: Some(base),
                    head_ref: Some(head),
                })
            }
        }
    }

    pub fn checkout_plan(&self, ref_name: &str) -> Result<GitCheckoutPlan, GitWorkspaceError> {
        let root = self.discover_root()?;
        validate_ref(ref_name, "branch")?;
        let resolved = self.resolve_branch_target(&root, ref_name)?.local_name;
        let listed = self.list_refs(None)?;
        if let Some(existing) = listed.refs.iter().find(|entry| {
            entry.kind == GitRefKind::Local
                && (entry.short_name == resolved || entry.short_name == ref_name)
                && entry
                    .worktree_path
                    .as_deref()
                    .is_some_and(|path| path != root)
        }) {
            return Ok(GitCheckoutPlan {
                ref_name: resolved,
                disposition: GitCheckoutDisposition::FocusExisting {
                    path: existing.worktree_path.clone().unwrap_or_default(),
                    session_id: existing
                        .owner
                        .as_ref()
                        .map(|owner| owner.session_id.clone()),
                },
                reasons: Vec::new(),
            });
        }
        let status = self.status_at(&root)?;
        let owner = owner_for_location(self.records, &root, self.session.host.as_deref());
        if status.branch.name.as_deref() == Some(resolved.as_str()) {
            return Ok(GitCheckoutPlan {
                ref_name: resolved,
                disposition: GitCheckoutDisposition::FocusExisting {
                    path: root,
                    session_id: owner.as_ref().map(|owner| owner.session_id.clone()),
                },
                reasons: Vec::new(),
            });
        }
        let mut reasons = Vec::new();
        if !status.conflicted.is_empty() {
            reasons.push(GitCheckoutBlock {
                code: "conflicted".into(),
                message: "the worktree has unresolved conflicts".into(),
            });
        }
        if !status.staged.is_empty() || !status.unstaged.is_empty() || !status.untracked.is_empty()
        {
            reasons.push(GitCheckoutBlock {
                code: "dirty".into(),
                message: "the index or working tree has uncommitted changes".into(),
            });
        }
        if owner.as_ref().is_some_and(|owner| owner.live) {
            reasons.push(GitCheckoutBlock {
                code: "liveOwner".into(),
                message: format!(
                    "a live agent owns this checkout ({})",
                    owner
                        .as_ref()
                        .map(|owner| owner.title.as_str())
                        .unwrap_or("session")
                ),
            });
        }
        if reasons.iter().any(|reason| reason.code == "liveOwner") {
            return Ok(GitCheckoutPlan {
                ref_name: resolved.clone(),
                disposition: GitCheckoutDisposition::OpenNewWorktree {
                    path: sibling_worktree_path(&root, &resolved),
                },
                reasons,
            });
        }
        if !reasons.is_empty() {
            return Ok(GitCheckoutPlan {
                ref_name: resolved,
                disposition: GitCheckoutDisposition::Blocked,
                reasons,
            });
        }
        Ok(GitCheckoutPlan {
            ref_name: resolved,
            disposition: GitCheckoutDisposition::SwitchInPlace,
            reasons,
        })
    }

    pub fn checkout(
        &self,
        ref_name: &str,
        mode: GitCheckoutMode,
    ) -> Result<GitCheckoutResult, GitWorkspaceError> {
        let root = self.discover_root()?;
        let _guard = self.acquire_mutation(&root)?;
        let target = self.resolve_branch_target(&root, ref_name)?;
        let mut plan = self.checkout_plan(ref_name)?;
        match mode {
            GitCheckoutMode::Switch => match &plan.disposition {
                GitCheckoutDisposition::SwitchInPlace => {
                    self.switch_in_place(&root, &target)?;
                }
                GitCheckoutDisposition::FocusExisting { .. } => {}
                GitCheckoutDisposition::OpenNewWorktree { .. } => {
                    let created = self.open_worktree(&root, &target)?;
                    plan.disposition = GitCheckoutDisposition::OpenNewWorktree { path: created };
                }
                GitCheckoutDisposition::Blocked => {
                    return Err(GitWorkspaceError::Blocked {
                        reasons: plan.reasons.clone(),
                    });
                }
            },
            GitCheckoutMode::Worktree => match &plan.disposition {
                GitCheckoutDisposition::FocusExisting { .. } => {}
                _ => {
                    let created = self.open_worktree(&root, &target)?;
                    plan.disposition = GitCheckoutDisposition::OpenNewWorktree { path: created };
                    plan.reasons.clear();
                }
            },
        }
        Ok(GitCheckoutResult {
            workspace: self.workspace(GitReviewTarget::WorkingTree, None)?,
            plan,
        })
    }

    pub fn branch_create(
        &self,
        name: &str,
        checkout: bool,
    ) -> Result<GitCheckoutResult, GitWorkspaceError> {
        let root = self.discover_root()?;
        let _guard = self.acquire_mutation(&root)?;
        validate_ref(name, "branch")?;
        self.check_ref_format(&root, name)?;
        if self.ref_exists(&root, name)? {
            return Err(GitWorkspaceError::InvalidInput(format!(
                "branch {name} already exists"
            )));
        }
        self.mutate(&root, &["branch", "--", name], None, "creating branch")?;
        if checkout {
            drop(_guard);
            return self.checkout(name, GitCheckoutMode::Switch);
        }
        Ok(GitCheckoutResult {
            plan: GitCheckoutPlan {
                ref_name: name.to_owned(),
                disposition: GitCheckoutDisposition::SwitchInPlace,
                reasons: Vec::new(),
            },
            workspace: self.workspace(GitReviewTarget::WorkingTree, None)?,
        })
    }

    pub fn pr_resolve(&self, input: &str) -> Result<GitPrResolveResult, GitWorkspaceError> {
        let root = self.discover_root()?;
        let origin = self.origin_url(&root);
        let repository = origin.as_deref().and_then(github_repository);
        let url = parse_pr_input(input, repository.as_deref())?;
        let status = self.view_pull_request(&url)?;
        let extra = self.pr_extra(&url)?;
        let target_repository = crate::pr_monitor::pr_coordinates(&url)
            .map(|(owner, repo, _)| format!("{owner}/{repo}"));
        let same_repository = match (repository.as_deref(), target_repository.as_deref()) {
            (Some(local), Some(target)) => local.eq_ignore_ascii_case(target),
            _ => false,
        };
        if same_repository {
            let head_source = format!("pull/{}/head", status.number);
            let head_destination = format!("refs/zeus/pr/{}/head", status.number);
            let head_refspec = format!("+{head_source}:{head_destination}");
            if let Ok(remote) = self.default_remote(&root) {
                let _guard = self.acquire_mutation(&root)?;
                let base_refspec = status.base_ref_name.as_deref().and_then(|base| {
                    validate_ref(base, "pull-request base")
                        .ok()
                        .map(|_| format!("+refs/heads/{base}:refs/zeus/pr/{}/base", status.number))
                });
                let mut args = vec!["fetch", "--no-tags", "--", remote.as_str(), &head_refspec];
                if let Some(base_refspec) = &base_refspec {
                    args.push(base_refspec);
                }
                let _ = self.executor.run(
                    &root,
                    &args,
                    None,
                    "fetching pull request ref",
                    FETCH_TIMEOUT,
                );
            }
        }
        Ok(GitPrResolveResult {
            status,
            head_oid: extra.head_oid,
            base_oid: extra.base_oid,
            is_cross_repository: extra.is_cross_repository,
            head_repository: extra.head_repository,
            same_repository,
        })
    }

    pub fn pr_open(&self, input: &str) -> Result<GitCheckoutResult, GitWorkspaceError> {
        let resolved = self.pr_resolve(input)?;
        self.open_resolved_pr(resolved)
    }

    pub(crate) fn open_resolved_pr(
        &self,
        resolved: GitPrResolveResult,
    ) -> Result<GitCheckoutResult, GitWorkspaceError> {
        if !resolved.same_repository {
            return Err(GitWorkspaceError::CrossRepository {
                url: resolved.status.url,
            });
        }
        let branch = if resolved.is_cross_repository {
            format!("pr/{}", resolved.status.number)
        } else {
            resolved
                .status
                .head_ref_name
                .clone()
                .unwrap_or_else(|| format!("pr/{}", resolved.status.number))
        };
        let spec = format!("refs/zeus/pr/{}/head", resolved.status.number);
        let root = self.discover_root()?;
        let _guard = self.acquire_mutation(&root)?;
        if !self.ref_exists(&root, &branch)? {
            if self.ref_exists(&root, &spec)? {
                self.mutate(
                    &root,
                    &["branch", "--", &branch, &spec],
                    None,
                    "creating pull-request branch",
                )?;
            } else {
                return Err(GitWorkspaceError::StaleRef {
                    name: branch,
                    message: "the pull request head is not available locally".into(),
                });
            }
        }
        drop(_guard);
        self.checkout(&branch, GitCheckoutMode::Worktree)
    }

    fn discover_root(&self) -> Result<String, GitWorkspaceError> {
        let cwd = &self.session.cwd;
        let output = self.executor.run(
            cwd,
            &["rev-parse", "--show-toplevel"],
            None,
            "finding repository",
            GIT_TIMEOUT,
        )?;
        if !output.status.success() {
            let message = output.stderr_message();
            if output.status.code() == Some(127)
                || message.contains("git is not installed on this host")
            {
                return Err(GitWorkspaceError::MissingTool {
                    tool: "git",
                    message: "install Git on this host and try again".into(),
                });
            }
            if message.contains("not a git repository") || message.contains("not a git work tree") {
                return Err(GitWorkspaceError::NotRepository(cwd.clone()));
            }
            return Err(output.failure("finding repository"));
        }
        let root = String::from_utf8_lossy(trim_line_ending(&output.stdout)).into_owned();
        if root.is_empty() {
            return Err(GitWorkspaceError::NotRepository(cwd.clone()));
        }
        let bare = self.executor.run(
            &root,
            &["rev-parse", "--is-bare-repository"],
            None,
            "checking repository",
            GIT_TIMEOUT,
        )?;
        let bare = ensure_success(bare, "checking repository")?;
        if String::from_utf8_lossy(&bare.stdout).trim() == "true" {
            return Err(GitWorkspaceError::NotRepository(cwd.clone()));
        }
        Ok(root)
    }

    fn status_at(&self, root: &str) -> Result<GitReviewStatus, GitWorkspaceError> {
        let output = self.executor.run(
            root,
            &[
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ],
            None,
            "reading status",
            GIT_TIMEOUT,
        )?;
        ensure_success(output, "reading status")
            .and_then(|output| parse_status(root, &output.stdout))
    }

    fn is_linked_worktree(&self, root: &str) -> Result<bool, GitWorkspaceError> {
        let git_dir = self.executor.run(
            root,
            &["rev-parse", "--git-dir"],
            None,
            "reading git dir",
            GIT_TIMEOUT,
        )?;
        let common = self.executor.run(
            root,
            &["rev-parse", "--git-common-dir"],
            None,
            "reading common git dir",
            GIT_TIMEOUT,
        )?;
        if !git_dir.status.success() || !common.status.success() {
            return Ok(false);
        }
        let git_dir = String::from_utf8_lossy(trim_line_ending(&git_dir.stdout));
        let common = String::from_utf8_lossy(trim_line_ending(&common.stdout));
        Ok(git_dir != common)
    }

    fn origin_url(&self, root: &str) -> Option<String> {
        let configured = self
            .executor
            .run(
                root,
                &["config", "--get", "remote.origin.url"],
                None,
                "reading origin",
                GIT_TIMEOUT,
            )
            .ok()?;
        if !configured.status.success() {
            return None;
        }
        let url = String::from_utf8_lossy(trim_line_ending(&configured.stdout))
            .trim()
            .to_owned();
        (!url.is_empty()).then_some(url)
    }

    fn default_remote(&self, root: &str) -> Result<String, GitWorkspaceError> {
        let output = self
            .executor
            .run(root, &["remote"], None, "listing remotes", GIT_TIMEOUT)?;
        let output = ensure_success(output, "listing remotes")?;
        let remotes = String::from_utf8_lossy(&output.stdout);
        let mut names = remotes.lines().filter(|line| !line.is_empty());
        let Some(first) = names.next() else {
            return Err(GitWorkspaceError::NoRemote);
        };
        if remotes.lines().any(|line| line == "origin") {
            Ok("origin".into())
        } else {
            Ok(first.to_owned())
        }
    }

    fn mutate(
        &self,
        root: &str,
        args: &[&str],
        input: Option<&[u8]>,
        operation: &'static str,
    ) -> Result<(), GitWorkspaceError> {
        let output = self
            .executor
            .run(root, args, input, operation, GIT_TIMEOUT)?;
        ensure_success(output, operation).map(|_| ())
    }

    fn with_rename_sources(
        &self,
        root: &str,
        paths: &[String],
    ) -> Result<Vec<String>, GitWorkspaceError> {
        let mut resolved = paths.to_vec();
        let renames: Vec<_> = self
            .status_at(root)?
            .staged
            .into_iter()
            .filter(|change| matches!(change.kind, GitChangeKind::Renamed | GitChangeKind::Copied))
            .filter_map(|change| change.original_path.map(|source| (change.path, source)))
            .collect();
        for (destination, source) in renames {
            if paths.iter().any(|path| path == &destination) && !resolved.contains(&source) {
                resolved.push(source);
            }
        }
        Ok(resolved)
    }

    fn list_worktrees(
        &self,
        root: &str,
    ) -> Result<Vec<(String, Option<String>)>, GitWorkspaceError> {
        let output = self.executor.run(
            root,
            &["worktree", "list", "--porcelain"],
            None,
            "listing worktrees",
            GIT_TIMEOUT,
        )?;
        let output = ensure_success(output, "listing worktrees")?;
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(crate::git::parse_porcelain(&text)
            .into_iter()
            .map(|info| (info.path, info.branch))
            .collect())
    }

    fn ensure_ref(&self, root: &str, name: &str) -> Result<(), GitWorkspaceError> {
        if self.ref_exists(root, name)? {
            Ok(())
        } else {
            Err(GitWorkspaceError::StaleRef {
                name: name.to_owned(),
                message: "the ref does not exist".into(),
            })
        }
    }

    fn ref_exists(&self, root: &str, name: &str) -> Result<bool, GitWorkspaceError> {
        let output = self.executor.run(
            root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{name}^{{commit}}"),
            ],
            None,
            "verifying ref",
            GIT_TIMEOUT,
        )?;
        Ok(output.status.success())
    }

    fn resolve_branch_target(
        &self,
        root: &str,
        name: &str,
    ) -> Result<BranchTarget, GitWorkspaceError> {
        let local_name = name.strip_prefix("refs/heads/").unwrap_or(name);
        if self.ref_exists(root, &format!("refs/heads/{local_name}"))? {
            return Ok(BranchTarget {
                local_name: local_name.to_owned(),
                start_point: None,
            });
        }

        let explicit_remote = name
            .strip_prefix("refs/remotes/")
            .or_else(|| (!name.starts_with("refs/") && name.contains('/')).then_some(name));
        let remote = if let Some(remote) = explicit_remote
            && self.ref_exists(root, &format!("refs/remotes/{remote}"))?
        {
            Some(remote.to_owned())
        } else if !name.starts_with("refs/") {
            if let Ok(remote) = self.default_remote(root) {
                let candidate = format!("{remote}/{name}");
                self.ref_exists(root, &format!("refs/remotes/{candidate}"))?
                    .then_some(candidate)
            } else {
                None
            }
        } else {
            None
        };
        let Some(remote) = remote else {
            return Err(GitWorkspaceError::StaleRef {
                name: name.to_owned(),
                message: "the branch does not exist locally; fetch and try again".into(),
            });
        };
        let Some((_, remote_branch)) = remote.split_once('/') else {
            return Err(GitWorkspaceError::StaleRef {
                name: name.to_owned(),
                message: "the remote-tracking branch name is malformed".into(),
            });
        };
        if self.ref_exists(root, &format!("refs/heads/{remote_branch}"))? {
            return Ok(BranchTarget {
                local_name: remote_branch.to_owned(),
                start_point: None,
            });
        }
        Ok(BranchTarget {
            local_name: remote_branch.to_owned(),
            start_point: Some(remote),
        })
    }

    fn check_ref_format(&self, root: &str, name: &str) -> Result<(), GitWorkspaceError> {
        let output = self.executor.run(
            root,
            &["check-ref-format", "--branch", name],
            None,
            "validating branch name",
            GIT_TIMEOUT,
        )?;
        if output.status.success() {
            Ok(())
        } else {
            Err(GitWorkspaceError::InvalidInput(format!(
                "{name} is not a valid Git branch name"
            )))
        }
    }

    fn switch_in_place(&self, root: &str, target: &BranchTarget) -> Result<(), GitWorkspaceError> {
        if let Some(start_point) = &target.start_point {
            self.mutate(
                root,
                &[
                    "switch",
                    "--track",
                    "-c",
                    &target.local_name,
                    "--",
                    start_point,
                ],
                None,
                "switching branch",
            )
        } else {
            self.mutate(
                root,
                &["switch", "--", &target.local_name],
                None,
                "switching branch",
            )
        }
    }

    fn open_worktree(
        &self,
        root: &str,
        target: &BranchTarget,
    ) -> Result<String, GitWorkspaceError> {
        let path = sibling_worktree_path(root, &target.local_name);
        if let Some(start_point) = &target.start_point {
            self.mutate(
                root,
                &[
                    "worktree",
                    "add",
                    "--track",
                    "-b",
                    &target.local_name,
                    "--",
                    &path,
                    start_point,
                ],
                None,
                "opening worktree",
            )?;
        } else {
            self.mutate(
                root,
                &["worktree", "add", "--", &path, &target.local_name],
                None,
                "opening worktree",
            )?;
        }
        Ok(path)
    }

    fn view_pull_request(&self, url: &str) -> Result<PullRequestStatus, GitWorkspaceError> {
        self.ensure_gh_auth()?;
        crate::pr_monitor::fetch(url, &self.gh.program.to_string_lossy(), true).ok_or_else(|| {
            GitWorkspaceError::InvalidInput(format!(
                "could not load pull request {url}; check the number and gh authentication"
            ))
        })
    }

    fn pr_extra(&self, url: &str) -> Result<PrExtra, GitWorkspaceError> {
        let output = self.gh_run(
            &[
                "pr",
                "view",
                url,
                "--json",
                "headRefOid,baseRefOid,isCrossRepository,headRepository,headRepositoryOwner,url",
            ],
            PR_TIMEOUT,
            "reading pull request",
        )?;
        let value: serde_json::Value = serde_json::from_slice(&output).map_err(|_| {
            GitWorkspaceError::MalformedStatus("pull request metadata is not JSON".into())
        })?;
        let head_repository = value["headRepositoryOwner"]["login"]
            .as_str()
            .zip(value["headRepository"]["name"].as_str())
            .map(|(owner, repo)| format!("{owner}/{repo}"));
        Ok(PrExtra {
            head_oid: value["headRefOid"].as_str().map(str::to_owned),
            base_oid: value["baseRefOid"].as_str().map(str::to_owned),
            is_cross_repository: value["isCrossRepository"].as_bool().unwrap_or(false),
            head_repository,
        })
    }

    fn ensure_gh_auth(&self) -> Result<(), GitWorkspaceError> {
        if !self.gh.program.exists() && which(&self.gh.program.to_string_lossy()).is_none() {
            return Err(GitWorkspaceError::MissingTool {
                tool: "gh",
                message: "install GitHub CLI and run gh auth login".into(),
            });
        }
        let mut command = Command::new(&self.gh.program);
        command
            .args(["auth", "status"])
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = spawn_bounded(
            command,
            None,
            "checking GitHub authentication",
            GH_AUTH_TIMEOUT,
        )
        .map_err(|error| match error {
            GitWorkspaceError::CouldNotRunGit { source, .. } => GitWorkspaceError::MissingTool {
                tool: "gh",
                message: source.to_string(),
            },
            other => other,
        })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(GitWorkspaceError::Unauthenticated {
                tool: "gh",
                message: "run gh auth login to view pull requests".into(),
            })
        }
    }

    fn gh_run(
        &self,
        args: &[&str],
        timeout: Duration,
        operation: &'static str,
    ) -> Result<Vec<u8>, GitWorkspaceError> {
        let mut command = Command::new(&self.gh.program);
        command
            .args(args)
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = spawn_bounded(command, None, operation, timeout)?;
        ensure_success(output, operation).map(|output| output.stdout)
    }
}

struct PrExtra {
    head_oid: Option<String>,
    base_oid: Option<String>,
    is_cross_repository: bool,
    head_repository: Option<String>,
}

pub fn parse_pr_input(
    input: &str,
    default_repository: Option<&str>,
) -> Result<String, GitWorkspaceError> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_PR_INPUT_BYTES || trimmed.contains('\0') {
        return Err(GitWorkspaceError::InvalidInput(
            "enter a pull request number such as #123 or a GitHub pull-request URL".into(),
        ));
    }
    if let Some(url) = parse_pr_url(trimmed) {
        return Ok(url);
    }
    let number = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if number.chars().all(|ch| ch.is_ascii_digit())
        && !number.is_empty()
        && let Ok(number) = number.parse::<i64>()
        && number > 0
    {
        let repo = default_repository.ok_or_else(|| {
            GitWorkspaceError::InvalidInput(
                "this checkout has no GitHub origin; paste the full pull-request URL".into(),
            )
        })?;
        return Ok(format!("https://github.com/{repo}/pull/{number}"));
    }
    Err(GitWorkspaceError::InvalidInput(
        "enter #123, a number, or a GitHub pull-request URL".into(),
    ))
}

pub fn parse_pr_url(input: &str) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    let prefix = if lower.starts_with("https://github.com/") {
        "https://github.com/"
    } else if lower.starts_with("http://github.com/") {
        "http://github.com/"
    } else {
        return None;
    };
    let rest = input.get(prefix.len()..)?.trim_end_matches('/');
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?.trim_end_matches(".git");
    if parts.next() != Some("pull") {
        return None;
    }
    let number = parts.next()?;
    let digits: String = number
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    let number = digits.parse::<i64>().ok().filter(|number| *number > 0)?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("https://github.com/{owner}/{repo}/pull/{number}"))
}

pub fn github_repository(origin: &str) -> Option<String> {
    let origin = origin.trim();
    let rest = origin
        .strip_prefix("git@github.com:")
        .or_else(|| origin.strip_prefix("ssh://git@github.com/"))
        .or_else(|| origin.strip_prefix("ssh://github.com/"))
        .or_else(|| origin.strip_prefix("https://github.com/"))
        .or_else(|| origin.strip_prefix("http://github.com/"))
        .or_else(|| origin.strip_prefix("git://github.com/"))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let (owner, repo) = rest.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

fn sanitize_remote_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let Some((_, host)) = authority.rsplit_once('@') else {
        return url.to_owned();
    };
    if path.is_empty() {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}/{path}")
    }
}

pub fn owner_for_path(records: &[SessionRecord], path: &str) -> Option<GitWorkspaceOwner> {
    owner_for_location(records, path, None)
}

fn owner_for_location(
    records: &[SessionRecord],
    path: &str,
    host: Option<&str>,
) -> Option<GitWorkspaceOwner> {
    let local = host.is_none();
    let normalized = normalize_path(path, local);
    let mut best: Option<&SessionRecord> = None;
    for record in records {
        if record.host.as_deref() != host {
            continue;
        }
        let candidate = record
            .worktree_path
            .as_deref()
            .unwrap_or(record.cwd.as_str());
        let candidate = normalize_path(candidate, local);
        let owns_location = candidate == normalized
            || (record.worktree_path.is_none()
                && Path::new(&candidate).starts_with(Path::new(&normalized)));
        if !owns_location {
            continue;
        }
        match best {
            Some(existing) if session_is_live(existing) || !session_is_live(record) => {}
            _ => best = Some(record),
        }
    }
    best.map(|record| GitWorkspaceOwner {
        session_id: record.id.clone(),
        title: record.title.clone(),
        live: session_is_live(record),
    })
}

fn session_is_live(record: &SessionRecord) -> bool {
    !matches!(
        record.status,
        SessionStatus::Exited(_) | SessionStatus::Unknown
    )
}

fn normalize_path(path: &str, local: bool) -> String {
    if local && let Ok(path) = std::fs::canonicalize(path) {
        return path.to_string_lossy().into_owned();
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn sibling_worktree_path(root: &str, branch: &str) -> String {
    let slug = crate::git::branch_to_path_slug(branch);
    let root_path = Path::new(root);
    let parent = root_path.parent().unwrap_or(Path::new("."));
    let repo_name = root_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    parent
        .join(format!("{repo_name}-{slug}"))
        .to_string_lossy()
        .into_owned()
}

fn which(binary: &str) -> Option<PathBuf> {
    if binary.contains('/') {
        let path = PathBuf::from(binary);
        return path.exists().then_some(path);
    }
    let path = std::env::var("PATH").ok()?;
    path.split(':')
        .map(|dir| Path::new(dir).join(binary))
        .find(|candidate| candidate.is_file())
}

pub fn validate_ref(name: &str, kind: &str) -> Result<(), GitWorkspaceError> {
    if name.is_empty() || name.len() > MAX_REF_BYTES {
        return Err(GitWorkspaceError::InvalidInput(format!(
            "{kind} name is empty or too long"
        )));
    }
    if name.starts_with('-')
        || name.contains('\0')
        || name.contains([' ', '\n', '\r', '\\', '\t'])
        || name.contains("..")
        || name.contains("://")
        || name.contains('~')
        || name.contains('^')
        || name.contains(':')
        || name.contains('?')
        || name.contains('*')
        || name.contains('[')
        || name.contains('@')
    {
        return Err(GitWorkspaceError::InvalidInput(format!(
            "{kind} name {name:?} is not accepted"
        )));
    }
    Ok(())
}

fn validate_paths(paths: &[String]) -> Result<Vec<String>, GitWorkspaceError> {
    if paths.is_empty() {
        return Err(GitWorkspaceError::EmptySelection);
    }
    paths
        .iter()
        .map(|path| {
            let path_buf = PathBuf::from(path);
            if path.is_empty() {
                return Err(invalid_path(path, "path is empty"));
            }
            if path_buf.is_absolute() {
                return Err(invalid_path(path, "absolute paths are not accepted"));
            }
            let mut components = path_buf.components();
            let Some(first) = components.next() else {
                return Err(invalid_path(path, "path is empty"));
            };
            let first = match first {
                Component::Normal(first) => first,
                Component::ParentDir => {
                    return Err(invalid_path(path, "parent traversal is not accepted"));
                }
                Component::CurDir => {
                    return Err(invalid_path(path, "current-directory paths are too broad"));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(invalid_path(path, "absolute paths are not accepted"));
                }
            };
            if os_str_eq_ignore_ascii_case(first, OsStr::new(".git")) {
                return Err(invalid_path(path, "Git metadata cannot be mutated"));
            }
            for component in components {
                if !matches!(component, Component::Normal(_)) {
                    return Err(invalid_path(
                        path,
                        "only normalized repository-relative paths are accepted",
                    ));
                }
            }
            if path.contains('\0') {
                return Err(invalid_path(path, "path contains a NUL byte"));
            }
            Ok(path.clone())
        })
        .collect()
}

fn invalid_path(path: &str, reason: &'static str) -> GitWorkspaceError {
    GitWorkspaceError::InvalidPath {
        path: path.to_owned(),
        reason,
    }
}

fn os_str_eq_ignore_ascii_case(left: &OsStr, right: &OsStr) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

fn parse_status(root: &str, bytes: &[u8]) -> Result<GitReviewStatus, GitWorkspaceError> {
    let records: Vec<&[u8]> = bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect();
    let mut status = GitReviewStatus {
        repo_root: root.to_owned(),
        ..GitReviewStatus::default()
    };
    let mut index = 0;
    let mut entries = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if let Some(header) = record.strip_prefix(b"# ") {
            parse_branch_header(&mut status.branch, header)?;
            continue;
        }
        entries += 1;
        if entries > MAX_STATUS_ENTRIES {
            return Err(GitWorkspaceError::OutputTooLarge {
                operation: "reading status",
                limit: MAX_STATUS_ENTRIES,
            });
        }
        match record.first().copied() {
            Some(b'1') => {
                let fields = split_fields(record, 9);
                require_fields(&fields, 9, "ordinary entry")?;
                add_tracked_change(&mut status, fields[1], fields[8], None, false)?;
            }
            Some(b'2') => {
                let fields = split_fields(record, 10);
                require_fields(&fields, 10, "rename/copy entry")?;
                let original = records.get(index).copied().ok_or_else(|| {
                    GitWorkspaceError::MalformedStatus(
                        "rename/copy entry has no original path".to_owned(),
                    )
                })?;
                index += 1;
                add_tracked_change(
                    &mut status,
                    fields[1],
                    fields[9],
                    Some(String::from_utf8_lossy(original).into_owned()),
                    false,
                )?;
            }
            Some(b'u') => {
                let fields = split_fields(record, 11);
                require_fields(&fields, 11, "unmerged entry")?;
                add_tracked_change(&mut status, fields[1], fields[10], None, true)?;
            }
            Some(b'?') if record.get(1) == Some(&b' ') => {
                status.untracked.push(GitFileChange {
                    path: String::from_utf8_lossy(&record[2..]).into_owned(),
                    original_path: None,
                    kind: GitChangeKind::Added,
                });
            }
            Some(other) => {
                return Err(GitWorkspaceError::MalformedStatus(format!(
                    "unknown entry kind {:?}",
                    char::from(other)
                )));
            }
            None => {}
        }
    }
    Ok(status)
}

fn parse_branch_header(branch: &mut GitBranchInfo, header: &[u8]) -> Result<(), GitWorkspaceError> {
    if let Some(value) = header.strip_prefix(b"branch.oid ") {
        if value == b"(initial)" {
            branch.oid = None;
        } else {
            branch.oid = Some(String::from_utf8_lossy(value).into_owned());
        }
    } else if let Some(value) = header.strip_prefix(b"branch.head ") {
        if value == b"(detached)" {
            branch.name = None;
        } else {
            branch.name = Some(String::from_utf8_lossy(value).into_owned());
        }
    } else if let Some(value) = header.strip_prefix(b"branch.upstream ") {
        branch.upstream = Some(String::from_utf8_lossy(value).into_owned());
    } else if let Some(value) = header.strip_prefix(b"branch.ab ") {
        let fields = split_fields(value, 2);
        require_fields(&fields, 2, "branch ahead/behind header")?;
        branch.ahead = parse_prefixed_count(fields[0], b'+')?;
        branch.behind = parse_prefixed_count(fields[1], b'-')?;
    }
    Ok(())
}

fn parse_prefixed_count(value: &[u8], prefix: u8) -> Result<u64, GitWorkspaceError> {
    let Some(number) = value.strip_prefix(&[prefix]) else {
        return Err(GitWorkspaceError::MalformedStatus(format!(
            "branch count {:?} has the wrong prefix",
            String::from_utf8_lossy(value)
        )));
    };
    String::from_utf8_lossy(number).parse().map_err(|_| {
        GitWorkspaceError::MalformedStatus(format!(
            "branch count {:?} is not a number",
            String::from_utf8_lossy(value)
        ))
    })
}

fn add_tracked_change(
    status: &mut GitReviewStatus,
    xy: &[u8],
    path: &[u8],
    original_path: Option<String>,
    explicitly_unmerged: bool,
) -> Result<(), GitWorkspaceError> {
    if xy.len() != 2 {
        return Err(GitWorkspaceError::MalformedStatus(format!(
            "XY status {:?} is not two bytes",
            String::from_utf8_lossy(xy)
        )));
    }
    let index_kind = xy[0];
    let worktree_kind = xy[1];
    let path = String::from_utf8_lossy(path).into_owned();
    if explicitly_unmerged || is_unmerged(index_kind, worktree_kind) {
        status.conflicted.push(GitFileChange {
            path,
            original_path,
            kind: GitChangeKind::Unmerged,
        });
        return Ok(());
    }
    if index_kind != b'.' {
        status.staged.push(GitFileChange {
            path: path.clone(),
            original_path: original_path.clone(),
            kind: change_kind(index_kind),
        });
    }
    if worktree_kind != b'.' {
        status.unstaged.push(GitFileChange {
            path,
            original_path,
            kind: change_kind(worktree_kind),
        });
    }
    Ok(())
}

fn is_unmerged(index: u8, worktree: u8) -> bool {
    index == b'U' || worktree == b'U' || matches!((index, worktree), (b'D', b'D') | (b'A', b'A'))
}

fn change_kind(value: u8) -> GitChangeKind {
    match value {
        b'A' => GitChangeKind::Added,
        b'M' => GitChangeKind::Modified,
        b'D' => GitChangeKind::Deleted,
        b'R' => GitChangeKind::Renamed,
        b'C' => GitChangeKind::Copied,
        b'T' => GitChangeKind::TypeChanged,
        b'U' => GitChangeKind::Unmerged,
        _ => GitChangeKind::Unknown,
    }
}

fn split_fields(bytes: &[u8], count: usize) -> Vec<&[u8]> {
    bytes.splitn(count, |byte| *byte == b' ').collect()
}

fn require_fields(
    fields: &[&[u8]],
    expected: usize,
    description: &str,
) -> Result<(), GitWorkspaceError> {
    if fields.len() == expected {
        Ok(())
    } else {
        Err(GitWorkspaceError::MalformedStatus(format!(
            "{description} has {} fields, expected {expected}",
            fields.len()
        )))
    }
}

fn trim_line_ending(mut bytes: &[u8]) -> &[u8] {
    if bytes.ends_with(b"\n") {
        bytes = &bytes[..bytes.len() - 1];
    }
    if bytes.ends_with(b"\r") {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn nonempty_lossy(bytes: &[u8]) -> Option<String> {
    let value = String::from_utf8_lossy(bytes);
    (!value.is_empty()).then(|| value.into_owned())
}

fn parse_track(bytes: &[u8]) -> (u64, u64) {
    let text = String::from_utf8_lossy(bytes);
    let mut ahead = 0;
    let mut behind = 0;
    if let Some(rest) = text.split("ahead ").nth(1) {
        ahead = rest
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
    }
    if let Some(rest) = text.split("behind ").nth(1) {
        behind = rest
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
    }
    (ahead, behind)
}

fn patch_creates_file(patch: &[u8]) -> bool {
    patch.split(|byte| *byte == b'\n').any(|line| {
        line.strip_suffix(b"\r").unwrap_or(line) == b"--- /dev/null"
            || line.starts_with(b"new file mode ")
    })
}

fn patch_rejected(output: GitOutput, mutation: GitPatchMutation) -> GitWorkspaceError {
    let message = output.stderr_message();
    GitWorkspaceError::PatchDoesNotApply {
        mutation,
        message: if message.is_empty() {
            "Git rejected the selected hunk; refresh the review and try again".to_owned()
        } else {
            message
        },
    }
}

pub fn encode_remote_git_input(cwd: &str, args: &[&str], input: Option<&[u8]>) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(cwd.as_bytes());
    payload.push(b'\n');
    payload.extend_from_slice(args.len().to_string().as_bytes());
    payload.push(b'\n');
    for arg in args {
        payload.extend_from_slice(arg.as_bytes());
        payload.push(b'\n');
    }
    if let Some(input) = input {
        payload.extend_from_slice(input);
    }
    payload
}

pub fn remote_git_run_script() -> &'static str {
    REMOTE_GIT_RUN_SCRIPT
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeus_proto::{DateMillis, ProjectId, Resumability, SessionId, TitleSource};

    #[test]
    fn pull_request_input_accepts_hash_number_and_url() {
        assert_eq!(
            parse_pr_input("#12", Some("nnayz/zeus")).unwrap(),
            "https://github.com/nnayz/zeus/pull/12"
        );
        assert_eq!(
            parse_pr_input("12", Some("nnayz/zeus")).unwrap(),
            "https://github.com/nnayz/zeus/pull/12"
        );
        assert_eq!(
            parse_pr_input("https://github.com/nnayz/zeus/pull/12/files", None).unwrap(),
            "https://github.com/nnayz/zeus/pull/12"
        );
        assert_eq!(
            parse_pr_input("HTTPS://GITHUB.COM/nnayz/zeus/pull/12", None).unwrap(),
            "https://github.com/nnayz/zeus/pull/12"
        );
        assert!(parse_pr_input("not-a-pr", Some("nnayz/zeus")).is_err());
        assert!(parse_pr_input("0", Some("nnayz/zeus")).is_err());
        assert!(parse_pr_input("12", None).is_err());
        assert!(parse_pr_input("https://evil.example/pull/1", None).is_err());
    }

    #[test]
    fn origin_urls_become_owner_repo() {
        assert_eq!(
            github_repository("git@github.com:nnayz/zeus.git").as_deref(),
            Some("nnayz/zeus")
        );
        assert_eq!(
            github_repository("https://github.com/nnayz/zeus").as_deref(),
            Some("nnayz/zeus")
        );
        let sanitized = sanitize_remote_url("https://secret@github.com/nnayz/zeus.git");
        assert_eq!(sanitized, "https://github.com/nnayz/zeus.git");
        assert_eq!(github_repository(&sanitized).as_deref(), Some("nnayz/zeus"));
        assert_eq!(github_repository("https://gitlab.com/nnayz/zeus"), None);
    }

    #[test]
    fn ref_names_are_treated_as_data() {
        assert!(validate_ref("feature/ok", "branch").is_ok());
        assert!(validate_ref("-n", "branch").is_err());
        assert!(validate_ref("foo;rm", "branch").is_ok());
        assert!(validate_ref("foo bar", "branch").is_err());
        assert!(validate_ref("foo..bar", "branch").is_err());
        assert!(validate_ref("origin/main", "branch").is_ok());
    }

    #[test]
    fn live_owner_wins_over_exited_session_on_the_same_path() {
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
        let owner = owner_for_path(&[dead, live], "/repo").expect("owner");
        assert_eq!(owner.session_id.0, "s_live");
        assert!(owner.live);
    }

    #[test]
    fn porcelain_status_parses_tracking_and_renames() {
        let bytes = b"# branch.oid abcdef\0# branch.head feature\0# branch.upstream origin/feature\0# branch.ab +3 -2\x002 R. N... 100644 100644 100644 aaaaaaa bbbbbbb R100 new name.txt\0old name.txt\0";
        let status = parse_status("/repo", bytes).unwrap();
        assert_eq!(status.branch.name.as_deref(), Some("feature"));
        assert_eq!(status.branch.upstream.as_deref(), Some("origin/feature"));
        assert_eq!(status.branch.ahead, 3);
        assert_eq!(status.branch.behind, 2);
        assert_eq!(status.staged[0].kind, GitChangeKind::Renamed);
        assert_eq!(status.staged[0].path, "new name.txt");
        assert_eq!(
            status.staged[0].original_path.as_deref(),
            Some("old name.txt")
        );
    }

    #[test]
    fn detached_and_unborn_headers_are_preserved() {
        let detached =
            parse_status("/repo", b"# branch.oid abcdef\0# branch.head (detached)\0").unwrap();
        assert!(detached.branch.name.is_none());
        assert_eq!(detached.branch.oid.as_deref(), Some("abcdef"));
        let unborn =
            parse_status("/repo", b"# branch.oid (initial)\0# branch.head main\0").unwrap();
        assert_eq!(unborn.branch.name.as_deref(), Some("main"));
        assert!(unborn.branch.oid.is_none());
    }

    #[test]
    fn remote_git_payload_keeps_argv_as_data_lines() {
        let payload = encode_remote_git_input("/srv/app", &["status", "--porcelain=v2"], None);
        let text = String::from_utf8(payload).unwrap();
        assert_eq!(text, "/srv/app\n2\nstatus\n--porcelain=v2\n");
    }

    #[test]
    fn mutation_locks_serialize_the_same_location() {
        let locks = MutationLocks::default();
        let first = locks.acquire("local\0/repo").unwrap();
        let second_locks = locks.clone();
        let (sent, received) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let _second = second_locks.acquire("local\0/repo").unwrap();
            sent.send(()).unwrap();
        });
        assert!(received.recv_timeout(Duration::from_millis(50)).is_err());
        drop(first);
        received.recv_timeout(Duration::from_secs(1)).unwrap();
        waiter.join().unwrap();
    }

    #[test]
    fn unsafe_paths_are_rejected() {
        for path in ["../outside", "/absolute", ".", ".git/config", "a/../b"] {
            assert!(matches!(
                validate_paths(&[path.to_owned()]),
                Err(GitWorkspaceError::InvalidPath { .. })
            ));
        }
        assert!(matches!(
            validate_paths(&[]),
            Err(GitWorkspaceError::EmptySelection)
        ));
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
}
