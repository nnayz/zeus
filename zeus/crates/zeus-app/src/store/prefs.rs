use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeus_proto::{AgentKind, ProjectId, SessionId};

const DEFAULT_THEME: &str = "zeus-dark";

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowMode {
    #[default]
    Windowed,
    Maximized,
    Fullscreen,
}

/// The last desktop window placement, stored without GPUI types so the
/// preferences file stays a plain, forwards-compatible JSON document.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowPlacement {
    #[serde(default)]
    pub display_uuid: Option<String>,
    pub mode: WindowMode,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InspectorTab {
    #[default]
    Info,
    Changes,
    Code,
    Artifacts,
}

/// Preferences intentionally persist the manifest id as a plain string. The
/// four pre-catalog enum spellings are accepted forever because prefs survive
/// upgrades; new saves use the canonical manifest ids (for example
/// `"claude-code"` and `"opencode"`).
fn serialize_default_agent<S>(agent: &AgentKind, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(agent.id())
}

fn deserialize_default_agent<'de, D>(deserializer: D) -> Result<AgentKind, D::Error>
where
    D: Deserializer<'de>,
{
    let saved = String::deserialize(deserializer)?;
    Ok(match saved.as_str() {
        "claudeCode" | AgentKind::CLAUDE_CODE_ID => AgentKind::CLAUDE_CODE,
        "codex" => AgentKind::CODEX,
        "cursor" => AgentKind::CURSOR,
        "gemini" => AgentKind::GEMINI,
        "shell" => AgentKind::SHELL,
        _ => AgentKind::new(saved),
    })
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    #[serde(
        serialize_with = "serialize_default_agent",
        deserialize_with = "deserialize_default_agent"
    )]
    pub default_agent: AgentKind,
    /// Persistent destination for global new-session shortcuts. `None` means
    /// this Mac; a host id means that configured remote host. The alias
    /// migrates preferences written by the earlier last-used implementation.
    #[serde(alias = "lastSpawnHost")]
    pub default_spawn_host: Option<String>,
    pub start_at_login: bool,
    pub confirm_before_closing_session: bool,
    pub status_sounds: bool,
    /// Check the releases feed in the background. Downloading and installing
    /// always stay manual — see `crate::updates`.
    pub automatic_updates: bool,
    /// A release the user chose not to install. Persisted so "Skip" outlives
    /// the session that clicked it; empty means nothing is skipped.
    pub skipped_update_version: String,
    pub hibernate_after_minutes: u32,
    pub memory_hard_limit_gb: u64,
    pub terminal_theme: String,
    pub terminal_font_size: f32,
    /// Last size, position, and presentation mode of the main window.
    pub window_placement: Option<WindowPlacement>,
    /// Whether the leading sidebar was mounted when the app last ran.
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
    /// Mirrored workbench: projects sidebar on the trailing edge and the
    /// inspector on the leading one. This is the default arrangement.
    pub sidebar_on_right: bool,
    /// Whether the trailing workbench inspector is mounted.
    pub inspector_open: bool,
    /// Width of the trailing workbench inspector in points.
    pub inspector_width: f32,
    /// Last selected tab in the trailing workbench inspector.
    pub inspector_tab: InspectorTab,
    /// Fraction of the terminal workbench reserved for the primary pane when
    /// the lower terminal is open.
    pub workbench_primary_fraction: f32,
    /// Newline-separated roots, matching the Swift settings text field.
    pub quick_open_roots: String,
    pub sidebar_project_order: Vec<ProjectId>,
    pub sidebar_session_order: Vec<SessionId>,
    pub sidebar_pinned_projects: Vec<ProjectId>,
    pub sidebar_pinned_sessions: Vec<SessionId>,
    pub sidebar_collapsed_projects: Vec<ProjectId>,
    /// Sessions whose spawned children are folded away.
    pub sidebar_collapsed_sessions: Vec<SessionId>,
    pub sidebar_expanded_archives: Vec<ProjectId>,
    /// Session that should regain focus after the daemon's initial hydrate.
    pub last_selected_session: Option<SessionId>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            default_agent: AgentKind::CLAUDE_CODE,
            default_spawn_host: None,
            start_at_login: false,
            confirm_before_closing_session: true,
            status_sounds: true,
            automatic_updates: true,
            skipped_update_version: String::new(),
            hibernate_after_minutes: 15,
            memory_hard_limit_gb: 6,
            terminal_theme: DEFAULT_THEME.to_owned(),
            terminal_font_size: 13.0,
            window_placement: None,
            sidebar_visible: true,
            sidebar_width: 232.0,
            sidebar_on_right: true,
            inspector_open: true,
            inspector_width: 400.0,
            inspector_tab: InspectorTab::Info,
            workbench_primary_fraction: crate::workbench::DEFAULT_PRIMARY_FRACTION,
            quick_open_roots: String::new(),
            sidebar_project_order: Vec::new(),
            sidebar_session_order: Vec::new(),
            sidebar_pinned_projects: Vec::new(),
            sidebar_pinned_sessions: Vec::new(),
            sidebar_collapsed_projects: Vec::new(),
            sidebar_collapsed_sessions: Vec::new(),
            sidebar_expanded_archives: Vec::new(),
            last_selected_session: None,
        }
    }
}

