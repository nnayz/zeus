//! Reusable New Agent launch recipes.
//!
//! Recipes persist in preferences and always resolve through [`SpawnOptions`]
//! so they cannot bypass agent, project, host, or worktree checks.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use zeus_proto::{AgentKind, AgentReadinessResult, HostEntry, Project, ProjectId};

use super::prefs::{deserialize_preference_agent, serialize_preference_agent};
use super::{SpawnOptions, WorktreeSpawn};

pub const CURRENT_RECIPES_VERSION: u8 = 1;

pub fn legacy_recipes_version() -> u8 {
    0
}

/// One saved New Agent configuration. Agent and host use stable ids; project
/// selection is either a Zeus project id or an explicit path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRecipe {
    pub id: String,
    pub name: String,
    #[serde(
        serialize_with = "serialize_preference_agent",
        deserialize_with = "deserialize_preference_agent"
    )]
    pub agent: AgentKind,
    pub project: RecipeProject,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<RecipeWorktree>,
    #[serde(default)]
    pub initial_prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// How a recipe names its working directory. Never resolve by display name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RecipeProject {
    Project { id: ProjectId },
    Path { path: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeWorktree {
    #[serde(default)]
    pub create: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// One-off field changes that do not mutate the stored recipe.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecipeOverrides {
    pub agent: Option<AgentKind>,
    pub project: Option<RecipeProject>,
    pub host: Option<Option<String>>,
    pub worktree: Option<Option<RecipeWorktree>>,
    pub initial_prompt: Option<String>,
    pub title: Option<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRecipe {
    pub kind: AgentKind,
    pub options: SpawnOptions,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecipeIssue {
    MissingAgent {
        id: String,
        display_name: String,
    },
    UnavailableAgent {
        id: String,
        display_name: String,
        detail: String,
    },
    MissingProject {
        id: ProjectId,
    },
    EmptyProject,
    MissingHost {
        id: String,
    },
    RemoteWorktree,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipeRepair {
    Agent,
    Project,
    Host,
    Worktree,
}

impl LaunchRecipe {
    pub fn new(
        name: impl Into<String>,
        agent: AgentKind,
        project: RecipeProject,
        host: Option<String>,
        worktree: Option<RecipeWorktree>,
        initial_prompt: impl Into<String>,
        title: Option<String>,
    ) -> Self {
        Self {
            id: new_recipe_id(),
            name: name.into(),
            agent,
            project,
            host,
            worktree,
            initial_prompt: initial_prompt.into(),
            title: normalize_title(title),
        }
    }

    pub fn is_well_formed(&self) -> bool {
        !self.id.trim().is_empty() && !self.name.trim().is_empty()
    }

    pub fn with_overrides(&self, overrides: &RecipeOverrides) -> Self {
        let mut recipe = self.clone();
        if let Some(agent) = &overrides.agent {
            recipe.agent = agent.clone();
        }
        if let Some(project) = &overrides.project {
            recipe.project = project.clone();
        }
        if let Some(host) = &overrides.host {
            recipe.host.clone_from(host);
        }
        if let Some(worktree) = &overrides.worktree {
            recipe.worktree.clone_from(worktree);
        }
        if let Some(prompt) = &overrides.initial_prompt {
            recipe.initial_prompt.clone_from(prompt);
        }
        if let Some(title) = &overrides.title {
            recipe.title = normalize_title(title.clone());
        }
        recipe
    }
}

impl RecipeIssue {
    pub fn message(&self) -> String {
        match self {
            Self::MissingAgent { display_name, id } => {
                format!("{display_name} ({id}) is not in the agent catalog")
            }
            Self::UnavailableAgent {
                display_name,
                detail,
                ..
            } => format!("{display_name}: {detail}"),
            Self::MissingProject { id } => {
                format!("Project {id} is missing from this machine")
            }
            Self::EmptyProject => "Choose a project or working directory".to_owned(),
            Self::MissingHost { id } => format!("Host {id} is not in hosts.json"),
            Self::RemoteWorktree => "Remote recipes cannot create a local git worktree".to_owned(),
        }
    }

    pub fn repair(&self) -> RecipeRepair {
        match self {
            Self::MissingAgent { .. } | Self::UnavailableAgent { .. } => RecipeRepair::Agent,
            Self::MissingProject { .. } | Self::EmptyProject => RecipeRepair::Project,
            Self::MissingHost { .. } => RecipeRepair::Host,
            Self::RemoteWorktree => RecipeRepair::Worktree,
        }
    }

    pub fn repair_label(&self) -> &'static str {
        match self.repair() {
            RecipeRepair::Agent => "Choose agent",
            RecipeRepair::Project => "Choose project",
            RecipeRepair::Host => "Choose host",
            RecipeRepair::Worktree => "Turn off worktree",
        }
    }
}

pub fn new_recipe_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("recipe-{nanos:x}")
}

pub fn duplicate_recipe_name(name: &str) -> String {
    format!("Copy of {name}")
}

pub fn recipe_project_for(root: &str, projects: &HashMap<ProjectId, Project>) -> RecipeProject {
    launch_project_ref(root, false, projects)
}

/// Remote recipes store an explicit path so they cannot bind to a local
/// project id and later pick up a different checkout.
pub fn launch_project_ref(
    root: &str,
    remote: bool,
    projects: &HashMap<ProjectId, Project>,
) -> RecipeProject {
    if remote {
        let path = root.trim();
        return RecipeProject::Path {
            path: if path.is_empty() {
                "~".to_owned()
            } else {
                path.to_owned()
            },
        };
    }
    projects
        .values()
        .find(|project| project.root == root)
        .map(|project| RecipeProject::Project {
            id: project.id.clone(),
        })
        .unwrap_or_else(|| RecipeProject::Path {
            path: root.to_owned(),
        })
}

pub fn normalize_title(title: Option<String>) -> Option<String> {
    title.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

/// Skip malformed entries instead of failing the whole preferences file.
pub fn deserialize_launch_recipes<'de, D>(deserializer: D) -> Result<Vec<LaunchRecipe>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(parse_launch_recipes(value.unwrap_or(Value::Null)))
}

pub fn parse_launch_recipes(value: Value) -> Vec<LaunchRecipe> {
    let Value::Array(items) = value else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| serde_json::from_value::<LaunchRecipe>(item).ok())
        .filter(LaunchRecipe::is_well_formed)
        .collect()
}

pub fn resolve_recipe(
    recipe: &LaunchRecipe,
    overrides: &RecipeOverrides,
    catalog: &AgentReadinessResult,
    projects: &HashMap<ProjectId, Project>,
    hosts: &[HostEntry],
) -> Result<ResolvedRecipe, RecipeIssue> {
    let recipe = recipe.with_overrides(overrides);
    let kind = resolve_agent(&recipe.agent, catalog)?;
    let cwd = resolve_project(&recipe.project, projects)?;
    let host = resolve_host(recipe.host.as_deref(), hosts)?;
    if host.is_some()
        && recipe
            .worktree
            .as_ref()
            .is_some_and(|worktree| worktree.create)
    {
        return Err(RecipeIssue::RemoteWorktree);
    }
    let worktree = if host.is_some() {
        None
    } else {
        recipe.worktree.as_ref().and_then(|worktree| {
            worktree.create.then(|| WorktreeSpawn {
                create: true,
                branch: worktree.branch.as_deref().and_then(|template| {
                    let expanded = expand_branch_template(template, &recipe.name);
                    (!expanded.is_empty()).then_some(expanded)
                }),
            })
        })
    };
    let initial_prompt = recipe.initial_prompt.trim();
    Ok(ResolvedRecipe {
        kind,
        options: SpawnOptions {
            cwd: Some(cwd),
            worktree,
            title: recipe.title.clone(),
            initial_prompt: (!initial_prompt.is_empty()).then(|| initial_prompt.to_owned()),
            host,
            ..SpawnOptions::default()
        },
    })
}

fn resolve_agent(
    agent: &AgentKind,
    catalog: &AgentReadinessResult,
) -> Result<AgentKind, RecipeIssue> {
    let options = crate::agent_catalog::default_agent_options(catalog);
    match options.iter().find(|option| option.kind == *agent) {
        Some(option) if option.available => Ok(option.kind.clone()),
        Some(option) => Err(RecipeIssue::UnavailableAgent {
            id: agent.id().to_owned(),
            display_name: option.display_name.clone(),
            detail: option
                .unavailable_detail()
                .unwrap_or_else(|| crate::agent_catalog::missing_binary_label(&option.binary)),
        }),
        None => Err(RecipeIssue::MissingAgent {
            id: agent.id().to_owned(),
            display_name: crate::agent_catalog::display_name(agent, catalog),
        }),
    }
}

fn resolve_project(
    project: &RecipeProject,
    projects: &HashMap<ProjectId, Project>,
) -> Result<String, RecipeIssue> {
    match project {
        RecipeProject::Project { id } => projects
            .get(id)
            .map(|project| project.root.clone())
            .ok_or_else(|| RecipeIssue::MissingProject { id: id.clone() }),
        RecipeProject::Path { path } => {
            let path = path.trim();
            if path.is_empty() {
                Err(RecipeIssue::EmptyProject)
            } else {
                Ok(path.to_owned())
            }
        }
    }
}

fn resolve_host(host: Option<&str>, hosts: &[HostEntry]) -> Result<Option<String>, RecipeIssue> {
    let Some(id) = host.filter(|id| !id.is_empty()) else {
        return Ok(None);
    };
    if hosts.iter().any(|entry| entry.id == id) {
        Ok(Some(id.to_owned()))
    } else {
        Err(RecipeIssue::MissingHost { id: id.to_owned() })
    }
}

fn expand_branch_template(template: &str, recipe_name: &str) -> String {
    template
        .replace("{name}", &slugify(recipe_name))
        .trim()
        .to_owned()
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeus_proto::{AgentDescriptor, AgentReadinessItem};

    fn project(id: &str, name: &str, root: &str) -> (ProjectId, Project) {
        let id = ProjectId::new(id);
        (
            id.clone(),
            Project {
                id,
                root: root.to_owned(),
                name: name.to_owned(),
                pinned_order: None,
            },
        )
    }

    fn catalog(available: bool) -> AgentReadinessResult {
        AgentReadinessResult {
            agents: vec![AgentReadinessItem {
                kind: AgentKind::CLAUDE_CODE,
                binary: "claude".into(),
                path: available.then(|| "/bin/claude".into()),
                descriptor: Some(AgentDescriptor {
                    id: AgentKind::CLAUDE_CODE_ID.into(),
                    display_name: "Claude Code".into(),
                    first_class: true,
                    ..AgentDescriptor::default()
                }),
            }],
        }
    }

    fn recipe() -> LaunchRecipe {
        LaunchRecipe {
            id: "recipe-1".into(),
            name: "Review this PR".into(),
            agent: AgentKind::CLAUDE_CODE,
            project: RecipeProject::Project {
                id: ProjectId::new("zeus"),
            },
            host: None,
            worktree: Some(RecipeWorktree {
                create: true,
                branch: Some("review/{name}".into()),
            }),
            initial_prompt: "Review the open pull request.".into(),
            title: Some("PR review".into()),
        }
    }

    fn host(id: &str) -> HostEntry {
        HostEntry {
            id: id.into(),
            name: Some("Forge".into()),
            ssh: "user@forge".into(),
            default_cwd: Some("~/code".into()),
            node: None,
        }
    }

    #[test]
    fn resolve_uses_project_id_not_a_similar_name() {
        let mut projects = HashMap::new();
        let (id, zeus) = project("zeus", "zeus", "/work/zeus");
        projects.insert(id, zeus);
        let (other_id, other) = project("other", "zeus-old", "/work/other");
        projects.insert(other_id, other);

        let resolved = resolve_recipe(
            &recipe(),
            &RecipeOverrides::default(),
            &catalog(true),
            &projects,
            &[],
        )
        .expect("recipe");
        assert_eq!(resolved.kind, AgentKind::CLAUDE_CODE);
        assert_eq!(resolved.options.cwd.as_deref(), Some("/work/zeus"));
        assert_eq!(
            resolved.options.worktree,
            Some(WorktreeSpawn {
                create: true,
                branch: Some("review/review-this-pr".into()),
            })
        );
        assert_eq!(
            resolved.options.initial_prompt.as_deref(),
            Some("Review the open pull request.")
        );
        assert_eq!(resolved.options.title.as_deref(), Some("PR review"));
        assert_eq!(resolved.options.host, None);
    }

    #[test]
    fn missing_project_does_not_fall_back_to_another_root() {
        let mut projects = HashMap::new();
        let (id, other) = project("other", "zeus", "/work/other");
        projects.insert(id, other);
        let error = resolve_recipe(
            &recipe(),
            &RecipeOverrides::default(),
            &catalog(true),
            &projects,
            &[],
        )
        .expect_err("missing project");
        assert_eq!(
            error,
            RecipeIssue::MissingProject {
                id: ProjectId::new("zeus")
            }
        );
    }

    #[test]
    fn missing_and_unavailable_agents_are_diagnosable() {
        let mut projects = HashMap::new();
        let (id, zeus) = project("zeus", "zeus", "/work/zeus");
        projects.insert(id, zeus);

        let missing = LaunchRecipe {
            agent: AgentKind::new("amp"),
            ..recipe()
        };
        assert!(matches!(
            resolve_recipe(&missing, &RecipeOverrides::default(), &catalog(true), &projects, &[]),
            Err(RecipeIssue::MissingAgent { id, .. }) if id == "amp"
        ));

        assert!(matches!(
            resolve_recipe(&recipe(), &RecipeOverrides::default(), &catalog(false), &projects, &[]),
            Err(RecipeIssue::UnavailableAgent { id, .. }) if id == AgentKind::CLAUDE_CODE_ID
        ));
    }

    #[test]
    fn missing_host_does_not_fall_back_to_local() {
        let mut projects = HashMap::new();
        let (id, zeus) = project("zeus", "zeus", "/work/zeus");
        projects.insert(id, zeus);
        let remote = LaunchRecipe {
            host: Some("forge".into()),
            worktree: None,
            ..recipe()
        };
        assert_eq!(
            resolve_recipe(
                &remote,
                &RecipeOverrides::default(),
                &catalog(true),
                &projects,
                &[],
            ),
            Err(RecipeIssue::MissingHost { id: "forge".into() })
        );
    }

    #[test]
    fn remote_recipe_uses_the_explicit_path_and_rejects_worktrees() {
        let mut projects = HashMap::new();
        let (id, zeus) = project("zeus", "zeus", "/work/zeus");
        projects.insert(id, zeus);
        let remote = LaunchRecipe {
            project: RecipeProject::Path {
                path: "~/src/zeus".into(),
            },
            host: Some("forge".into()),
            worktree: None,
            ..recipe()
        };
        let resolved = resolve_recipe(
            &remote,
            &RecipeOverrides::default(),
            &catalog(true),
            &projects,
            &[host("forge")],
        )
        .expect("remote recipe");
        assert_eq!(resolved.options.host.as_deref(), Some("forge"));
        assert_eq!(resolved.options.cwd.as_deref(), Some("~/src/zeus"));
        assert_eq!(resolved.options.worktree, None);

        let with_worktree = LaunchRecipe {
            worktree: Some(RecipeWorktree {
                create: true,
                branch: None,
            }),
            ..remote
        };
        assert_eq!(
            resolve_recipe(
                &with_worktree,
                &RecipeOverrides::default(),
                &catalog(true),
                &projects,
                &[host("forge")],
            ),
            Err(RecipeIssue::RemoteWorktree)
        );
    }

    #[test]
    fn overrides_do_not_require_mutating_the_stored_recipe() {
        let mut projects = HashMap::new();
        let (id, zeus) = project("zeus", "zeus", "/work/zeus");
        projects.insert(id, zeus);
        let stored = recipe();
        let resolved = resolve_recipe(
            &stored,
            &RecipeOverrides {
                initial_prompt: Some("Just this once.".into()),
                title: Some(None),
                worktree: Some(None),
                ..RecipeOverrides::default()
            },
            &catalog(true),
            &projects,
            &[],
        )
        .expect("override");
        assert_eq!(
            resolved.options.initial_prompt.as_deref(),
            Some("Just this once.")
        );
        assert_eq!(resolved.options.title, None);
        assert_eq!(resolved.options.worktree, None);
        assert_eq!(stored.initial_prompt, "Review the open pull request.");
        assert_eq!(stored.title.as_deref(), Some("PR review"));
        assert!(stored.worktree.is_some());
    }

    #[test]
    fn path_policy_does_not_upgrade_to_a_similar_project() {
        let mut projects = HashMap::new();
        let (id, zeus) = project("zeus", "zeus", "/work/zeus");
        projects.insert(id, zeus);
        let path_recipe = LaunchRecipe {
            project: RecipeProject::Path {
                path: "/tmp/scratch".into(),
            },
            worktree: None,
            ..recipe()
        };
        let resolved = resolve_recipe(
            &path_recipe,
            &RecipeOverrides::default(),
            &catalog(true),
            &projects,
            &[],
        )
        .expect("path");
        assert_eq!(resolved.options.cwd.as_deref(), Some("/tmp/scratch"));
    }

    #[test]
    fn malformed_and_stale_recipe_arrays_are_dropped_not_fatal() {
        let parsed = parse_launch_recipes(serde_json::json!([
            {"id": "", "name": "broken", "agent": "claude-code", "project": {"kind": "path", "path": "/x"}},
            {"name": "no-id"},
            {
                "id": "recipe-ok",
                "name": "Fix tests",
                "agent": "claudeCode",
                "project": {"kind": "path", "path": "/work/zeus"},
                "initialPrompt": "fix it"
            },
            12,
            {"id": "recipe-bad-project", "name": "Bad", "agent": "codex", "project": {"kind": "unknown"}}
        ]));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "recipe-ok");
        assert_eq!(parsed[0].agent, AgentKind::CLAUDE_CODE);
        assert_eq!(
            parsed[0].project,
            RecipeProject::Path {
                path: "/work/zeus".into()
            }
        );
        assert!(parse_launch_recipes(serde_json::json!({"not": "an array"})).is_empty());
    }

    #[test]
    fn recipe_project_prefers_stable_ids_when_the_root_matches() {
        let mut projects = HashMap::new();
        let (id, zeus) = project("zeus", "Zeus", "/work/zeus");
        projects.insert(id.clone(), zeus);
        assert_eq!(
            recipe_project_for("/work/zeus", &projects),
            RecipeProject::Project { id }
        );
        assert_eq!(
            recipe_project_for("/tmp/other", &projects),
            RecipeProject::Path {
                path: "/tmp/other".into()
            }
        );
        assert_eq!(
            launch_project_ref("/work/zeus", true, &projects),
            RecipeProject::Path {
                path: "/work/zeus".into()
            }
        );
        assert_eq!(
            launch_project_ref("  ", true, &projects),
            RecipeProject::Path { path: "~".into() }
        );
    }
}
