use mega_memory::{
    AppId, AssertionMode, EntityId, HistoricalMode, MemoryId, MemoryStatus, MemoryType, ScopeType,
    Sensitivity, SensitivityAllowance,
};
use serde::{Deserialize, Serialize};

pub const MAX_MEMORY_QUERY_CHARS: usize = 500;
pub const MAX_MEMORY_RESULTS: usize = 200;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MemorySearchRequest {
    pub query: Option<String>,
    pub app_id: Option<AppId>,
    pub category_slug: Option<String>,
    pub scope_type: Option<ScopeType>,
    pub scope_key: Option<String>,
    pub entity_id: Option<EntityId>,
    pub memory_type: Option<MemoryType>,
    pub status: Option<MemoryStatus>,
    pub historical_mode: HistoricalMode,
    pub from_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub limit: usize,
}

impl MemorySearchRequest {
    pub fn validate(&self) -> Result<(), MemoryRequestError> {
        if self.query.as_ref().is_some_and(|query| {
            query.trim().is_empty() || query.chars().count() > MAX_MEMORY_QUERY_CHARS
        }) {
            return Err(MemoryRequestError::QueryTooLong);
        }
        if self.limit == 0 || self.limit > MAX_MEMORY_RESULTS {
            return Err(MemoryRequestError::InvalidLimit);
        }
        if let (Some(from), Some(until)) = (self.from_ms, self.until_ms)
            && until < from
        {
            return Err(MemoryRequestError::InvalidDateRange);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CreateManualMemoryRequest {
    pub correlation_id: String,
    pub content: String,
    pub memory_type: MemoryType,
    pub scope_type: ScopeType,
    pub scope_key: String,
    pub scope_display_name: String,
    pub category_slugs: Vec<String>,
    pub applicable_app_ids: Vec<AppId>,
    pub importance: f32,
    pub sensitivity: Sensitivity,
    pub valid_from_ms: Option<i64>,
    pub valid_until_ms: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EditMemoryRequest {
    pub memory_id: MemoryId,
    pub expected_revision: u32,
    pub replacement: CreateManualMemoryRequest,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MemorySearchResultDto {
    pub id: MemoryId,
    pub content: String,
    pub memory_type: MemoryType,
    pub assertion_mode: AssertionMode,
    pub status: MemoryStatus,
    pub scope_type: ScopeType,
    pub scope_key: String,
    pub confidence: f32,
    pub source_app_ids: Vec<AppId>,
    pub source_timestamps_ms: Vec<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConfirmMemoryRequest {
    pub correlation_id: String,
    pub memory_id: MemoryId,
    pub expected_revision: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RejectMemoryRequest {
    pub correlation_id: String,
    pub memory_id: MemoryId,
    pub expected_revision: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeleteMemoryRequest {
    pub correlation_id: String,
    pub memory_id: MemoryId,
    pub expected_revision: u32,
    pub mode: MemoryDeleteMode,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryDeleteMode {
    Forget,
    Permanent,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryErrorCode {
    InvalidRequest,
    NotFound,
    RevisionConflict,
    EncryptionUnavailable,
    LeaseLost,
    MemoryUnavailable,
    StorageFailure,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryCommandError {
    pub code: MemoryErrorCode,
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MemoryContextDto {
    pub rendered: String,
    pub memory_ids: Vec<MemoryId>,
    pub estimated_tokens: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MemoryContextRequestDto {
    pub correlation_id: String,
    pub current_app_id: Option<AppId>,
    pub project_key: Option<String>,
    pub query_text: String,
    pub mentioned_entity_ids: Vec<EntityId>,
    pub historical_mode: HistoricalMode,
    pub sensitivity_allowance: SensitivityAllowance,
    pub token_budget: usize,
    pub temporal_query: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MemoryRequestError {
    #[error("memory search query exceeds 500 characters")]
    QueryTooLong,
    #[error("memory search limit must be between 1 and 200")]
    InvalidLimit,
    #[error("memory search end date precedes start date")]
    InvalidDateRange,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_contract_bounds_query_results_and_dates() {
        let valid = MemorySearchRequest {
            query: Some("mushrooms".into()),
            app_id: None,
            category_slug: None,
            scope_type: None,
            scope_key: None,
            entity_id: None,
            memory_type: None,
            status: None,
            historical_mode: HistoricalMode::ActiveOnly,
            from_ms: Some(1),
            until_ms: Some(2),
            limit: 20,
        };
        assert!(valid.validate().is_ok());
        assert_eq!(
            MemorySearchRequest {
                limit: 0,
                ..valid.clone()
            }
            .validate(),
            Err(MemoryRequestError::InvalidLimit)
        );
        assert_eq!(
            MemorySearchRequest {
                from_ms: Some(3),
                ..valid
            }
            .validate(),
            Err(MemoryRequestError::InvalidDateRange)
        );
    }
}
