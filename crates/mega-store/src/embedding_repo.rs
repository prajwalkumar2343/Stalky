use mega_memory::{MemoryId, exact_cosine_similarity};

use crate::{MemoryStore, StoreError};

#[derive(Clone, Debug)]
pub struct EmbeddingInput {
    pub memory_id: MemoryId,
    pub provider: String,
    pub model: String,
    pub values: Vec<f32>,
    pub content_hash: [u8; 32],
    pub embedded_at: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddingMatch {
    pub memory_id: MemoryId,
    pub similarity: f32,
}

impl MemoryStore {
    pub fn upsert_embedding(&self, input: &EmbeddingInput) -> Result<(), StoreError> {
        validate_model(&input.provider, &input.model)?;
        let normalized = normalize_vector(&input.values)?;
        let blob = encode_vector(&normalized);
        let changed = self.connection().execute(
            "INSERT INTO memory_embeddings(memory_id, provider, model, dimensions, vector_f32,
             content_hash, embedded_at) SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7
             WHERE EXISTS(SELECT 1 FROM memories WHERE id=?1 AND status='active')
             ON CONFLICT(memory_id) DO UPDATE SET provider=excluded.provider, model=excluded.model,
               dimensions=excluded.dimensions, vector_f32=excluded.vector_f32,
               content_hash=excluded.content_hash, embedded_at=excluded.embedded_at",
            rusqlite::params![
                input.memory_id.as_str(),
                input.provider,
                input.model,
                i64::try_from(normalized.len())
                    .map_err(|_| StoreError::InvalidInput("embedding dimension overflow"))?,
                blob,
                input.content_hash.as_slice(),
                input.embedded_at
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::InvalidInput("embedding target is not active"));
        }
        self.connection().execute(
            "DELETE FROM projection_jobs WHERE projection_type='embedding' AND projection_key=?1
             AND source_revision<=(SELECT revision FROM memories WHERE id=?1)",
            [input.memory_id.as_str()],
        )?;
        Ok(())
    }

    pub fn nearest_embeddings(
        &self,
        query: &[f32],
        provider: &str,
        model: &str,
        eligible_memory_ids: &[MemoryId],
        limit: usize,
    ) -> Result<Vec<EmbeddingMatch>, StoreError> {
        validate_model(provider, model)?;
        if !(1..=200).contains(&limit) {
            return Err(StoreError::InvalidInput(
                "embedding result limit must be 1..=200",
            ));
        }
        let query = normalize_vector(query)?;
        let mut matches = Vec::new();
        let mut stmt = self.connection().prepare(
            "SELECT dimensions, vector_f32 FROM memory_embeddings e
             JOIN memories m ON m.id=e.memory_id
             WHERE e.memory_id=?1 AND e.provider=?2 AND e.model=?3 AND m.status='active'",
        )?;
        for memory_id in eligible_memory_ids.iter().take(10_000) {
            let stored = stmt.query_row(
                rusqlite::params![memory_id.as_str(), provider, model],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            );
            let (dimensions, bytes) = match stored {
                Ok(value) => value,
                Err(rusqlite::Error::QueryReturnedNoRows) => continue,
                Err(error) => return Err(error.into()),
            };
            let dimensions = usize::try_from(dimensions)
                .map_err(|_| StoreError::Invariant("stored embedding dimensions are invalid"))?;
            if dimensions != query.len() {
                continue;
            }
            let vector = decode_vector(&bytes, dimensions)?;
            let similarity = exact_cosine_similarity(&query, &vector)
                .map_err(|_| StoreError::Invariant("stored embedding is invalid"))?;
            matches.push(EmbeddingMatch {
                memory_id: memory_id.clone(),
                similarity,
            });
        }
        matches.sort_by(|left, right| {
            right
                .similarity
                .total_cmp(&left.similarity)
                .then_with(|| left.memory_id.cmp(&right.memory_id))
        });
        matches.truncate(limit);
        Ok(matches)
    }
}

fn validate_model(provider: &str, model: &str) -> Result<(), StoreError> {
    if provider.trim().is_empty()
        || provider.chars().count() > 128
        || model.trim().is_empty()
        || model.chars().count() > 128
    {
        return Err(StoreError::InvalidInput(
            "invalid embedding provider metadata",
        ));
    }
    Ok(())
}

fn normalize_vector(values: &[f32]) -> Result<Vec<f32>, StoreError> {
    if values.is_empty() || values.len() > 16_384 || values.iter().any(|value| !value.is_finite()) {
        return Err(StoreError::InvalidInput(
            "embedding vector is empty, oversized, or non-finite",
        ));
    }
    let magnitude = values
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    if magnitude == 0.0 {
        return Err(StoreError::InvalidInput(
            "embedding vector has zero magnitude",
        ));
    }
    Ok(values
        .iter()
        .map(|value| (f64::from(*value) / magnitude) as f32)
        .collect())
}

fn encode_vector(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn decode_vector(bytes: &[u8], dimensions: usize) -> Result<Vec<f32>, StoreError> {
    if bytes.len() != dimensions.saturating_mul(4) {
        return Err(StoreError::Invariant(
            "embedding byte length does not match dimensions",
        ));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManualMemoryInput;
    use mega_memory::{MemoryType, ScopeType, Sensitivity};

    #[test]
    fn exact_search_compares_only_matching_model_and_active_memories() {
        let mut store = MemoryStore::in_memory().unwrap();
        let create = |content: &str, now_ms| ManualMemoryInput {
            content: content.into(),
            memory_type: MemoryType::Fact,
            scope_type: ScopeType::Global,
            scope_key: String::new(),
            scope_display_name: "Global".into(),
            category_slugs: vec![],
            applicable_app_ids: vec![],
            importance: 0.5,
            sensitivity: Sensitivity::Private,
            valid_from_ms: None,
            valid_until_ms: None,
            now_ms,
        };
        let first = store
            .create_manual(&create("Stalky uses encrypted SQLite.", 1), "first")
            .unwrap();
        let second = store
            .create_manual(&create("Stalky supports local retrieval.", 2), "second")
            .unwrap();
        store
            .upsert_embedding(&EmbeddingInput {
                memory_id: first.id.clone(),
                provider: "local".into(),
                model: "v1".into(),
                values: vec![1.0, 0.0],
                content_hash: [1; 32],
                embedded_at: 3,
            })
            .unwrap();
        store
            .upsert_embedding(&EmbeddingInput {
                memory_id: second.id.clone(),
                provider: "local".into(),
                model: "v1".into(),
                values: vec![0.0, 1.0],
                content_hash: [2; 32],
                embedded_at: 3,
            })
            .unwrap();
        let matches = store
            .nearest_embeddings(
                &[0.9, 0.1],
                "local",
                "v1",
                &[first.id.clone(), second.id],
                2,
            )
            .unwrap();
        assert_eq!(matches[0].memory_id, first.id);
        assert!(
            store
                .nearest_embeddings(&[1.0, 0.0], "local", "v2", &[first.id], 2)
                .unwrap()
                .is_empty()
        );
    }
}