impl Prefs {
    pub const MIN_TERMINAL_FONT_SIZE: f32 = 10.0;
    pub const MAX_TERMINAL_FONT_SIZE: f32 = 20.0;

    pub fn path() -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/nonexistent"));
        Self::path_in_home(&home)
    }

    pub fn path_in_home(home: &Path) -> PathBuf {
        home.join("Library/Application Support/zeus/prefs.json")
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let mut prefs: Self = serde_json::from_slice(&bytes)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                prefs.normalize();
                Ok(prefs)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut normalized = self.clone();
        normalized.normalize();
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "preference path has no parent")
        })?;
        fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec_pretty(&normalized)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)
    }

    pub fn zoom_terminal(&mut self, delta: f32) {
        self.terminal_font_size = (self.terminal_font_size + delta)
            .clamp(Self::MIN_TERMINAL_FONT_SIZE, Self::MAX_TERMINAL_FONT_SIZE);
    }

    pub fn reset_terminal_zoom(&mut self) {
        self.terminal_font_size = 13.0;
    }

    pub fn normalize(&mut self) {
        if !self.terminal_font_size.is_finite() {
            self.terminal_font_size = 13.0;
        }
        self.terminal_font_size = self
            .terminal_font_size
            .clamp(Self::MIN_TERMINAL_FONT_SIZE, Self::MAX_TERMINAL_FONT_SIZE);
        if let Some(placement) = &mut self.window_placement {
            let valid = placement.x.is_finite()
                && placement.y.is_finite()
                && placement.width.is_finite()
                && placement.height.is_finite()
                && placement.width > 0.0
                && placement.height > 0.0;
            if valid {
                placement.width = placement.width.max(900.0);
                placement.height = placement.height.max(560.0);
            } else {
                self.window_placement = None;
            }
        }
        // Carry installations that still have the former untouched defaults
        // into the compact layout, while preserving any width the user
        // actually resized to.
        if self.sidebar_width == 248.0 {
            self.sidebar_width = 232.0;
        }
        if !self.sidebar_width.is_finite() {
            self.sidebar_width = 232.0;
        }
        self.sidebar_width = self.sidebar_width.clamp(184.0, 360.0);
        if self.inspector_width == 440.0 {
            self.inspector_width = 400.0;
        }
        if !self.inspector_width.is_finite() {
            self.inspector_width = 400.0;
        }
        self.inspector_width = self.inspector_width.clamp(300.0, 720.0);
        if !self.workbench_primary_fraction.is_finite() {
            self.workbench_primary_fraction = crate::workbench::DEFAULT_PRIMARY_FRACTION;
        }
        self.workbench_primary_fraction = self.workbench_primary_fraction.clamp(0.0, 1.0);
        if self.terminal_theme.is_empty() {
            self.terminal_theme = DEFAULT_THEME.to_owned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Prefs;

    #[test]
    fn former_panel_defaults_migrate_to_compact_widths() {
        let mut prefs = Prefs {
            sidebar_width: 248.0,
            inspector_width: 440.0,
            ..Prefs::default()
        };

        prefs.normalize();

        assert_eq!(prefs.sidebar_width, 232.0);
        assert_eq!(prefs.inspector_width, 400.0);
    }

    /// Preferences written before the mirrored workbench existed have no
    /// `sidebarOnRight` key, and must still land on the new default.
    #[test]
    fn saved_preferences_without_a_layout_choice_adopt_the_mirrored_default() {
        let restored: Prefs = serde_json::from_str(r#"{"sidebarWidth": 232.0}"#).expect("prefs");

        assert!(restored.sidebar_on_right);
        assert!(Prefs::default().sidebar_on_right);
    }

    #[test]
    fn explicit_panel_widths_survive_compact_normalization() {
        let mut prefs = Prefs {
            sidebar_width: 276.0,
            inspector_width: 512.0,
            ..Prefs::default()
        };

        prefs.normalize();

        assert_eq!(prefs.sidebar_width, 276.0);
        assert_eq!(prefs.inspector_width, 512.0);
    }
}
