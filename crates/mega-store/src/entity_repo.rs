use mega_memory::{EntityId, EntityType, SourceEventId};
use rusqlite::OptionalExtension;
use uuid::Uuid;

use crate::operations_repo::{MemoryEventType, record_event_tx};
use crate::{MemoryStore, StoreError};

#[derive(Clone, Debug, PartialEq)]
pub struct EntityRecord {
    pub id: EntityId,
    pub entity_type: EntityType,
    pub canonical_name: String,
    pub identity_confidence: f32,
    pub revision: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EntityResolution {
    Resolved(EntityRecord),
    Ambiguous(Vec<EntityRecord>),
    Unresolved,
}

impl MemoryStore {
    pub fn create_entity(
        &self,
        entity_type: EntityType,
        canonical_name: &str,
        confidence: f32,
        source_event_id: Option<&SourceEventId>,
        now_ms: i64,
    ) -> Result<EntityRecord, StoreError> {
        let normalized = normalize_name(canonical_name)?;
        validate_confidence(confidence)?;
        if let Some(source_id) = source_event_id {
            let exists: bool = self.connection().query_row(
                "SELECT EXISTS(SELECT 1 FROM source_events WHERE id=?1)",
                [source_id.as_str()],
                |row| row.get(0),
            )?;
            if !exists {
                return Err(StoreError::InvalidInput("entity source does not exist"));
            }
        }
        let id = EntityId::new(Uuid::now_v7().to_string());
        self.connection().execute(
            "INSERT INTO entities(id, entity_type, canonical_name, normalized_name,
             identity_confidence, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            rusqlite::params![
                id.as_str(),
                entity_type_str(entity_type),
                canonical_name.trim(),
                normalized,
                confidence,
                now_ms
            ],
        )?;
        self.connection().execute(
            "INSERT INTO entity_aliases(entity_id, alias, normalized_alias, source_event_id, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id.as_str(), canonical_name.trim(), normalized, source_event_id.map(SourceEventId::as_str), confidence],
        )?;
        Ok(EntityRecord {
            id,
            entity_type,
            canonical_name: canonical_name.trim().to_owned(),
            identity_confidence: confidence,
            revision: 1,
        })
    }

    pub fn resolve_entity(
        &self,
        entity_type: EntityType,
        mention: &str,
    ) -> Result<EntityResolution, StoreError> {
        let normalized = normalize_name(mention)?;
        let mut stmt = self.connection().prepare(
            "SELECT DISTINCT e.id, e.entity_type, e.canonical_name, e.identity_confidence, e.revision
             FROM entities e LEFT JOIN entity_aliases ea ON ea.entity_id=e.id
             WHERE e.entity_type=?1 AND (e.normalized_name=?2 OR ea.normalized_alias=?2)
             ORDER BY e.identity_confidence DESC, e.id LIMIT 10",
        )?;
        let matches = stmt
            .query_map(
                rusqlite::params![entity_type_str(entity_type), normalized],
                entity_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(match matches.len() {
            0 => EntityResolution::Unresolved,
            1 => EntityResolution::Resolved(
                matches
                    .into_iter()
                    .next()
                    .ok_or(StoreError::Invariant("entity result disappeared"))?,
            ),
            _ => EntityResolution::Ambiguous(matches),
        })
    }

    pub fn merge_entities(
        &mut self,
        primary_id: &EntityId,
        primary_revision: u32,
        duplicate_id: &EntityId,
        duplicate_revision: u32,
        correlation_id: &str,
        now_ms: i64,
    ) -> Result<u32, StoreError> {
        if primary_id == duplicate_id {
            return Err(StoreError::InvalidInput(
                "cannot merge an entity into itself",
            ));
        }
        let tx = self.connection_mut().transaction()?;
        let primary = entity_revision_and_type(&tx, primary_id)?;
        let duplicate = entity_revision_and_type(&tx, duplicate_id)?;
        if primary.0 != primary_revision {
            return Err(StoreError::RevisionConflict {
                expected: primary_revision,
                actual: primary.0,
            });
        }
        if duplicate.0 != duplicate_revision {
            return Err(StoreError::RevisionConflict {
                expected: duplicate_revision,
                actual: duplicate.0,
            });
        }
        if primary.1 != duplicate.1 {
            return Err(StoreError::InvalidInput(
                "entities of different types cannot be merged",
            ));
        }
        tx.execute(
            "INSERT OR IGNORE INTO entity_aliases(entity_id, alias, normalized_alias, source_event_id, confidence)
             SELECT ?1, alias, normalized_alias, source_event_id, confidence FROM entity_aliases WHERE entity_id=?2",
            rusqlite::params![primary_id.as_str(), duplicate_id.as_str()],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO memory_entities(memory_id, entity_id, role)
             SELECT memory_id, ?1, role FROM memory_entities WHERE entity_id=?2",
            rusqlite::params![primary_id.as_str(), duplicate_id.as_str()],
        )?;
        tx.execute("DELETE FROM entities WHERE id=?1", [duplicate_id.as_str()])?;
        tx.execute(
            "UPDATE entities SET revision=revision+1, updated_at=?2,
             identity_confidence=MAX(identity_confidence, ?3) WHERE id=?1 AND revision=?4",
            rusqlite::params![primary_id.as_str(), now_ms, duplicate.2, primary_revision],
        )?;
        tx.execute("DELETE FROM memory_profiles WHERE projection_type='entity' AND projection_key IN (?1, ?2)",
            rusqlite::params![primary_id.as_str(), duplicate_id.as_str()])?;
        record_event_tx(
            &tx,
            MemoryEventType::EntityMerged,
            correlation_id,
            None,
            None,
            Some("merged"),
            now_ms,
        )?;
        tx.commit()?;
        Ok(primary_revision + 1)
    }
}

fn normalize_name(value: &str) -> Result<String, StoreError> {
    let normalized = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if !(1..=200).contains(&normalized.chars().count()) {
        return Err(StoreError::InvalidInput(
            "entity name must be 1..=200 characters",
        ));
    }
    Ok(normalized)
}

fn validate_confidence(value: f32) -> Result<(), StoreError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(StoreError::InvalidInput(
            "entity confidence must be in [0, 1]",
        ))
    }
}

