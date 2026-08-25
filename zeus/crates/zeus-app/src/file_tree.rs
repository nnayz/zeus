//! Worktree-scoped data model for the Code inspector's contextual file tree.
//!
//! The UI crosses two blocking seams: discover one workspace, then read one
//! directory (or the ancestor chain required by an explicit reveal). Changed
//! rows are projections of Review's porcelain-v2 [`ReviewStatus`], so Code and
//! Review cannot disagree about staged, working, rename, conflict, or
//! untracked semantics. Traversal is lazy, Git-ignore-aware, symlink-safe, and
//! bounded before it reaches GPUI.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::git_review::{ChangeKind, GitRepository, GitReviewError, ReviewStatus};

pub const MAX_DIRECTORY_ENTRIES: usize = 2_000;
pub const MAX_DIRECTORY_SCAN: usize = 10_000;
pub const MAX_VISIBLE_ROWS: usize = 20_000;
pub const MAX_REVEAL_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileTreeMode {
    #[default]
    Changed,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TreeEntryKind {
    Directory,
    File,
}

#[derive(Clone, Debug)]
pub struct FileTreeWorkspace {
    root: PathBuf,
    repository: Option<GitRepository>,
    repository_error: Option<String>,
}

impl FileTreeWorkspace {
    pub fn for_session(cwd: impl AsRef<Path>) -> Result<Self, FileTreeError> {
        let requested = cwd.as_ref();
        let canonical =
            fs::canonicalize(requested).map_err(|error| FileTreeError::Unavailable {
                path: requested.to_path_buf(),
                message: error.to_string(),
            })?;
        let session_cwd = if canonical.is_file() {
            canonical
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| FileTreeError::Unavailable {
                    path: canonical.clone(),
                    message: "the session path has no parent directory".to_owned(),
                })?
        } else if canonical.is_dir() {
            canonical
        } else {
            return Err(FileTreeError::Unavailable {
                path: canonical,
                message: "the session path is not a directory".to_owned(),
            });
        };

        match GitRepository::discover(&session_cwd) {
            Ok(repository) => Ok(Self {
                root: repository.root().to_path_buf(),
                repository: Some(repository),
                repository_error: None,
            }),
            Err(GitReviewError::NotRepository(_)) => Ok(Self {
                root: session_cwd,
                repository: None,
                repository_error: None,
            }),
            Err(error) => Ok(Self {
                root: session_cwd,
                repository: None,
                repository_error: Some(error.to_string()),
            }),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn changed_unavailable_message(&self) -> Option<String> {
        self.repository_error.clone().or_else(|| {
            self.repository.is_none().then(|| {
                "This session is not inside a Git repository. All files is still available."
                    .to_owned()
            })
        })
    }

    pub fn load_directory(&self, directory: &Path) -> Result<DirectoryListing, FileTreeError> {
        self.load_directory_with_limits(directory, MAX_DIRECTORY_ENTRIES, MAX_DIRECTORY_SCAN)
    }

    fn load_directory_with_limits(
        &self,
        directory: &Path,
        entry_limit: usize,
        scan_limit: usize,
    ) -> Result<DirectoryListing, FileTreeError> {
        validate_relative(directory, true)?;
        let absolute = self.root.join(directory);
        let metadata = fs::symlink_metadata(&absolute)
            .map_err(|error| directory_error(directory, "inspect", error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(FileTreeError::NotDirectory(directory.to_path_buf()));
        }
        let canonical = fs::canonicalize(&absolute)
            .map_err(|error| directory_error(directory, "resolve", error))?;
        if !canonical.starts_with(&self.root) {
            return Err(FileTreeError::OutsideWorkspace(canonical));
        }

        let reader =
            fs::read_dir(&absolute).map_err(|error| directory_error(directory, "read", error))?;
        let mut candidates = Vec::new();
        let mut truncated = false;
        for (scanned, entry) in reader.enumerate() {
            if scanned >= scan_limit {
                truncated = true;
                break;
            }
            let entry = entry.map_err(|error| directory_error(directory, "read", error))?;
            let name = entry.file_name();
            if name == OsStr::new(".git") {
                continue;
            }
            let file_type = entry
                .file_type()
                .map_err(|error| directory_error(directory, "inspect an entry in", error))?;
            if file_type.is_symlink() {
                continue;
            }
            let kind = if file_type.is_dir() {
                TreeEntryKind::Directory
            } else if file_type.is_file() {
                TreeEntryKind::File
            } else {
                continue;
            };
            let relative_path = directory.join(&name);
            candidates.push(DirectoryEntry {
                relative_path,
                name: name.to_string_lossy().into_owned(),
                kind,
            });
        }

        if let Some(repository) = &self.repository {
            let paths: Vec<_> = candidates
                .iter()
                .map(|entry| entry.relative_path.clone())
                .collect();
            let ignored = repository.ignored_paths(&paths)?;
            candidates.retain(|entry| !ignored.contains(&entry.relative_path));
        }
        candidates.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.relative_path.cmp(&right.relative_path))
        });
        if candidates.len() > entry_limit {
            candidates.truncate(entry_limit);
            truncated = true;
        }
        Ok(DirectoryListing {
            directory: directory.to_path_buf(),
            entries: candidates,
            truncated,
        })
    }

    /// Loads only the directories needed to expose `relative_path`.
    pub fn reveal(&self, relative_path: &Path) -> Result<Vec<DirectoryListing>, FileTreeError> {
        validate_relative(relative_path, false)?;
        let parent = relative_path.parent().unwrap_or_else(|| Path::new(""));
        let mut directories = vec![PathBuf::new()];
        let mut current = PathBuf::new();
        for component in parent.components() {
            let Component::Normal(component) = component else {
                return Err(FileTreeError::UnsafePath(relative_path.to_path_buf()));
            };
            current.push(component);
            directories.push(current.clone());
            if directories.len() > MAX_REVEAL_DEPTH {
                return Err(FileTreeError::RevealTooDeep {
                    path: relative_path.to_path_buf(),
                    limit: MAX_REVEAL_DEPTH,
                });
            }
        }

        let mut listings = Vec::with_capacity(directories.len());
        for (index, directory) in directories.iter().enumerate() {
            let listing = self.load_directory(directory)?;
            let wanted = directories
                .get(index + 1)
                .map_or(relative_path, PathBuf::as_path);
            if !listing
                .entries
                .iter()
                .any(|entry| entry.relative_path == wanted)
            {
                return Err(FileTreeError::RevealUnavailable {
                    path: relative_path.to_path_buf(),
                    directory: directory.clone(),
                    truncated: listing.truncated,
                });
            }
            listings.push(listing);
        }
        Ok(listings)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub relative_path: PathBuf,
    pub name: String,
    pub kind: TreeEntryKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryListing {
    pub directory: PathBuf,
    pub entries: Vec<DirectoryEntry>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileStatus {
    pub staged: Option<ChangeKind>,
    pub working: Option<ChangeKind>,
    pub untracked: bool,
    pub conflicted: bool,
    pub original_path: Option<PathBuf>,
}

impl FileStatus {
    /// Two-column Git decoration: index first, working tree second.
    pub fn decoration(&self) -> String {
        if self.conflicted {
            return "UU".to_owned();
        }
        if self.untracked {
            return "??".to_owned();
        }
        format!(
            "{}{}",
            self.staged.map_or('·', change_code),
            self.working.map_or('·', change_code)
        )
    }

    pub fn primary_kind(&self) -> ChangeKind {
        if self.conflicted {
            ChangeKind::Unmerged
        } else if self.untracked {
            ChangeKind::Added
        } else {
            self.working
                .or(self.staged)
                .unwrap_or(ChangeKind::Unknown('?'))
        }
    }
}

fn change_code(kind: ChangeKind) -> char {
    match kind {
        ChangeKind::Added => 'A',
        ChangeKind::Modified => 'M',
        ChangeKind::Deleted => 'D',
        ChangeKind::Renamed => 'R',
        ChangeKind::Copied => 'C',
        ChangeKind::TypeChanged => 'T',
        ChangeKind::Unmerged => 'U',
        ChangeKind::Unknown(value) => value,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub relative_path: PathBuf,
    pub status: FileStatus,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangedSnapshot {
    pub repo_root: PathBuf,
    pub files: Vec<ChangedFile>,
    pub truncated: bool,
    rows: Arc<Vec<VisibleTreeRow>>,
}

impl From<&ReviewStatus> for ChangedSnapshot {
    fn from(status: &ReviewStatus) -> Self {
        let mut files: BTreeMap<PathBuf, FileStatus> = BTreeMap::new();
        for change in &status.staged {
            let entry = files.entry(change.path.clone()).or_default();
            entry.staged = Some(change.kind);
            entry.original_path = change.original_path.clone();
        }
        for change in &status.unstaged {
            let entry = files.entry(change.path.clone()).or_default();
            entry.working = Some(change.kind);
            if change.original_path.is_some() {
                entry.original_path = change.original_path.clone();
            }
        }
        for change in &status.untracked {
            let entry = files.entry(change.path.clone()).or_default();
            entry.untracked = true;
        }
        for change in &status.conflicted {
            let entry = files.entry(change.path.clone()).or_default();
            entry.conflicted = true;
            entry.original_path = change.original_path.clone();
        }
        let truncated = files.len() > MAX_VISIBLE_ROWS;
        let files: Vec<ChangedFile> = files
            .into_iter()
            .take(MAX_VISIBLE_ROWS)
            .map(|(relative_path, status)| ChangedFile {
                relative_path,
                status,
            })
            .collect();
        let rows = Arc::new(
            files
                .iter()
                .map(|file| VisibleTreeRow {
                    relative_path: file.relative_path.clone(),
                    label: file.relative_path.to_string_lossy().into_owned(),
                    depth: 0,
                    kind: TreeEntryKind::File,
                    status: Some(file.status.clone()),
                })
                .collect(),
        );
        Self {
            repo_root: status.repo_root.clone(),
            files,
            truncated,
            rows,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleTreeRow {
    pub relative_path: PathBuf,
    pub label: String,
    pub depth: usize,
    pub kind: TreeEntryKind,
    pub status: Option<FileStatus>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileTreeIntent {
    None,
    OpenFile(PathBuf),
    LoadDirectory(PathBuf),
}

#[derive(Clone, Debug, Default)]
pub struct FileTreeModel {
    mode: FileTreeMode,
    filter: String,
    changed: Vec<ChangedFile>,
    changed_rows: Arc<Vec<VisibleTreeRow>>,
    listings: BTreeMap<PathBuf, DirectoryListing>,
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
}

impl FileTreeModel {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub const fn mode(&self) -> FileTreeMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: FileTreeMode) {
        self.mode = mode;
        self.ensure_selection();
    }

    pub fn set_filter(&mut self, filter: impl Into<String>) {
        self.filter = filter.into();
        self.ensure_selection();
    }

    pub fn clear_filter(&mut self) {
        self.filter.clear();
        self.ensure_selection();
    }

    pub fn set_changed(&mut self, snapshot: ChangedSnapshot) {
        self.changed = snapshot.files;
        self.changed_rows = snapshot.rows;
        self.ensure_selection();
    }

    pub fn changed_count(&self) -> usize {
        self.changed.len()
    }

    pub fn changed_contains(&self, path: &Path) -> bool {
        self.changed.iter().any(|file| file.relative_path == path)
    }

    pub fn changed_reference(&self, reference: &str) -> Option<PathBuf> {
        let normalized = reference.replace('\\', "/");
        self.changed
            .iter()
            .filter_map(|file| {
                let path = file.relative_path.to_string_lossy().replace('\\', "/");
                normalized
                    .contains(&path)
                    .then_some((path.len(), file.relative_path.clone()))
            })
            .max_by_key(|(length, _)| *length)
            .map(|(_, path)| path)
    }

    pub fn has_listing(&self, directory: &Path) -> bool {
        self.listings.contains_key(directory)
    }

    pub fn insert_listing(&mut self, listing: DirectoryListing) {
        self.listings.insert(listing.directory.clone(), listing);
        self.ensure_selection();
    }

    pub fn expand_ancestors(&mut self, path: &Path) {
        let mut current = PathBuf::new();
        if let Some(parent) = path.parent() {
            for component in parent.components() {
                if let Component::Normal(component) = component {
                    current.push(component);
                    self.expanded.insert(current.clone());
                }
            }
        }
    }

    pub fn select_path(&mut self, path: impl Into<PathBuf>) {
        self.selected = Some(path.into());
        self.ensure_selection();
    }

    pub fn selected_path(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    pub fn selected_index(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.rows_arc()
            .iter()
            .position(|row| &row.relative_path == selected)
    }

    pub fn move_selection(&mut self, delta: isize) {
        let rows = self.rows();
        if rows.is_empty() {
            self.selected = None;
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|path| rows.iter().position(|row| &row.relative_path == path))
            .unwrap_or(0);
        let next = current
            .saturating_add_signed(delta)
            .min(rows.len().saturating_sub(1));
        self.selected = Some(rows[next].relative_path.clone());
    }

    pub fn activate_selected(&mut self) -> FileTreeIntent {
        let Some(row) = self.selected_row() else {
            return FileTreeIntent::None;
        };
        match row.kind {
            TreeEntryKind::File => FileTreeIntent::OpenFile(row.relative_path),
            TreeEntryKind::Directory => self.toggle_directory(&row.relative_path),
        }
    }

    pub fn expand_selected(&mut self) -> FileTreeIntent {
        let Some(row) = self.selected_row() else {
            return FileTreeIntent::None;
        };
        if row.kind == TreeEntryKind::File {
            return FileTreeIntent::OpenFile(row.relative_path);
        }
        if self.expanded.insert(row.relative_path.clone()) {
            if !self.has_listing(&row.relative_path) {
                return FileTreeIntent::LoadDirectory(row.relative_path);
            }
            return FileTreeIntent::None;
        }
        let rows = self.rows();
        if let Some(index) = rows
            .iter()
            .position(|candidate| candidate.relative_path == row.relative_path)
            && let Some(child) = rows.get(index + 1)
            && child.depth == row.depth + 1
        {
            self.selected = Some(child.relative_path.clone());
        }
        FileTreeIntent::None
    }

    pub fn collapse_selected(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        if row.kind == TreeEntryKind::Directory && self.expanded.remove(&row.relative_path) {
            return;
        }
        let Some(parent) = row
            .relative_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        else {
            return;
        };
        if self
            .rows()
            .iter()
            .any(|candidate| candidate.relative_path == parent)
        {
            self.selected = Some(parent.to_path_buf());
        }
    }

    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    pub fn rows(&self) -> Vec<VisibleTreeRow> {
        self.rows_arc().as_ref().clone()
    }

    pub fn rows_arc(&self) -> Arc<Vec<VisibleTreeRow>> {
        if self.mode == FileTreeMode::Changed && self.filter.trim().is_empty() {
            return Arc::clone(&self.changed_rows);
        }
        let mut rows = match self.mode {
            FileTreeMode::Changed => self.changed_rows.as_ref().clone(),
            FileTreeMode::All => {
                let mut rows = Vec::new();
                self.collect_directory(Path::new(""), 0, &mut rows);
                rows
            }
        };
        if !self.filter.trim().is_empty() {
            rows.retain(|row| matches_filter(&row.relative_path, &self.filter));
        }
        rows.truncate(MAX_VISIBLE_ROWS);
        Arc::new(rows)
    }

    fn selected_row(&self) -> Option<VisibleTreeRow> {
        let selected = self.selected.as_ref()?;
        self.rows()
            .into_iter()
            .find(|row| &row.relative_path == selected)
    }

    fn toggle_directory(&mut self, path: &Path) -> FileTreeIntent {
        if !self.expanded.insert(path.to_path_buf()) {
            self.expanded.remove(path);
            return FileTreeIntent::None;
        }
        if self.has_listing(path) {
            FileTreeIntent::None
        } else {
            FileTreeIntent::LoadDirectory(path.to_path_buf())
        }
    }

    fn ensure_selection(&mut self) {
        let rows = self.rows_arc();
        if self
            .selected
            .as_ref()
            .is_some_and(|selected| rows.iter().any(|row| &row.relative_path == selected))
        {
            return;
        }
        self.selected = rows.first().map(|row| row.relative_path.clone());
    }

    fn collect_directory(&self, directory: &Path, depth: usize, rows: &mut Vec<VisibleTreeRow>) {
        if rows.len() >= MAX_VISIBLE_ROWS {
            return;
        }
        let Some(listing) = self.listings.get(directory) else {
            return;
        };
        for entry in &listing.entries {
            if rows.len() >= MAX_VISIBLE_ROWS {
                return;
            }
            rows.push(VisibleTreeRow {
                relative_path: entry.relative_path.clone(),
                label: entry.name.clone(),
                depth,
                kind: entry.kind,
                status: None,
            });
            if entry.kind == TreeEntryKind::Directory
                && self.expanded.contains(&entry.relative_path)
            {
                self.collect_directory(&entry.relative_path, depth + 1, rows);
            }
        }
    }
}

fn matches_filter(path: &Path, filter: &str) -> bool {
    let path = path.to_string_lossy().to_lowercase();
    filter
        .to_lowercase()
        .split_whitespace()
        .all(|token| path.contains(token))
}

fn validate_relative(path: &Path, allow_empty: bool) -> Result<(), FileTreeError> {
    if path.as_os_str().is_empty() {
        return if allow_empty {
            Ok(())
        } else {
            Err(FileTreeError::UnsafePath(path.to_path_buf()))
        };
    }
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(FileTreeError::UnsafePath(path.to_path_buf()));
    }
    Ok(())
}

fn directory_error(directory: &Path, operation: &'static str, error: io::Error) -> FileTreeError {
    FileTreeError::DirectoryIo {
        directory: directory.to_path_buf(),
        operation,
        message: error.to_string(),
    }
}

#[derive(Debug)]
pub enum FileTreeError {
    Unavailable {
        path: PathBuf,
        message: String,
    },
    UnsafePath(PathBuf),
    OutsideWorkspace(PathBuf),
    NotDirectory(PathBuf),
    DirectoryIo {
        directory: PathBuf,
        operation: &'static str,
        message: String,
    },
    Git(GitReviewError),
    RevealTooDeep {
        path: PathBuf,
        limit: usize,
    },
    RevealUnavailable {
        path: PathBuf,
        directory: PathBuf,
        truncated: bool,
    },
}

impl fmt::Display for FileTreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { path, message } => {
                write!(formatter, "Cannot browse {}: {message}", path.display())
            }
            Self::UnsafePath(path) => {
                write!(
                    formatter,
                    "Refusing to browse unsafe path {}",
                    path.display()
                )
            }
            Self::OutsideWorkspace(path) => write!(
                formatter,
                "Refusing to browse a directory outside the workspace: {}",
                path.display()
            ),
            Self::NotDirectory(path) => {
                write!(formatter, "{} is not a browsable directory", path.display())
            }
            Self::DirectoryIo {
                directory,
                operation,
                message,
            } => write!(
                formatter,
                "Could not {operation} directory {}: {message}",
                if directory.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    directory
                }
                .display()
            ),
            Self::Git(error) => error.fmt(formatter),
            Self::RevealTooDeep { path, limit } => write!(
                formatter,
                "Cannot reveal {}: the path exceeds the {limit}-directory limit",
                path.display()
            ),
            Self::RevealUnavailable {
                path,
                directory,
                truncated,
            } => write!(
                formatter,
                "Cannot reveal {} below {}{}",
                path.display(),
                if directory.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    directory
                }
                .display(),
                if *truncated {
                    " because that directory exceeds the bounded tree limit"
                } else {
                    " because it is missing, ignored, or unreadable"
                }
            ),
        }
    }
}

impl std::error::Error for FileTreeError {}

impl From<GitReviewError> for FileTreeError {
    fn from(error: GitReviewError) -> Self {
        Self::Git(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_review::{BranchInfo, FileChange};
    use std::process::Command;

    fn change(path: &str, original: Option<&str>, kind: ChangeKind) -> FileChange {
        FileChange {
            path: PathBuf::from(path),
            original_path: original.map(PathBuf::from),
            kind,
        }
    }

    fn listing(directory: &str, entries: &[(&str, TreeEntryKind)]) -> DirectoryListing {
        let directory = PathBuf::from(directory);
        DirectoryListing {
            directory: directory.clone(),
            entries: entries
                .iter()
                .map(|(name, kind)| DirectoryEntry {
                    relative_path: directory.join(name),
                    name: (*name).to_owned(),
                    kind: *kind,
                })
                .collect(),
            truncated: false,
        }
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn changed_projection_uses_review_status_and_git_xy_decorations() {
        let status = ReviewStatus {
            repo_root: PathBuf::from("/repo"),
            branch: BranchInfo::default(),
            staged: vec![
                change("both.rs", None, ChangeKind::Added),
                change("new.rs", None, ChangeKind::Added),
                change("renamed.rs", Some("old.rs"), ChangeKind::Renamed),
            ],
            unstaged: vec![
                change("both.rs", None, ChangeKind::Modified),
                change("deleted.rs", None, ChangeKind::Deleted),
                change("modified.rs", None, ChangeKind::Modified),
            ],
            untracked: vec![change("scratch.rs", None, ChangeKind::Added)],
            conflicted: vec![change("conflict.rs", None, ChangeKind::Unmerged)],
        };

        let snapshot = ChangedSnapshot::from(&status);
        let decorations: Vec<_> = snapshot
            .files
            .iter()
            .map(|file| {
                (
                    file.relative_path.to_string_lossy().into_owned(),
                    file.status.decoration(),
                )
            })
            .collect();
        assert_eq!(
            decorations,
            vec![
                ("both.rs".to_owned(), "AM".to_owned()),
                ("conflict.rs".to_owned(), "UU".to_owned()),
                ("deleted.rs".to_owned(), "·D".to_owned()),
                ("modified.rs".to_owned(), "·M".to_owned()),
                ("new.rs".to_owned(), "A·".to_owned()),
                ("renamed.rs".to_owned(), "R·".to_owned()),
                ("scratch.rs".to_owned(), "??".to_owned()),
            ]
        );
        assert_eq!(
            snapshot.files[5].status.original_path.as_deref(),
            Some(Path::new("old.rs"))
        );
    }

    #[test]
    fn model_defaults_to_changed_and_supports_reveal_and_keyboard_navigation() {
        let status = ReviewStatus {
            repo_root: PathBuf::from("/repo"),
            branch: BranchInfo::default(),
            unstaged: vec![change("src/lib.rs", None, ChangeKind::Modified)],
            ..ReviewStatus::default()
        };
        let mut model = FileTreeModel::default();
        model.set_changed(ChangedSnapshot::from(&status));
        assert_eq!(model.mode(), FileTreeMode::Changed);
        assert_eq!(
            model.changed_reference(" --> src/lib.rs:12:3"),
            Some(PathBuf::from("src/lib.rs"))
        );

        model.set_mode(FileTreeMode::All);
        model.insert_listing(listing("", &[("src", TreeEntryKind::Directory)]));
        model.insert_listing(listing(
            "src",
            &[
                ("lib.rs", TreeEntryKind::File),
                ("main.rs", TreeEntryKind::File),
            ],
        ));
        model.select_path("src");
        assert_eq!(
            model.expand_selected(),
            FileTreeIntent::None,
            "the loaded directory expands without another I/O request"
        );
        model.expand_selected();
        assert_eq!(model.selected_path(), Some(Path::new("src/lib.rs")));
        model.move_selection(1);
        assert_eq!(model.selected_path(), Some(Path::new("src/main.rs")));
        model.collapse_selected();
        assert_eq!(model.selected_path(), Some(Path::new("src")));
        model.set_filter("main");
        assert_eq!(model.rows().len(), 1);
        assert_eq!(model.selected_path(), Some(Path::new("src/main.rs")));
    }

    #[test]
    fn directory_load_is_lazy_ignore_aware_and_bounded() {
        let temporary = tempfile::tempdir().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(temporary.path())
                .status()
                .unwrap()
                .success()
        );
        write(
            &temporary.path().join(".gitignore"),
            "ignored.rs\nignored-dir/\n",
        );
        write(&temporary.path().join("visible.rs"), "visible\n");
        write(&temporary.path().join("ignored.rs"), "ignored\n");
        write(&temporary.path().join(":magic.rs"), "ignored magic\n");
        write(&temporary.path().join("ignored-dir/hidden.rs"), "ignored\n");
        write(&temporary.path().join("nested/child.rs"), "child\n");
        write(&temporary.path().join("tracked-ignored.rs"), "tracked\n");
        write(
            &temporary.path().join(".gitignore"),
            "ignored.rs\n:magic.rs\nignored-dir/\ntracked-ignored.rs\n",
        );
        assert!(
            Command::new("git")
                .args(["add", "-f", "tracked-ignored.rs"])
                .current_dir(temporary.path())
                .status()
                .unwrap()
                .success()
        );

        let workspace = FileTreeWorkspace::for_session(temporary.path()).unwrap();
        let root = workspace.load_directory(Path::new("")).unwrap();
        let paths: Vec<_> = root
            .entries
            .iter()
            .map(|entry| entry.relative_path.as_path())
            .collect();
        assert!(paths.contains(&Path::new("visible.rs")));
        assert!(paths.contains(&Path::new("tracked-ignored.rs")));
        assert!(paths.contains(&Path::new("nested")));
        assert!(!paths.contains(&Path::new("ignored.rs")));
        assert!(!paths.contains(&Path::new(":magic.rs")));
        assert!(!paths.contains(&Path::new("ignored-dir")));
        assert!(!paths.contains(&Path::new(".git")));
        assert!(
            root.entries
                .iter()
                .all(|entry| entry.relative_path != Path::new("nested/child.rs")),
            "the root load must not eagerly traverse nested directories"
        );

        let bounded = workspace
            .load_directory_with_limits(Path::new(""), 2, 100)
            .unwrap();
        assert_eq!(bounded.entries.len(), 2);
        assert!(bounded.truncated);
    }

    #[test]
    fn reveal_loads_only_the_ancestor_chain() {
        let temporary = tempfile::tempdir().unwrap();
        write(&temporary.path().join("src/deep/file.rs"), "source\n");
        write(&temporary.path().join("other/untouched.rs"), "other\n");
        let workspace = FileTreeWorkspace::for_session(temporary.path()).unwrap();

        let listings = workspace.reveal(Path::new("src/deep/file.rs")).unwrap();
        let directories: Vec<_> = listings
            .iter()
            .map(|listing| listing.directory.as_path())
            .collect();
        assert_eq!(
            directories,
            vec![Path::new(""), Path::new("src"), Path::new("src/deep")]
        );
        assert!(
            listings
                .iter()
                .all(|listing| listing.directory != Path::new("other"))
        );
    }
}
