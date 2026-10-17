//! Persisted panel layout. Live viewers and terminal attachments belong to the UI.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};
use taskmaster_proto::SessionId;

pub(crate) const MAX_PANEL_TABS: usize = 32;
const MAX_TITLE_CHARS: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PanelTabKind {
    Details,
    Review,
    Files,
    Terminal,
    Browser,
    File,
}

impl PanelTabKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Details => "Details",
            Self::Review => "Review",
            Self::Files => "Files",
            Self::Terminal => "Terminal",
            Self::Browser => "Browser",
            Self::File => "File",
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::Details => "square.stack.3d.up",
            Self::Review => "arrow.triangle.pull",
            Self::Files => "folder",
            Self::Terminal => "terminal",
            Self::Browser => "network",
            Self::File => "chevron.left.forwardslash.chevron.right",
        }
    }

    pub(crate) fn is_singleton(self) -> bool {
        matches!(self, Self::Details | Self::Review | Self::Files)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PanelTabFile {
    pub cwd: PathBuf,
    pub reference: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PanelTab {
    pub id: u64,
    pub kind: PanelTabKind,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PanelTabFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
}

impl PanelTab {
    pub(crate) fn label(&self) -> &str {
        if self.title.is_empty() {
            self.kind.label()
        } else {
            &self.title
        }
    }

    pub(crate) fn icon(&self) -> &'static str {
        self.kind.icon()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PanelTabs {
    pub tabs: Vec<PanelTab>,
    pub active: Option<u64>,
    // Zero marks exhausted IDs; a closed tab's identity is never reused.
    next_id: u64,
}

impl Default for PanelTabs {
    fn default() -> Self {
        Self::new(PanelTabKind::Details)
    }
}

impl<'de> Deserialize<'de> for PanelTabs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct SavedTabs {
            #[serde(default)]
            tabs: Vec<serde_json::Value>,
            #[serde(default)]
            active: Option<u64>,
            #[serde(default = "first_id")]
            next_id: u64,
        }
        fn first_id() -> u64 {
            1
        }

        let saved = SavedTabs::deserialize(deserializer)?;
        let max_saved_id = saved
            .tabs
            .iter()
            .filter_map(|tab| tab.get("id").and_then(serde_json::Value::as_u64))
            .max()
            .unwrap_or(0);
        let next_id = if saved.next_id == 0 {
            0
        } else {
            max_saved_id
                .checked_add(1)
                .map_or(0, |minimum| saved.next_id.max(minimum))
        };
        let mut tabs = Self {
            // A future tab kind or malformed tab must not invalidate all preferences.
            tabs: saved
                .tabs
                .into_iter()
                .filter_map(|tab| serde_json::from_value(tab).ok())
                .collect(),
            active: saved.active,
            next_id,
        };
        tabs.normalize();
        Ok(tabs)
    }
}

impl PanelTabs {
    pub(crate) fn new(initial: PanelTabKind) -> Self {
        let mut tabs = Self::empty();
        tabs.open(initial);
        tabs
    }

    pub(crate) fn empty() -> Self {
        Self {
            tabs: Vec::new(),
            active: None,
            next_id: 1,
        }
    }

    pub(crate) fn active_tab(&self) -> Option<&PanelTab> {
        self.tabs.iter().find(|tab| Some(tab.id) == self.active)
    }

    pub(crate) fn open(&mut self, kind: PanelTabKind) -> Option<u64> {
        if kind == PanelTabKind::File {
            return None;
        }
        if kind.is_singleton()
            && let Some(tab) = self.tabs.iter().find(|tab| tab.kind == kind)
        {
            self.active = Some(tab.id);
            return self.active;
        }
        let title = if matches!(kind, PanelTabKind::Terminal | PanelTabKind::Browser) {
            let prefix = format!("{} ", kind.label());
            let number = self
                .tabs
                .iter()
                .filter(|tab| tab.kind == kind)
                .filter_map(|tab| tab.title.strip_prefix(&prefix)?.parse::<usize>().ok())
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            format!("{prefix}{number}")
        } else {
            kind.label().to_owned()
        };
        self.insert(kind, title, None)
    }

    pub(crate) fn open_file(&mut self, cwd: PathBuf, reference: String) -> Option<u64> {
        if reference.trim().is_empty() {
            return None;
        }
        let file = PanelTabFile { cwd, reference };
        if let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| tab.file.as_ref() == Some(&file))
        {
            self.active = Some(tab.id);
            return self.active;
        }
        let title = file_title(&file.reference);
        self.insert(PanelTabKind::File, title, Some(file))
    }

    fn insert(
        &mut self,
        kind: PanelTabKind,
        title: String,
        file: Option<PanelTabFile>,
    ) -> Option<u64> {
        if self.tabs.len() >= MAX_PANEL_TABS || self.next_id == 0 {
            return None;
        }
        let id = self.next_id;
        self.next_id = id.checked_add(1).unwrap_or(0);
        self.tabs.push(PanelTab {
            id,
            kind,
            title: title.chars().take(MAX_TITLE_CHARS).collect(),
            file,
            session_id: None,
        });
        self.active = Some(id);
        Some(id)
    }

    pub(crate) fn select(&mut self, id: u64) -> bool {
        if self.active == Some(id) || !self.tabs.iter().any(|tab| tab.id == id) {
            return false;
        }
        self.active = Some(id);
        true
    }

    pub(crate) fn select_relative(&mut self, delta: isize) -> bool {
        if self.tabs.is_empty() {
            return false;
        }
        let Some(index) = self.tabs.iter().position(|tab| Some(tab.id) == self.active) else {
            self.active = Some(self.tabs[0].id);
            return true;
        };
        let count = self.tabs.len() as isize;
        // Reduce before adding so even isize::MAX/MIN are safe.
        let next = (index as isize + delta.rem_euclid(count)).rem_euclid(count) as usize;
        self.select(self.tabs[next].id)
    }

    pub(crate) fn close(&mut self, id: u64) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return false;
        };
        self.tabs.remove(index);
        if self.active == Some(id) {
            self.active = self
                .tabs
                .get(index.min(self.tabs.len().saturating_sub(1)))
                .map(|tab| tab.id);
        }
        true
    }

    pub(crate) fn close_active(&mut self) -> bool {
        self.active.is_some_and(|id| self.close(id))
    }

    /// Repair saved metadata while preserving an explicitly empty panel.
    pub(crate) fn normalize(&mut self) {
        let max_seen = self.tabs.iter().map(|tab| tab.id).max().unwrap_or(0);
        if self.next_id != 0 {
            self.next_id = max_seen
                .checked_add(1)
                .map_or(0, |minimum| self.next_id.max(minimum));
        }
        let mut ids = HashSet::new();
        let mut singletons = HashSet::new();
        let mut files = Vec::new();
        let mut retained = 0;
        self.tabs.retain_mut(|tab| {
            if retained >= MAX_PANEL_TABS || tab.id == 0 || !ids.insert(tab.id) {
                return false;
            }
            if tab.kind.is_singleton() && !singletons.insert(tab.kind) {
                return false;
            }
            if tab.kind == PanelTabKind::File {
                let Some(file) = tab.file.as_ref() else {
                    return false;
                };
                if file.reference.trim().is_empty() || files.contains(file) {
                    return false;
                }
                files.push(file.clone());
                if tab.title.is_empty() {
                    tab.title = file_title(&file.reference);
                }
            } else {
                tab.file = None;
            }
            if tab.kind != PanelTabKind::Terminal {
                tab.session_id = None;
            }
            if tab.title.is_empty() {
                tab.title = tab.kind.label().to_owned();
            }
            tab.title = tab.title.chars().take(MAX_TITLE_CHARS).collect();
            retained += 1;
            true
        });
        if !self.tabs.iter().any(|tab| Some(tab.id) == self.active) {
            self.active = self.tabs.first().map(|tab| tab.id);
        }
    }
}