fn entity_revision_and_type(
    tx: &rusqlite::Transaction<'_>,
    id: &EntityId,
) -> Result<(u32, String, f32), StoreError> {
    tx.query_row(
        "SELECT revision, entity_type, identity_confidence FROM entities WHERE id=?1",
        [id.as_str()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()?
    .ok_or(StoreError::NotFound)
}

fn entity_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EntityRecord> {
    let entity_type: String = row.get(1)?;
    Ok(EntityRecord {
        id: EntityId::new(row.get::<_, String>(0)?),
        entity_type: parse_entity_type_sql(&entity_type)?,
        canonical_name: row.get(2)?,
        identity_confidence: row.get(3)?,
        revision: row.get(4)?,
    })
}

const fn entity_type_str(value: EntityType) -> &'static str {
    match value {
        EntityType::Person => "person",
        EntityType::Organization => "organization",
        EntityType::Project => "project",
        EntityType::Place => "place",
        EntityType::Product => "product",
    }
}

fn parse_entity_type_sql(value: &str) -> rusqlite::Result<EntityType> {
    match value {
        "person" => Ok(EntityType::Person),
        "organization" => Ok(EntityType::Organization),
        "project" => Ok(EntityType::Project),
        "place" => Ok(EntityType::Place),
        "product" => Ok(EntityType::Product),
        _ => Err(rusqlite::Error::InvalidColumnType(
            1,
            "entity_type".into(),
            rusqlite::types::Type::Text,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambiguity_is_preserved_until_entities_are_explicitly_merged() {
        let mut store = MemoryStore::in_memory().unwrap();
        let first = store
            .create_entity(EntityType::Person, "Alice", 0.9, None, 1)
            .unwrap();
        let second = store
            .create_entity(EntityType::Person, "Alice", 0.8, None, 2)
            .unwrap();
        assert!(
            matches!(store.resolve_entity(EntityType::Person, "alice").unwrap(), EntityResolution::Ambiguous(values) if values.len() == 2)
        );
        store
            .merge_entities(&first.id, 1, &second.id, 1, "merge-alice", 3)
            .unwrap();
        assert!(
            matches!(store.resolve_entity(EntityType::Person, "Alice").unwrap(), EntityResolution::Resolved(value) if value.id == first.id && value.revision == 2)
        );
    }
}
