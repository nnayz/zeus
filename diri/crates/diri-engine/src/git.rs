//! Git facts a session needs: which branch it is on, and whether it is in a
//! linked worktree.
//!
//! Branch reading parses `.git/HEAD` directly rather than shelling out. It is
//! polled for a live label in the sidebar, and a `git` subprocess per session
//! per second is a cost worth not paying. Worktree operations do shell out —
//! they are rare, and reimplementing `git worktree` would be reckless.
//!
//! Ported from the Swift `DirijorGit`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One entry from `git worktree list --porcelain`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: Option<String>,
    pub is_bare: bool,
    pub is_detached: bool,
    pub is_prunable: bool,
}

/// The current branch for a working directory: a branch name, a short SHA when
/// HEAD is detached, or `None` outside a repository.
pub fn branch(cwd: &Path) -> Option<String> {
    let git_dir = git_dir(cwd)?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let trimmed = head.trim();

    if let Some(reference) = trimmed.strip_prefix("ref: ") {
        return match reference.split_once("refs/heads/") {
            Some((_, name)) => Some(name.to_string()),
            None => reference.rsplit('/').next().map(str::to_string),
        };
    }
    // Detached HEAD: a raw object id.
    Some(trimmed.chars().take(8).collect())
}

/// True when `cwd` is inside a *linked* worktree rather than the main checkout.
///
/// The signal is what `.git` is: a directory in the main checkout, a file
/// carrying `gitdir:` indirection in a linked one. This is what distinguishes
/// an agent's own worktree from the primary tree.
pub fn is_linked_worktree(cwd: &Path) -> bool {
    let mut dir = cwd.to_path_buf();
    loop {
        let dot_git = dir.join(".git");
        if let Ok(metadata) = std::fs::metadata(&dot_git) {
            return !metadata.is_dir();
        }
        if !dir.pop() {
            return false;
        }
    }
}

/// Resolves the directory holding `HEAD`, following worktree indirection.
fn git_dir(cwd: &Path) -> Option<PathBuf> {
    let mut dir = cwd.to_path_buf();
    loop {
        let dot_git = dir.join(".git");
        if let Ok(metadata) = std::fs::metadata(&dot_git) {
            if metadata.is_dir() {
                return Some(dot_git);
            }
            // `.git` is a file: "gitdir: <path>".
            let contents = std::fs::read_to_string(&dot_git).ok()?;
            let line = contents.lines().next()?;
            let target = line.strip_prefix("gitdir: ")?.trim();
            let resolved = if Path::new(target).is_absolute() {
                PathBuf::from(target)
            } else {
                dir.join(target)
            };
            return Some(resolved);
        }
        if !dir.pop() {
            return None;
        }
    }
}

pub fn is_repository(path: &Path) -> bool {
    git_dir(path).is_some()
}

