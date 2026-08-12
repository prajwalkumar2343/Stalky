use std::collections::BTreeMap;

use mega_memory::{
    HistoricalMode, MemoryContextRequest, MemoryId, RetrievalSignals, RetrievedMemory, ScopeType,
    conservative_token_estimate, rank_memories, render_memory_context,
};

use crate::operations_repo::{MemoryEventType, record_event_tx};
use crate::{MemoryAppFilterRole, MemorySearchFilter, MemoryStore, StoreError};

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryContextAssembly {
    pub rendered: String,
    pub memory_ids: Vec<MemoryId>,
    pub estimated_tokens: usize,
}

impl MemoryStore {
    pub fn assemble_context(
        &mut self,
        request: &MemoryContextRequest,
        correlation_id: &str,
        now_ms: i64,
    ) -> Result<MemoryContextAssembly, StoreError> {
        validate_context_request(request)?;
        let include_history = request.historical_mode == HistoricalMode::IncludeHistory;
        let mut candidates = BTreeMap::<MemoryId, RetrievedMemory>::new();

        if let Some(project_key) = &request.active_project_key {
            add_search_results(
                &mut candidates,
                self.search(&MemorySearchFilter {
                    scope_type: Some(ScopeType::Project),
                    scope_key: Some(project_key.clone()),
                    include_history,
                    limit: 200,
                    ..Default::default()
                })?,
                RetrievalSignals {
                    fts_relevance: 0.0,
                    ..Default::default()
                },
            );
        }
        if let Some(app_id) = &request.current_app_id {
            add_search_results(
                &mut candidates,
                self.search(&MemorySearchFilter {
                    app_id: Some(app_id.clone()),
                    app_role: MemoryAppFilterRole::AppliesTo,
                    include_history,
                    limit: 200,
                    ..Default::default()
                })?,
                RetrievalSignals::default(),
            );
            add_search_results(
                &mut candidates,
                self.search(&MemorySearchFilter {
                    scope_type: Some(ScopeType::App),
                    scope_key: Some(app_id.as_str().to_owned()),
                    include_history,
                    limit: 200,
                    ..Default::default()
                })?,
                RetrievalSignals::default(),
            );
        }
        for entity_id in request.mentioned_entity_ids.iter().take(20) {
            add_search_results(
                &mut candidates,
                self.search(&MemorySearchFilter {
                    entity_id: Some(entity_id.clone()),
                    include_history,
                    limit: 100,
                    ..Default::default()
                })?,
                RetrievalSignals {
                    exact_entity_alias_match: true,
                    ..Default::default()
                },
            );
        }
        add_search_results(
            &mut candidates,
            self.search(&MemorySearchFilter {
                scope_type: Some(ScopeType::Global),
                include_history,
                limit: 200,
                ..Default::default()
            })?,
            RetrievalSignals::default(),
        );
        if !request.query_text.trim().is_empty() {
            add_search_results(
                &mut candidates,
                self.search(&MemorySearchFilter {
                    query: Some(request.query_text.clone()),
                    include_history,
                    limit: 200,
                    ..Default::default()
                })?,
                RetrievalSignals {
                    fts_relevance: 1.0,
                    ..Default::default()
                },
            );
        }

        let ranked = rank_memories(request, candidates.into_values());
        let rendered = render_memory_context(request, &ranked);
        let memory_ids = included_ids(&rendered, &ranked);
        let estimated_tokens = conservative_token_estimate(&rendered);
        if estimated_tokens > request.total_token_budget {
            return Err(StoreError::Invariant(
                "rendered context exceeded its token budget",
            ));
        }
        let tx = self.connection_mut().transaction()?;
        record_event_tx(
            &tx,
            MemoryEventType::ContextAssembled,
            correlation_id,
            None,
            None,
            Some(if memory_ids.is_empty() {
                "empty"
            } else {
                "bounded"
            }),
            now_ms,
        )?;
        tx.commit()?;
        Ok(MemoryContextAssembly {
            rendered,
            memory_ids,
            estimated_tokens,
        })
    }
}

fn validate_context_request(request: &MemoryContextRequest) -> Result<(), StoreError> {
    if request.total_token_budget > 16_000 {
        return Err(StoreError::InvalidInput(
            "memory context budget exceeds 16,000 tokens",
        ));
    }
    if request.query_text.chars().count() > 4_000 || request.mentioned_entity_ids.len() > 20 {
        return Err(StoreError::InvalidInput(
            "memory context request is unbounded",
        ));
    }
    Ok(())
}

fn add_search_results(
    candidates: &mut BTreeMap<MemoryId, RetrievedMemory>,
    memories: Vec<mega_memory::Memory>,
    signals: RetrievalSignals,
) {
    for memory in memories {
        let source_timestamp_ms = Some(memory.updated_at_ms);
        candidates
            .entry(memory.id.clone())
            .and_modify(|existing| {
                existing.signals.fts_relevance =
                    existing.signals.fts_relevance.max(signals.fts_relevance);
                existing.signals.exact_entity_alias_match |= signals.exact_entity_alias_match;
            })
            .or_insert(RetrievedMemory {
                memory,
                signals,
                score: 0.0,
                source_timestamp_ms,
            });
    }
}

fn included_ids(rendered: &str, ranked: &[RetrievedMemory]) -> Vec<MemoryId> {
    ranked
        .iter()
        .filter(|item| rendered.contains(&format!("id=\"{}\"", item.memory.id.as_str())))
        .map(|item| item.memory.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use mega_memory::{
        HistoricalMode, MemoryContextRequest, MemoryType, ScopeType, Sensitivity,
        SensitivityAllowance,
    };

    use super::*;
    use crate::ManualMemoryInput;

    #[test]
    fn context_is_scoped_traceable_and_budgeted() {
        let mut store = MemoryStore::in_memory().unwrap();
        let create = |content: &str, scope_type, scope_key: &str, now_ms| ManualMemoryInput {
            content: content.into(),
            memory_type: MemoryType::Decision,
            scope_type,
            scope_key: scope_key.into(),
            scope_display_name: scope_key.into(),
            category_slugs: vec![],
            applicable_app_ids: vec![],
            importance: 0.9,
            sensitivity: Sensitivity::Private,
            valid_from_ms: None,
            valid_until_ms: None,
            now_ms,
        };
        let project = store
            .create_manual(
                &create(
                    "Stalky uses encrypted SQLite memory.",
                    ScopeType::Project,
                    "stalky",
                    1,
                ),
                "project",
            )
            .unwrap();
        store
            .create_manual(
                &create(
                    "Another project uses a different database.",
                    ScopeType::Project,
                    "other",
                    2,
                ),
                "other",
            )
            .unwrap();
        let assembly = store
            .assemble_context(
                &MemoryContextRequest {
                    current_app_id: None,
                    active_project_key: Some("stalky".into()),
                    query_text: "SQLite".into(),
                    mentioned_entity_ids: vec![],
                    historical_mode: HistoricalMode::ActiveOnly,
                    sensitivity_allowance: SensitivityAllowance::IncludePrivate,
                    total_token_budget: 500,
                    temporal_query: false,
                },
                "context-request",
                3,
            )
            .unwrap();
        assert!(assembly.rendered.contains(project.id.as_str()));
        assert!(!assembly.rendered.contains("Another project"));
        assert!(assembly.estimated_tokens <= 500);
        assert_eq!(assembly.memory_ids, vec![project.id]);
    }
}
