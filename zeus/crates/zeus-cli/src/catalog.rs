use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use zeus_proto::AgentKind;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDescriptor {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub short_label: String,
    #[serde(default)]
    pub glyph: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub first_class: bool,
    #[serde(default)]
    pub binary: Option<String>,
}

#[derive(Deserialize)]
struct ManifestFile {
    agent: Option<AgentDescriptor>,
}

#[derive(Clone, Debug)]
pub struct AgentCatalog {
    pub ordered: Vec<AgentDescriptor>,
}

impl AgentCatalog {
    pub fn shared() -> &'static Self {
        static CATALOG: OnceLock<AgentCatalog> = OnceLock::new();
        CATALOG.get_or_init(Self::load)
    }

    pub fn load() -> Self {
        let mut descriptors: Vec<AgentDescriptor> = Vec::new();
        for dir in manifest_dirs() {
            if !dir.is_dir() {
                continue;
            }
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            let mut paths: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .collect();
            paths.sort();
            for path in paths {
                let Ok(bytes) = fs::read(&path) else {
                    continue;
                };
                let Ok(file) = serde_json::from_slice::<ManifestFile>(&bytes) else {
                    continue;
                };
                if let Some(agent) = file.agent
                    && !agent.id.is_empty()
                {
                    if let Some(existing) = descriptors.iter_mut().find(|item| item.id == agent.id)
                    {
                        *existing = agent;
                    } else {
                        descriptors.push(agent);
                    }
                }
            }
        }
        descriptors.sort_by(|left, right| left.id.cmp(&right.id));
        Self {
            ordered: descriptors,
        }
    }

    pub fn launchable(&self) -> impl Iterator<Item = &AgentDescriptor> {
        self.ordered.iter().filter(|item| item.binary.is_some())
    }

    pub fn resolve(&self, name: &str) -> Option<&AgentDescriptor> {
        let needle = name.to_ascii_lowercase();
        self.ordered
            .iter()
            .find(|item| item.id == needle)
            .or_else(|| {
                self.ordered.iter().find(|item| {
                    item.short_label.eq_ignore_ascii_case(&needle)
                        || item
                            .aliases
                            .iter()
                            .any(|alias| alias.eq_ignore_ascii_case(&needle))
                })
            })
    }

    pub fn descriptor(&self, id: &str) -> Option<&AgentDescriptor> {
        self.ordered.iter().find(|item| item.id == id)
    }
}

pub fn parse_kind(raw: &str) -> AgentKind {
    match raw.to_ascii_lowercase().as_str() {
        "shell" | "sh" | "bash" | "zsh" | "fish" => AgentKind::SHELL,
        _ => AgentCatalog::shared()
            .resolve(raw)
            .map(|descriptor| AgentKind::new(descriptor.id.clone()))
            .unwrap_or_else(|| AgentKind::generic(raw)),
    }
}

pub fn spawnable_kind_labels() -> Vec<String> {
    let mut labels: Vec<String> = AgentCatalog::shared()
        .launchable()
        .map(|item| item.short_label.clone())
        .collect();
    if !labels.iter().any(|label| label == "shell") {
        labels.push("shell".into());
    }
    labels
}

pub fn spawnable_display_names() -> String {
    let mut names: Vec<String> = AgentCatalog::shared()
        .launchable()
        .map(|item| item.display_name.clone())
        .collect();
    names.push("a shell".into());
    match names.as_slice() {
        [] => "an agent".into(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

pub fn short_label(kind: &AgentKind) -> String {
    if let Some(command) = kind.command() {
        return Path::new(command)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(command)
            .to_string();
    }
    AgentCatalog::shared()
        .descriptor(kind.id())
        .map(|item| item.short_label.clone())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| kind.id().to_string())
}

pub fn glyph(kind: &AgentKind) -> String {
    AgentCatalog::shared()
        .descriptor(kind.id())
        .map(|item| item.glyph.clone())
        .filter(|mark| !mark.is_empty())
        .unwrap_or_else(|| "•".into())
}

fn manifest_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(configured) = std::env::var("ZEUS_MANIFESTS_DIR") {
        dirs.push(PathBuf::from(configured));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        dirs.push(parent.join("manifests"));
    }
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join("Library/Application Support/Zeus/bin/manifests"));
    }
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../zeus-engine/manifests"));
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_loads_every_bundled_agent() {
        let catalog = AgentCatalog::load();
        assert!(
            catalog.ordered.len() >= 20,
            "catalog shrank to {}",
            catalog.ordered.len()
        );
        for descriptor in catalog.launchable() {
            let parsed = parse_kind(&descriptor.id);
            assert_eq!(parsed.id(), descriptor.id);
        }
        assert_eq!(parse_kind("claude-code"), AgentKind::CLAUDE_CODE);
        assert_eq!(parse_kind("CLAUDE"), AgentKind::CLAUDE_CODE);
        assert_eq!(parse_kind("bash"), AgentKind::SHELL);
        assert_eq!(parse_kind("htop").command(), Some("htop"));
    }
}