/// The repository root for `path`.
pub fn repository_root(path: &Path) -> Option<String> {
    let output = run(&["rev-parse", "--show-toplevel"], path).ok()?;
    let trimmed = output.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub fn list_worktrees(repo: &Path) -> std::io::Result<Vec<WorktreeInfo>> {
    let porcelain = run(&["worktree", "list", "--porcelain"], repo)?;
    Ok(parse_porcelain(&porcelain))
}

/// Parses `git worktree list --porcelain`.
///
/// Blocks are separated by blank lines, but a `worktree` line also starts a new
/// block — trailing output without a final blank line still has to flush.
pub fn parse_porcelain(porcelain: &str) -> Vec<WorktreeInfo> {
    /// One block being accumulated. Replaced wholesale on flush, so flags
    /// cannot leak from one worktree into the next.
    #[derive(Default)]
    struct Block {
        path: Option<String>,
        branch: Option<String>,
        is_bare: bool,
        is_detached: bool,
        is_prunable: bool,
    }

    fn flush(block: &mut Block, results: &mut Vec<WorktreeInfo>) {
        let Some(path) = block.path.take() else {
            return;
        };
        let finished = std::mem::take(block);
        results.push(WorktreeInfo {
            path,
            branch: finished.branch,
            is_bare: finished.is_bare,
            is_detached: finished.is_detached,
            is_prunable: finished.is_prunable,
        });
    }

    let mut results = Vec::new();
    let mut block = Block::default();

    for line in porcelain.split('\n') {
        if line.is_empty() {
            flush(&mut block, &mut results);
        } else if let Some(rest) = line.strip_prefix("worktree ") {
            flush(&mut block, &mut results);
            block.path = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            block.branch = Some(rest.strip_prefix("refs/heads/").unwrap_or(rest).to_string());
        } else if line == "bare" {
            block.is_bare = true;
        } else if line == "detached" {
            block.is_detached = true;
        } else if line == "prunable" || line.starts_with("prunable ") {
            block.is_prunable = true;
        }
        // `HEAD <sha>` and other keys carry nothing this type models.
    }
    flush(&mut block, &mut results);
    results
}

/// Runs a git command, bounded and without inheriting a terminal.
///
/// The hardening is the same as the test helpers': a git that can prompt is a
/// git that can hang forever, and ambient config from the host has no business
/// affecting what the daemon sees.
fn run(args: &[&str], cwd: &Path) -> std::io::Result<String> {
    let output = Command::new("/usr/bin/git")
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_parsing_handles_a_main_checkout_and_a_linked_worktree() {
        let porcelain = "worktree /repo\nHEAD abc123\nbranch refs/heads/main\n\
             \n\
             worktree /repo/.claude/worktrees/bright-fox\nHEAD def456\n\
             branch refs/heads/worktree-bright-fox\n\n";
        let worktrees = parse_porcelain(porcelain);

        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].path, "/repo");
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
        assert_eq!(
            worktrees[1].branch.as_deref(),
            Some("worktree-bright-fox"),
            "the refs/heads/ prefix is stripped"
        );
    }

    #[test]
    fn a_final_block_without_a_trailing_blank_line_is_not_dropped() {
        let worktrees = parse_porcelain("worktree /repo\nbranch refs/heads/main");
        assert_eq!(worktrees.len(), 1, "the last block must still flush");
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn bare_detached_and_prunable_are_recognized() {
        let porcelain = "worktree /repo\nbare\n\n\
             worktree /repo/wt\nHEAD abc\ndetached\nprunable gitdir file points to non-existent\n";
        let worktrees = parse_porcelain(porcelain);

        assert!(worktrees[0].is_bare);
        assert!(worktrees[1].is_detached);
        assert!(
            worktrees[1].is_prunable,
            "prunable carries a reason after it"
        );
        assert!(
            !worktrees[0].is_detached,
            "flags do not leak between blocks"
        );
    }

    #[test]
    fn head_parsing_reads_a_branch_a_detached_sha_and_nothing() {
        let temp = tempfile::tempdir().expect("temp");
        let repo = temp.path().join("repo");
        let git = repo.join(".git");
        std::fs::create_dir_all(&git).expect("mkdir");

        std::fs::write(git.join("HEAD"), "ref: refs/heads/feature/login\n").expect("write");
        assert_eq!(branch(&repo).as_deref(), Some("feature/login"));

        std::fs::write(git.join("HEAD"), "9f8e7d6c5b4a3210\n").expect("write");
        assert_eq!(
            branch(&repo).as_deref(),
            Some("9f8e7d6c"),
            "a detached HEAD shows a short sha"
        );

        assert_eq!(branch(temp.path()).as_deref(), None, "not a repository");
    }

    #[test]
    fn a_branch_is_found_from_a_subdirectory() {
        let temp = tempfile::tempdir().expect("temp");
        let repo = temp.path().join("repo");
        let nested = repo.join("src/deep/inner");
        std::fs::create_dir_all(&nested).expect("mkdir");
        std::fs::create_dir_all(repo.join(".git")).expect("mkdir");
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").expect("write");

        assert_eq!(branch(&nested).as_deref(), Some("main"));
    }

    #[test]
    fn a_linked_worktree_is_told_apart_from_the_main_checkout() {
        let temp = tempfile::tempdir().expect("temp");
        let main = temp.path().join("repo");
        std::fs::create_dir_all(main.join(".git")).expect("mkdir");
        assert!(!is_linked_worktree(&main), ".git is a directory here");

        let linked = temp.path().join("wt");
        std::fs::create_dir_all(&linked).expect("mkdir");
        std::fs::write(
            linked.join(".git"),
            format!("gitdir: {}/.git/worktrees/wt\n", main.display()),
        )
        .expect("write");
        assert!(is_linked_worktree(&linked), ".git is a file here");
    }

    #[test]
    fn a_worktrees_head_is_followed_through_the_indirection() {
        let temp = tempfile::tempdir().expect("temp");
        let main = temp.path().join("repo");
        let worktree_git = main.join(".git/worktrees/wt");
        std::fs::create_dir_all(&worktree_git).expect("mkdir");
        std::fs::write(worktree_git.join("HEAD"), "ref: refs/heads/side-branch\n").expect("write");

        let linked = temp.path().join("wt");
        std::fs::create_dir_all(&linked).expect("mkdir");
        std::fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", worktree_git.display()),
        )
        .expect("write");

        assert_eq!(
            branch(&linked).as_deref(),
            Some("side-branch"),
            "a linked worktree has its own HEAD"
        );
    }
}
