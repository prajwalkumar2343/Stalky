use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use mega_permissions::PermissionCapability;
use serde::{Deserialize, Serialize};

const PREFERENCES_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountMode {
    #[default]
    Local,
    Google,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct StoredPreferences {
    version: u8,
    onboarding_completed: bool,
    account_mode: Option<AccountMode>,
    permission_requests: BTreeMap<PermissionCapability, PermissionRequestMetadata>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PermissionRequestMetadata {
    count: u32,
    last_requested_at: u64,
}

#[derive(Debug)]
pub struct PreferenceStore {
    path: PathBuf,
    values: Mutex<StoredPreferences>,
}

impl PreferenceStore {
    pub fn new(path: PathBuf) -> Self {
        let values = fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .filter(|values: &StoredPreferences| values.version <= PREFERENCES_VERSION)
            .unwrap_or_else(|| StoredPreferences {
                version: PREFERENCES_VERSION,
                ..StoredPreferences::default()
            });
        Self {
            path,
            values: Mutex::new(values),
        }
    }

    pub fn onboarding_state(&self) -> OnboardingState {
        let values = self
            .values
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        OnboardingState {
            completed: values.onboarding_completed,
            account_mode: values.account_mode,
        }
    }

    pub fn has_requested(&self, capability: PermissionCapability) -> bool {
        self.values
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .permission_requests
            .get(&capability)
            .is_some_and(|metadata| metadata.count > 0)
    }

    pub fn complete_onboarding(&self, account_mode: AccountMode) -> Result<(), String> {
        self.update(|values| {
            values.onboarding_completed = true;
            values.account_mode = Some(account_mode);
        })
    }

    pub fn reset_onboarding(&self) -> Result<(), String> {
        self.update(|values| {
            values.onboarding_completed = false;
            values.account_mode = None;
        })
    }

    pub fn set_account_mode(&self, account_mode: AccountMode) -> Result<(), String> {
        self.update(|values| values.account_mode = Some(account_mode))
    }

    pub fn record_permission_request(
        &self,
        capability: PermissionCapability,
    ) -> Result<(), String> {
        self.update(|values| {
            let metadata = values.permission_requests.entry(capability).or_default();
            metadata.count = metadata.count.saturating_add(1);
            metadata.last_requested_at = now_seconds();
        })
    }

    fn update(&self, update: impl FnOnce(&mut StoredPreferences)) -> Result<(), String> {
        let mut values = self
            .values
            .lock()
            .map_err(|_| "Local preference storage is unavailable.".to_owned())?;
        let mut next = values.clone();
        update(&mut next);
        next.version = PREFERENCES_VERSION;
        let encoded = serde_json::to_vec_pretty(&next)
            .map_err(|error| format!("could not encode local preferences: {error}"))?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "Local preference path has no parent.".to_owned())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create local preference directory: {error}"))?;
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, encoded)
            .map_err(|error| format!("could not write local preferences: {error}"))?;
        fs::rename(&temporary, &self.path)
            .map_err(|error| format!("could not commit local preferences: {error}"))?;
        *values = next;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingState {
    pub completed: bool,
    pub account_mode: Option<AccountMode>,
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use mega_permissions::PermissionCapability;

    use super::{AccountMode, PreferenceStore};

    fn temporary_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "stalky-preferences-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn only_onboarding_choice_and_request_metadata_are_persisted() {
        let path = temporary_path();
        let store = PreferenceStore::new(path.clone());
        store.complete_onboarding(AccountMode::Local).unwrap();
        store
            .record_permission_request(PermissionCapability::Accessibility)
            .unwrap();

        let reloaded = PreferenceStore::new(path.clone());
        assert_eq!(
            reloaded.onboarding_state().account_mode,
            Some(AccountMode::Local)
        );
        assert!(reloaded.has_requested(PermissionCapability::Accessibility));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_corrupt_and_future_preferences_recover_to_local_defaults() {
        for contents in [
            Some("{not-json"),
            Some(r#"{"version":99,"onboarding_completed":true}"#),
            None,
        ] {
            let path = temporary_path();
            if let Some(contents) = contents {
                std::fs::write(&path, contents).unwrap();
            }
            let store = PreferenceStore::new(path.clone());
            assert!(!store.onboarding_state().completed);
            assert_eq!(store.onboarding_state().account_mode, None);
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn failed_persistence_does_not_change_the_in_memory_snapshot() {
        let path = temporary_path();
        std::fs::create_dir_all(&path).unwrap();
        let store = PreferenceStore::new(path.clone());
        assert!(store.complete_onboarding(AccountMode::Local).is_err());
        assert!(!store.onboarding_state().completed);
        let _ = std::fs::remove_file(path.with_extension("json.tmp"));
        std::fs::remove_dir_all(path).unwrap();
    }
}
