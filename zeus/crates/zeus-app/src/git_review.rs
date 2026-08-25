//! Review cockpit view models.
//!
//! Mutations and repository navigation run in the Engine. This module keeps
//! the PathBuf-oriented snapshot the inspector already renders, plus the
//! sidebar preview fixture.

use std::path::PathBuf;

use zeus_proto::{
    GitBranchInfo, GitChangeKind, GitFileChange, GitPatchMutation, GitReviewStatus,
    GitWorkspaceResult,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewStatus {
    pub repo_root: PathBuf,
    pub branch: BranchInfo,
    pub staged: Vec<FileChange>,
    pub unstaged: Vec<FileChange>,
    pub untracked: Vec<FileChange>,
    pub conflicted: Vec<FileChange>,
}

/// Deterministic Review cockpit for sidebar preview fixtures.
pub fn preview_review_status() -> ReviewStatus {
    ReviewStatus {
        repo_root: PathBuf::from("/Users/preview/Projects/zeus"),
        branch: BranchInfo {
            name: Some("sidebar-craft".into()),
            oid: Some("9b81d04c".into()),
            upstream: Some("origin/sidebar-craft".into()),
            ahead: 2,
            behind: 0,
        },
        staged: vec![file_change(
            "crates/zeus-app/src/sidebar/view.rs",
            ChangeKind::Modified,
        )],
        unstaged: vec![file_change(
            "crates/zeus-app/src/sidebar/state.rs",
            ChangeKind::Modified,
        )],
        untracked: vec![file_change(
            "crates/zeus-app/src/sidebar/tests.rs",
            ChangeKind::Added,
        )],
        conflicted: Vec::new(),
    }
}

pub fn preview_workspace() -> GitWorkspaceResult {
    let status = preview_review_status();
    GitWorkspaceResult {
        session_id: zeus_proto::SessionId::new("s_preview"),
        repo_root: status.repo_root.to_string_lossy().into_owned(),
        worktree_path: "/Users/preview/Projects/zeus".into(),
        linked_worktree: false,
        origin_url: Some("https://github.com/nnayz/zeus.git".into()),
        repository: Some("nnayz/zeus".into()),
        branch: GitBranchInfo {
            name: status.branch.name.clone(),
            oid: status.branch.oid.clone(),
            upstream: status.branch.upstream.clone(),
            ahead: status.branch.ahead,
            behind: status.branch.behind,
        },
        dirty: true,
        conflicted: false,
        unborn: false,
        detached: false,
        owner: Some(zeus_proto::GitWorkspaceOwner {
            session_id: zeus_proto::SessionId::new("s_preview"),
            title: "Codex".into(),
            live: true,
        }),
        target: zeus_proto::GitReviewTarget::WorkingTree,
        status: GitReviewStatus {
            repo_root: "/Users/preview/Projects/zeus".into(),
            branch: GitBranchInfo {
                name: status.branch.name.clone(),
                oid: status.branch.oid.clone(),
                upstream: status.branch.upstream.clone(),
                ahead: status.branch.ahead,
                behind: status.branch.behind,
            },
            staged: vec![GitFileChange {
                path: "crates/zeus-app/src/sidebar/view.rs".into(),
                original_path: None,
                kind: GitChangeKind::Modified,
            }],
            unstaged: vec![GitFileChange {
                path: "crates/zeus-app/src/sidebar/state.rs".into(),
                original_path: None,
                kind: GitChangeKind::Modified,
            }],
            untracked: vec![GitFileChange {
                path: "crates/zeus-app/src/sidebar/tests.rs".into(),
                original_path: None,
                kind: GitChangeKind::Added,
            }],
            conflicted: Vec::new(),
        },
        pull_request: None,
    }
}

fn file_change(path: &str, kind: ChangeKind) -> FileChange {
    FileChange {
        path: PathBuf::from(path),
        original_path: None,
        kind,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchInfo {
    /// `None` means detached HEAD. An unborn branch still has a name.
    pub name: Option<String>,
    /// The full HEAD object id, or `None` for an unborn branch.
    pub oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: PathBuf,
    pub original_path: Option<PathBuf>,
    pub kind: ChangeKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Unknown(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchMutation {
    Stage,
    Unstage,
    Discard,
}

impl PatchMutation {
    pub const fn into_proto(self) -> GitPatchMutation {
        match self {
            Self::Stage => GitPatchMutation::Stage,
            Self::Unstage => GitPatchMutation::Unstage,
            Self::Discard => GitPatchMutation::Discard,
        }
    }
}

impl ReviewStatus {
    pub fn from_proto(status: &GitReviewStatus) -> Self {
        Self {
            repo_root: PathBuf::from(&status.repo_root),
            branch: BranchInfo {
                name: status.branch.name.clone(),
                oid: status.branch.oid.clone(),
                upstream: status.branch.upstream.clone(),
                ahead: status.branch.ahead,
                behind: status.branch.behind,
            },
            staged: status.staged.iter().map(file_from_proto).collect(),
            unstaged: status.unstaged.iter().map(file_from_proto).collect(),
            untracked: status.untracked.iter().map(file_from_proto).collect(),
            conflicted: status.conflicted.iter().map(file_from_proto).collect(),
        }
    }
}

fn file_from_proto(change: &GitFileChange) -> FileChange {
    FileChange {
        path: PathBuf::from(&change.path),
        original_path: change.original_path.as_ref().map(PathBuf::from),
        kind: match change.kind {
            GitChangeKind::Added => ChangeKind::Added,
            GitChangeKind::Modified => ChangeKind::Modified,
            GitChangeKind::Deleted => ChangeKind::Deleted,
            GitChangeKind::Renamed => ChangeKind::Renamed,
            GitChangeKind::Copied => ChangeKind::Copied,
            GitChangeKind::TypeChanged => ChangeKind::TypeChanged,
            GitChangeKind::Unmerged => ChangeKind::Unmerged,
            GitChangeKind::Unknown => ChangeKind::Unknown('?'),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_review_status_has_staged_and_working_files() {
        let status = preview_review_status();
        assert_eq!(status.branch.name.as_deref(), Some("sidebar-craft"));
        assert_eq!(status.staged.len(), 1);
        assert_eq!(status.unstaged.len(), 1);
        assert_eq!(status.untracked.len(), 1);
        assert_eq!(status.branch.ahead, 2);
    }

    #[test]
    fn preview_workspace_identifies_the_repository() {
        let workspace = preview_workspace();
        assert_eq!(workspace.repository.as_deref(), Some("nnayz/zeus"));
        assert!(workspace.dirty);
        assert_eq!(workspace.status.staged.len(), 1);
    }
}
