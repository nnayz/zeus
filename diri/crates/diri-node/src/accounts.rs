use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use diri_proto::{
    AccountCatalogResult, AccountInstallation, AccountProfile, AccountSetDefaultParams,
    AccountUpsertParams, InstallationStatus, ProviderKind,
};
use serde::{Deserialize, Serialize};

use crate::config::{NodePaths, atomic_json, set_owner_directory};
use crate::error::{NodeError, NodeResult};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct AccountsFile {
    #[serde(default)]
    profiles: Vec<AccountProfile>,
    #[serde(default)]
    defaults: BTreeMap<ProviderKind, String>,
}

pub struct AccountStore {
    node_id: String,
    paths: NodePaths,
    data: AccountsFile,
}

impl AccountStore {
    pub fn load(paths: NodePaths, node_id: impl Into<String>) -> NodeResult<Self> {
        let data = if paths.accounts.exists() {
            serde_json::from_slice(&fs::read(&paths.accounts)?)?
        } else {
            AccountsFile::default()
        };
        let store = Self {
            node_id: node_id.into(),
            paths,
            data,
        };
        store.validate()?;
        Ok(store)
    }

    pub fn catalog(&self) -> AccountCatalogResult {
        AccountCatalogResult {
            profiles: self.data.profiles.clone(),
            installations: self
                .data
                .profiles
                .iter()
                .map(|profile| self.installation(profile, InstallationStatus::SignedOut))
                .collect(),
            defaults: self.data.defaults.clone(),
        }
    }

    pub fn profiles(&self) -> &[AccountProfile] {
        &self.data.profiles
    }

    pub fn profile(&self, id: &str) -> NodeResult<&AccountProfile> {
        self.data
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| NodeError::NotFound(format!("account profile `{id}`")))
    }

    pub fn config_home(&self, profile: &AccountProfile) -> PathBuf {
        self.paths
            .accounts_root
            .join(profile.provider.as_str())
            .join(&profile.id)
    }

    pub fn upsert(&mut self, params: AccountUpsertParams) -> NodeResult<AccountProfile> {
        validate_profile_id(&params.id)?;
        if params.label.trim().is_empty() {
            return Err(NodeError::BadRequest(
                "account label cannot be empty".into(),
            ));
        }
        let now = now_seconds();
        let profile = if let Some(existing) = self
            .data
            .profiles
            .iter_mut()
            .find(|profile| profile.id == params.id)
        {
            if existing.provider != params.provider {
                return Err(NodeError::Conflict(
                    "an account profile cannot change provider".into(),
                ));
            }
            existing.label = params.label;
            existing.email = params.email;
            existing.organization = params.organization;
            existing.tags = params.tags;
            existing.updated_at = now;
            existing.clone()
        } else {
            let profile = AccountProfile {
                id: params.id,
                provider: params.provider,
                label: params.label,
                email: params.email,
                organization: params.organization,
                tags: params.tags,
                created_at: now,
                updated_at: now,
            };
            self.data.profiles.push(profile.clone());
            profile
        };
        let config_home = self.config_home(&profile);
        fs::create_dir_all(&config_home)?;
        set_owner_directory(&config_home)?;
        self.save()?;
        Ok(profile)
    }

    pub fn set_default(&mut self, params: AccountSetDefaultParams) -> NodeResult<()> {
        let profile = self.profile(&params.profile_id)?;
        if profile.provider != params.provider {
            return Err(NodeError::BadRequest(
                "default provider does not match the profile".into(),
            ));
        }
        self.data
            .defaults
            .insert(params.provider, params.profile_id);
        self.save()
    }

    pub fn installation(
        &self,
        profile: &AccountProfile,
        status: InstallationStatus,
    ) -> AccountInstallation {
        AccountInstallation {
            profile_id: profile.id.clone(),
            provider: profile.provider,
            node_id: self.node_id.clone(),
            status,
            config_home: self.config_home(profile).to_string_lossy().into_owned(),
            identity: profile.email.clone(),
            plan: None,
            last_error: None,
            checked_at: None,
        }
    }

    fn save(&self) -> NodeResult<()> {
        atomic_json(&self.paths.accounts, &self.data)?;
        Ok(())
    }

    fn validate(&self) -> NodeResult<()> {
        for profile in &self.data.profiles {
            validate_profile_id(&profile.id)?;
        }
        for (&provider, profile_id) in &self.data.defaults {
            let Some(profile) = self
                .data
                .profiles
                .iter()
                .find(|item| item.id == *profile_id)
            else {
                return Err(NodeError::Protocol(format!(
                    "default profile `{profile_id}` is missing"
                )));
            };
            if profile.provider != provider {
                return Err(NodeError::Protocol(format!(
                    "default profile `{profile_id}` has the wrong provider"
                )));
            }
        }
        Ok(())
    }
}

pub fn validate_profile_id(id: &str) -> NodeResult<()> {
    if id.is_empty()
        || id.len() > 80
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || matches!(id, "." | "..")
    {
        return Err(NodeError::BadRequest(
            "profile id must use 1-80 letters, numbers, dots, dashes, or underscores".into(),
        ));
    }
    Ok(())
}

pub fn profile_environment(provider: ProviderKind, config_home: &Path) -> (&'static str, String) {
    let variable = match provider {
        ProviderKind::Claude => "CLAUDE_CONFIG_DIR",
        ProviderKind::Codex => "CODEX_HOME",
    };
    (variable, config_home.to_string_lossy().into_owned())
}

pub(crate) fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_metadata_and_installations_are_node_local() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let paths = NodePaths::for_root(directory.path().join("node"));
        paths.create_layout().expect("layout");
        let mut store = AccountStore::load(paths.clone(), "forge").expect("store");
        let profile = store
            .upsert(AccountUpsertParams {
                id: "work".into(),
                provider: ProviderKind::Codex,
                label: "Work".into(),
                email: Some("person@example.com".into()),
                organization: None,
                tags: vec!["company".into()],
            })
            .expect("upsert");
        store
            .set_default(AccountSetDefaultParams {
                provider: ProviderKind::Codex,
                profile_id: profile.id.clone(),
            })
            .expect("default");
        let catalog = store.catalog();
        assert_eq!(catalog.profiles.len(), 1);
        assert_eq!(catalog.installations[0].node_id, "forge");
        assert!(Path::new(&catalog.installations[0].config_home).is_dir());
        let raw = fs::read_to_string(paths.accounts).expect("accounts file");
        assert!(!raw.contains("token"));
        assert!(!raw.contains("secret"));
    }

    #[test]
    fn profile_ids_cannot_escape_the_accounts_root() {
        for invalid in ["", "..", "a/b", "hello world"] {
            assert!(validate_profile_id(invalid).is_err(), "{invalid}");
        }
        assert!(validate_profile_id("personal.codex-2").is_ok());
    }
}