fn file_title(reference: &str) -> String {
    let mut path = reference;
    for _ in 0..2 {
        if let Some((prefix, suffix)) = path.rsplit_once(':')
            && !suffix.is_empty()
            && suffix.bytes().all(|byte| byte.is_ascii_digit())
        {
            path = prefix;
        }
    }
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("File")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singleton_open_focuses_existing_but_terminals_and_browsers_repeat() {
        let mut tabs = PanelTabs::default();
        let details = tabs.active.unwrap();
        let terminal = tabs.open(PanelTabKind::Terminal).unwrap();
        let another_terminal = tabs.open(PanelTabKind::Terminal).unwrap();
        assert_eq!(tabs.active_tab().unwrap().label(), "Terminal 2");
        let browser = tabs.open(PanelTabKind::Browser).unwrap();
        let another_browser = tabs.open(PanelTabKind::Browser).unwrap();
        assert_ne!(terminal, another_terminal);
        assert_ne!(browser, another_browser);
        assert_eq!(tabs.open(PanelTabKind::Details), Some(details));
        assert_eq!(tabs.active, Some(details));
        assert_eq!(tabs.tabs.len(), 5);
    }

    #[test]
    fn closing_active_selects_right_then_left_and_last_stays_empty() {
        let mut tabs = PanelTabs::default();
        let first = tabs.active.unwrap();
        let middle = tabs.open(PanelTabKind::Review).unwrap();
        let last = tabs.open(PanelTabKind::Files).unwrap();
        assert!(tabs.select(middle));
        assert!(tabs.close(middle));
        assert_eq!(tabs.active, Some(last));
        assert!(tabs.close(last));
        assert_eq!(tabs.active, Some(first));
        assert!(tabs.close_active());
        tabs.normalize();
        assert!(tabs.tabs.is_empty());
        assert_eq!(tabs.active, None);
        assert!(!tabs.close_active());
        assert!(!tabs.select_relative(1));
        assert!(!tabs.select(123));
    }

    #[test]
    fn closing_background_preserves_selection_and_relative_selection_wraps() {
        let mut tabs = PanelTabs::default();
        let first = tabs.active.unwrap();
        let middle = tabs.open(PanelTabKind::Review).unwrap();
        let last = tabs.open(PanelTabKind::Files).unwrap();
        assert!(tabs.close(middle));
        assert_eq!(tabs.active, Some(last));
        assert!(tabs.select_relative(1));
        assert_eq!(tabs.active, Some(first));
        assert!(tabs.select_relative(-1));
        assert_eq!(tabs.active, Some(last));
        assert!(!tabs.select_relative(2));
        assert!(tabs.select_relative(isize::MIN + 1));
    }

    #[test]
    fn serde_keeps_empty_state_ids_and_terminal_session_without_live_entities() {
        let mut tabs = PanelTabs::empty();
        let terminal = tabs.open(PanelTabKind::Terminal).unwrap();
        tabs.tabs[0].session_id = Some(SessionId("panel-shell".into()));
        let saved = serde_json::to_string(&tabs).unwrap();
        let restored: PanelTabs = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored, tabs);
        tabs.close(terminal);
        let mut restored: PanelTabs =
            serde_json::from_str(&serde_json::to_string(&tabs).unwrap()).unwrap();
        assert!(restored.tabs.is_empty());
        assert_eq!(restored.active, None);
        assert!(restored.open(PanelTabKind::Terminal).unwrap() > terminal);
    }

    #[test]
    fn named_files_keep_location_and_focus_existing_reference() {
        let mut tabs = PanelTabs::empty();
        let cwd = PathBuf::from("/work/project");
        let id = tabs
            .open_file(cwd.clone(), "src/lib.rs:42:3".into())
            .unwrap();
        assert_eq!(tabs.active_tab().unwrap().label(), "lib.rs");
        tabs.open(PanelTabKind::Files);
        assert_eq!(tabs.open_file(cwd, "src/lib.rs:42:3".into()), Some(id));
        assert_eq!(tabs.tabs.len(), 2);
        assert!(
            tabs.open_file(PathBuf::from("/other"), "LICENSE".into())
                .is_some()
        );
        let restored: PanelTabs =
            serde_json::from_str(&serde_json::to_string(&tabs).unwrap()).unwrap();
        assert_eq!(restored, tabs);
        assert!(tabs.open(PanelTabKind::File).is_none());
    }

    #[test]
    fn unknown_and_corrupt_tabs_do_not_discard_other_saved_tabs() {
        let mut tabs: PanelTabs = serde_json::from_value(serde_json::json!({
            "tabs": [
                {"id": 0, "kind": "details"},
                {"id": 2, "kind": "details"},
                {"id": 3, "kind": "details"},
                {"id": 2, "kind": "review"},
                {"id": 40, "kind": "futureKind"},
                {"id": 5, "kind": "terminal"},
                {"id": 6, "kind": "file"},
                {"id": "invalid", "kind": "files"}
            ],
            "active": 40
        }))
        .unwrap();
        assert_eq!(
            tabs.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            vec![2, 5]
        );
        assert_eq!(tabs.active, Some(2));
        // Even unknown kinds' IDs are not reused by later content.
        assert!(tabs.open(PanelTabKind::Review).unwrap() > 40);
    }

    #[test]
    fn capacity_keeps_singletons_focusable_and_normalize_bounds_saved_tabs() {
        let mut tabs = PanelTabs::default();
        let details = tabs.active.unwrap();
        for _ in 1..MAX_PANEL_TABS {
            assert!(tabs.open(PanelTabKind::Terminal).is_some());
        }
        assert!(tabs.open(PanelTabKind::Browser).is_none());
        assert_eq!(tabs.open(PanelTabKind::Details), Some(details));
        let mut saved = serde_json::to_value(&tabs).unwrap();
        saved["tabs"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "id": 100, "kind": "terminal", "title": "extra"
            }));
        let mut restored: PanelTabs = serde_json::from_value(saved).unwrap();
        assert_eq!(restored.tabs.len(), MAX_PANEL_TABS);
        restored.close(details);
        assert!(restored.open(PanelTabKind::Browser).unwrap() > 100);
    }

    #[test]
    fn exhausted_id_counter_never_wraps_or_reuses_an_id() {
        let mut tabs = PanelTabs::empty();
        tabs.next_id = u64::MAX;
        let id = tabs.open(PanelTabKind::Terminal).unwrap();
        assert_eq!(id, u64::MAX);
        tabs.close(id);
        assert!(tabs.open(PanelTabKind::Browser).is_none());
        let mut restored: PanelTabs =
            serde_json::from_str(&serde_json::to_string(&tabs).unwrap()).unwrap();
        assert!(restored.open(PanelTabKind::Terminal).is_none());
    }
}
