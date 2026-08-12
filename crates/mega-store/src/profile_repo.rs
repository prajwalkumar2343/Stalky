use serde_json::Value;
use uuid::Uuid;

use crate::operations_repo::{MemoryEventType, record_event_tx};
use crate::{MemoryStore, StoreError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileType {
    Global,
    App,
    Project,
    Category,
    Entity,
}

impl ProfileType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::App => "app",
            Self::Project => "project",
            Self::Category => "category",
            Self::Entity => "entity",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProfileRecord {
    pub profile_type: ProfileType,
    pub key: String,
    pub stable: Value,
    pub current: Value,
    pub source_revision: u64,
    pub generated_at: i64,
}

impl MemoryStore {
    pub fn put_profile(
        &mut self,
        profile: &ProfileRecord,
        correlation_id: &str,
    ) -> Result<bool, StoreError> {
        validate_profile(profile)?;
        let stable = canonical_json(&profile.stable)?;
        let current = canonical_json(&profile.current)?;
        let source_revision = i64::try_from(profile.source_revision)
            .map_err(|_| StoreError::InvalidInput("profile revision overflow"))?;
        let tx = self.connection_mut().transaction()?;
        let changed = tx.execute(
            "INSERT INTO memory_profiles(id, projection_type, projection_key, stable_json,
             current_json, source_revision, generated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(projection_type, projection_key) DO UPDATE SET stable_json=excluded.stable_json,
               current_json=excluded.current_json, source_revision=excluded.source_revision,
               generated_at=excluded.generated_at
             WHERE excluded.source_revision>=memory_profiles.source_revision",
            rusqlite::params![Uuid::now_v7().to_string(), profile.profile_type.as_str(), profile.key,
                stable, current, source_revision, profile.generated_at],
        )?;
        if changed == 1 {
            record_event_tx(
                &tx,
                MemoryEventType::ProfileRegenerated,
                correlation_id,
                None,
                None,
                Some(profile.profile_type.as_str()),
                profile.generated_at,
            )?;
        }
        tx.commit()?;
        Ok(changed == 1)
    }

    pub fn get_profile(
        &self,
        profile_type: ProfileType,
        key: &str,
    ) -> Result<Option<ProfileRecord>, StoreError> {
        use rusqlite::OptionalExtension;
        let raw = self.connection().query_row(
            "SELECT stable_json, current_json, source_revision, generated_at FROM memory_profiles
             WHERE projection_type=?1 AND projection_key=?2",
            rusqlite::params![profile_type.as_str(), key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)),
        ).optional()?;
        raw.map(|(stable, current, source_revision, generated_at)| {
            Ok(ProfileRecord {
                profile_type,
                key: key.to_owned(),
                stable: serde_json::from_str(&stable)?,
                current: serde_json::from_str(&current)?,
                source_revision: u64::try_from(source_revision)
                    .map_err(|_| StoreError::Invariant("stored profile revision is invalid"))?,
                generated_at,
            })
        })
        .transpose()
    }
}

fn validate_profile(profile: &ProfileRecord) -> Result<(), StoreError> {
    if profile.key.trim().is_empty() || profile.key.chars().count() > 256 {
        return Err(StoreError::InvalidInput(
            "profile key must be 1..=256 characters",
        ));
    }
    if profile.stable.is_null() || profile.current.is_null() {
        return Err(StoreError::InvalidInput("profile JSON cannot be null"));
    }
    Ok(())
}

fn canonical_json(value: &Value) -> Result<String, StoreError> {
    let encoded = serde_json::to_string(value)?;
    if encoded.len() > 256 * 1024 {
        return Err(StoreError::InvalidInput("profile JSON exceeds 256 KiB"));
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stale_profile_generation_cannot_replace_a_newer_projection() {
        let mut store = MemoryStore::in_memory().unwrap();
        let mut profile = ProfileRecord {
            profile_type: ProfileType::Global,
            key: "global".into(),
            stable: json!(["new"]),
            current: json!([]),
            source_revision: 2,
            generated_at: 20,
        };
        assert!(store.put_profile(&profile, "profile-2").unwrap());
        profile.stable = json!(["stale"]);
        profile.source_revision = 1;
        profile.generated_at = 10;
        assert!(!store.put_profile(&profile, "profile-1").unwrap());
        assert_eq!(
            store
                .get_profile(ProfileType::Global, "global")
                .unwrap()
                .unwrap()
                .stable,
            json!(["new"])
        );
    }
}
