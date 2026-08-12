use serde::{Deserialize, Serialize};

use crate::{AppId, EntityId, Memory, MemoryId, MemoryStatus, MemoryType, ScopeType};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileProjectionType {
    Global,
    App,
    Project,
    Category,
    Entity,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProfileItem {
    pub memory_id: MemoryId,
    pub content: String,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProfileProjection {
    pub projection_type: ProfileProjectionType,
    pub projection_key: String,
    pub stable: Vec<ProfileItem>,
    pub current: Vec<ProfileItem>,
    pub source_revision: u64,
    pub generated_at_ms: i64,
}

/// Builds a deterministic, replaceable profile projection from active memory.
pub fn build_profile(
    projection_type: ProfileProjectionType,
    projection_key: impl Into<String>,
    memories: &[Memory],
    source_revision: u64,
    generated_at_ms: i64,
) -> ProfileProjection {
    let projection_key = projection_key.into();
    let mut included: Vec<_> = memories
        .iter()
        .filter(|memory| memory.status == MemoryStatus::Active)
        .filter(|memory| belongs_to_projection(memory, projection_type, &projection_key))
        .collect();
    included.sort_by(|left, right| {
        right
            .importance
            .total_cmp(&left.importance)
            .then_with(|| right.confidence.total_cmp(&left.confidence))
            .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut stable = Vec::new();
    let mut current = Vec::new();
    for memory in included {
        let item = ProfileItem {
            memory_id: memory.id.clone(),
            content: memory.display_content.clone(),
            updated_at_ms: memory.updated_at_ms,
        };
        match memory.memory_type {
            MemoryType::Preference | MemoryType::Fact | MemoryType::Procedure => stable.push(item),
            MemoryType::Decision | MemoryType::Episode | MemoryType::Task => current.push(item),
        }
    }
    ProfileProjection {
        projection_type,
        projection_key,
        stable,
        current,
        source_revision,
        generated_at_ms,
    }
}

/// Whether the entity has enough active memories for its required projection.
pub fn entity_profile_is_eligible(entity_id: &EntityId, memories: &[Memory]) -> bool {
    memories
        .iter()
        .filter(|memory| memory.status == MemoryStatus::Active)
        .filter(|memory| {
            memory
                .entities
                .iter()
                .any(|entity| &entity.entity_id == entity_id)
        })
        .take(3)
        .count()
        >= 3
}

fn belongs_to_projection(
    memory: &Memory,
    projection_type: ProfileProjectionType,
    key: &str,
) -> bool {
    match projection_type {
        ProfileProjectionType::Global => memory.scope.scope_type == ScopeType::Global,
        ProfileProjectionType::App => {
            let app_id = AppId::new(key);
            memory.applicable_app_ids.contains(&app_id)
                || memory.scope.scope_type == ScopeType::App && memory.scope.scope_key == key
        }
        ProfileProjectionType::Project => {
            memory.scope.scope_type == ScopeType::Project && memory.scope.scope_key == key
        }
        ProfileProjectionType::Category => memory.category_slugs.iter().any(|slug| {
            slug == key
                || slug
                    .strip_prefix(key)
                    .is_some_and(|suffix| suffix.starts_with('.'))
        }),
        ProfileProjectionType::Entity => memory
            .entities
            .iter()
            .any(|entity| entity.entity_id.as_str() == key),
    }
}

#[cfg(test)]
mod tests {
    use crate::{AssertionMode, MemoryScope, ScopeId, Sensitivity};

    use super::*;

    fn memory(id: &str, memory_type: MemoryType, category: &str) -> Memory {
        Memory {
            id: MemoryId::from(id),
            normalized_content: id.into(),
            display_content: id.into(),
            memory_type,
            assertion_mode: AssertionMode::Explicit,
            status: MemoryStatus::Active,
            scope: MemoryScope {
                id: ScopeId::from("global"),
                scope_type: ScopeType::Global,
                scope_key: "global".into(),
                display_name: "Global".into(),
            },
            source_app_ids: vec![],
            applicable_app_ids: vec![],
            category_slugs: vec![category.into()],
            entities: vec![],
            source_event_ids: vec![],
            importance: 0.5,
            confidence: 1.0,
            sensitivity: Sensitivity::Private,
            valid_from_ms: None,
            valid_until_ms: None,
            created_at_ms: 0,
            updated_at_ms: 1,
            revision: 1,
        }
    }

    #[test]
    fn category_projection_includes_descendants_and_splits_stability() {
        let projection = build_profile(
            ProfileProjectionType::Category,
            "choices.design",
            &[
                memory(
                    "preference",
                    MemoryType::Preference,
                    "choices.design.typography",
                ),
                memory("task", MemoryType::Task, "choices.design"),
                memory("food", MemoryType::Preference, "choices.food"),
            ],
            4,
            10,
        );
        assert_eq!(
            projection
                .stable
                .iter()
                .map(|item| item.content.as_str())
                .collect::<Vec<_>>(),
            ["preference"]
        );
        assert_eq!(
            projection
                .current
                .iter()
                .map(|item| item.content.as_str())
                .collect::<Vec<_>>(),
            ["task"]
        );
    }
}
