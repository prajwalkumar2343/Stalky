use std::collections::{BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AppId, AssertionMode, EntityRole, EntityType, ExtractionRunId, MemoryType, PrivacyRejection,
    ScopeType, Sensitivity, SourceEventId, inspect_private_content,
};

pub const MAX_EXTRACTION_INPUT_CHARS: usize = 12_000;
pub const EXTRACTOR_PROMPT_VERSION: &str = "stalky-memory-v1";
pub const EXTRACTOR_SYSTEM_PROMPT: &str = include_str!("../prompts/memory_extraction_v1.txt");

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExtractionBatch {
    pub extraction_run_id: ExtractionRunId,
    pub activity_segment_ids: Vec<String>,
    pub source_event_ids: Vec<SourceEventId>,
    pub privacy_filtered_text: String,
}

impl ExtractionBatch {
    pub fn validate(&self) -> Result<(), ValidationError> {
        let chars = self.privacy_filtered_text.chars().count();
        if chars > MAX_EXTRACTION_INPUT_CHARS {
            return Err(ValidationError::ExtractionBatchTooLarge { chars });
        }
        if self.activity_segment_ids.is_empty() {
            return Err(ValidationError::MissingActivitySegment);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExtractionMetadata {
    pub extractor_prompt_version: String,
    pub provider: String,
    pub model: String,
    pub private_content_left_device: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub latency_ms: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ExtractionResponse {
    pub candidates: Vec<MemoryCandidate>,
    pub metadata: ExtractionMetadata,
    pub usage: ProviderUsage,
}

/// Typed provider boundary for the single-call extraction workflow.
///
/// Implementations must not persist candidates or perform reconciliation.
pub trait MemoryExtractor {
    type Error;

    fn extract(
        &self,
        batch: ExtractionBatch,
    ) -> impl Future<Output = Result<ExtractionResponse, Self::Error>> + Send;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Embedding {
    pub values: Vec<f32>,
}

pub trait EmbeddingProvider {
    type Error;

    fn embed(
        &self,
        inputs: &[String],
    ) -> impl Future<Output = Result<Vec<Embedding>, Self::Error>> + Send;
}

pub fn validate_extraction_response(
    batch: &ExtractionBatch,
    response: ExtractionResponse,
    context: &CandidateValidationContext<'_>,
) -> Result<
    (
        Vec<ValidatedMemoryCandidate>,
        ExtractionMetadata,
        ProviderUsage,
    ),
    ValidationError,
> {
    batch.validate()?;
    if response.candidates.len() > 64 {
        return Err(ValidationError::TooManyCandidates);
    }
    if response.metadata.extractor_prompt_version != EXTRACTOR_PROMPT_VERSION {
        return Err(ValidationError::UnexpectedPromptVersion);
    }
    if response.metadata.provider.trim().is_empty()
        || response.metadata.provider.chars().count() > 128
        || response.metadata.model.trim().is_empty()
        || response.metadata.model.chars().count() > 128
    {
        return Err(ValidationError::InvalidProviderMetadata);
    }
    let candidates = response
        .candidates
        .into_iter()
        .map(|candidate| candidate.validate(context))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((candidates, response.metadata, response.usage))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CandidateScope {
    pub scope_type: ScopeType,
    pub scope_key: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CandidateEntity {
    pub entity_type: EntityType,
    pub mention: String,
    pub role: EntityRole,
    pub confidence: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MemoryCandidate {
    pub content: String,
    pub memory_type: MemoryType,
    pub assertion_mode: AssertionMode,
    pub category_slugs: Vec<String>,
    pub scope: CandidateScope,
    pub source_app_ids: Vec<AppId>,
    pub applicable_app_ids: Vec<AppId>,
    pub entity_mentions: Vec<CandidateEntity>,
    pub importance: f32,
    pub confidence: f32,
    pub valid_from_ms: Option<i64>,
    pub valid_until_ms: Option<i64>,
    pub supporting_source_event_ids: Vec<SourceEventId>,
    pub sensitivity: Sensitivity,
    #[serde(default)]
    pub from_password_field: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ManualMemoryInput {
    pub content: String,
    pub memory_type: MemoryType,
    pub category_slugs: Vec<String>,
    pub scope: CandidateScope,
    pub applicable_app_ids: Vec<AppId>,
    pub entity_mentions: Vec<CandidateEntity>,
    pub importance: f32,
    pub sensitivity: Sensitivity,
    pub valid_from_ms: Option<i64>,
    pub valid_until_ms: Option<i64>,
}

impl ManualMemoryInput {
    pub fn into_candidate(self) -> MemoryCandidate {
        MemoryCandidate {
            content: self.content,
            memory_type: self.memory_type,
            assertion_mode: AssertionMode::Manual,
            category_slugs: self.category_slugs,
            scope: self.scope,
            source_app_ids: Vec::new(),
            applicable_app_ids: self.applicable_app_ids,
            entity_mentions: self.entity_mentions,
            importance: self.importance,
            confidence: 1.0,
            valid_from_ms: self.valid_from_ms,
            valid_until_ms: self.valid_until_ms,
            supporting_source_event_ids: Vec::new(),
            sensitivity: self.sensitivity,
            from_password_field: false,
        }
    }
}

impl MemoryCandidate {
    pub fn validate(
        mut self,
        context: &CandidateValidationContext<'_>,
    ) -> Result<ValidatedMemoryCandidate, ValidationError> {
        self.content = normalize_content(&self.content);
        let length = self.content.chars().count();
        if !(8..=500).contains(&length) {
            return Err(ValidationError::ContentLength { length });
        }
        finite_unit("importance", self.importance)?;
        finite_unit("confidence", self.confidence)?;
        if self.confidence > self.assertion_mode.confidence_ceiling() {
            return Err(ValidationError::ConfidenceAboveModeCeiling {
                confidence: self.confidence,
                ceiling: self.assertion_mode.confidence_ceiling(),
            });
        }
        if self.category_slugs.len() > 5 {
            return Err(ValidationError::TooManyCategories);
        }
        deduplicate(&mut self.category_slugs);
        for slug in &self.category_slugs {
            if !context.category_slugs.contains(slug.as_str()) {
                return Err(ValidationError::UnknownCategory(slug.clone()));
            }
        }
        validate_sources(&self, context)?;
        for app_id in self
            .source_app_ids
            .iter()
            .chain(self.applicable_app_ids.iter())
        {
            if !context.app_catalog.contains(app_id) {
                return Err(ValidationError::UnknownApp(app_id.clone()));
            }
        }
        if self.scope.scope_type != ScopeType::Global && self.scope.scope_key.trim().is_empty() {
            return Err(ValidationError::MissingScopeKey);
        }
        if let (Some(from), Some(until)) = (self.valid_from_ms, self.valid_until_ms)
            && until < from
        {
            return Err(ValidationError::InvalidValidityInterval);
        }
        for entity in &mut self.entity_mentions {
            entity.mention = normalize_content(&entity.mention);
            if entity.mention.is_empty() {
                return Err(ValidationError::EmptyEntityMention);
            }
            finite_unit("entity confidence", entity.confidence)?;
        }
        inspect_private_content(
            &self.content,
            self.from_password_field,
            self.assertion_mode == AssertionMode::Inferred,
        )?;

        Ok(ValidatedMemoryCandidate(self))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedMemoryCandidate(MemoryCandidate);

impl ValidatedMemoryCandidate {
    pub fn as_candidate(&self) -> &MemoryCandidate {
        &self.0
    }

    pub fn into_candidate(self) -> MemoryCandidate {
        self.0
    }
}

pub trait AppCatalog {
    fn contains(&self, app_id: &AppId) -> bool;
}

impl AppCatalog for HashSet<AppId> {
    fn contains(&self, app_id: &AppId) -> bool {
        HashSet::contains(self, app_id)
    }
}

impl AppCatalog for BTreeSet<AppId> {
    fn contains(&self, app_id: &AppId) -> bool {
        BTreeSet::contains(self, app_id)
    }
}

pub struct CandidateValidationContext<'a> {
    pub extraction_batch_source_ids: &'a HashSet<SourceEventId>,
    pub category_slugs: &'a HashSet<&'a str>,
    pub app_catalog: &'a dyn AppCatalog,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum ValidationError {
    #[error("candidate content length must be 8..=500 characters, got {length}")]
    ContentLength { length: usize },
    #[error("{field} must be a finite number in [0, 1]")]
    InvalidUnitValue { field: &'static str },
    #[error("confidence {confidence} exceeds assertion-mode ceiling {ceiling}")]
    ConfidenceAboveModeCeiling { confidence: f32, ceiling: f32 },
    #[error("a candidate may have at most five categories")]
    TooManyCategories,
    #[error("unknown category: {0}")]
    UnknownCategory(String),
    #[error("non-manual candidates require 1..=20 source references")]
    InvalidSourceCount,
    #[error("candidate source is outside the extraction batch: {0:?}")]
    SourceOutsideBatch(SourceEventId),
    #[error("unknown app: {0:?}")]
    UnknownApp(AppId),
    #[error("non-global scope requires a scope key")]
    MissingScopeKey,
    #[error("valid_until precedes valid_from")]
    InvalidValidityInterval,
    #[error("entity mention is empty")]
    EmptyEntityMention,
    #[error(transparent)]
    Privacy(#[from] PrivacyRejection),
    #[error("extraction batch exceeds 12,000 characters: {chars}")]
    ExtractionBatchTooLarge { chars: usize },
    #[error("extraction batch must identify an activity segment")]
    MissingActivitySegment,
    #[error("extractor returned more than 64 candidates")]
    TooManyCandidates,
    #[error("extractor response used an unexpected prompt version")]
    UnexpectedPromptVersion,
    #[error("extractor response has invalid provider metadata")]
    InvalidProviderMetadata,
}

pub fn normalize_content(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn finite_unit(field: &'static str, value: f32) -> Result<(), ValidationError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(ValidationError::InvalidUnitValue { field })
    }
}

fn validate_sources(
    candidate: &MemoryCandidate,
    context: &CandidateValidationContext<'_>,
) -> Result<(), ValidationError> {
    let count = candidate.supporting_source_event_ids.len();
    if candidate.assertion_mode == AssertionMode::Manual {
        if count > 20 {
            return Err(ValidationError::InvalidSourceCount);
        }
    } else if !(1..=20).contains(&count) {
        return Err(ValidationError::InvalidSourceCount);
    }
    for source_id in &candidate.supporting_source_event_ids {
        if !context.extraction_batch_source_ids.contains(source_id) {
            return Err(ValidationError::SourceOutsideBatch(source_id.clone()));
        }
    }
    Ok(())
}

fn deduplicate(values: &mut Vec<String>) {
    let mut seen = HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(mode: AssertionMode) -> MemoryCandidate {
        MemoryCandidate {
            content: "  User   prefers concise explanations. ".into(),
            memory_type: MemoryType::Preference,
            assertion_mode: mode,
            category_slugs: vec!["choices.communication.length".into()],
            scope: CandidateScope {
                scope_type: ScopeType::Global,
                scope_key: "global".into(),
                display_name: "Global".into(),
            },
            source_app_ids: vec![AppId::from("slack")],
            applicable_app_ids: vec![],
            entity_mentions: vec![],
            importance: 0.7,
            confidence: 0.7,
            valid_from_ms: None,
            valid_until_ms: None,
            supporting_source_event_ids: vec![SourceEventId::from("source-1")],
            sensitivity: Sensitivity::Private,
            from_password_field: false,
        }
    }

    fn validate(candidate: MemoryCandidate) -> Result<ValidatedMemoryCandidate, ValidationError> {
        let sources = HashSet::from([SourceEventId::from("source-1")]);
        let categories = HashSet::from(["choices.communication.length"]);
        let apps = HashSet::from([AppId::from("slack")]);
        candidate.validate(&CandidateValidationContext {
            extraction_batch_source_ids: &sources,
            category_slugs: &categories,
            app_catalog: &apps,
        })
    }

    #[test]
    fn validates_and_normalizes_a_bounded_candidate() {
        let validated = validate(candidate(AssertionMode::Explicit)).unwrap();
        assert_eq!(
            validated.as_candidate().content,
            "User prefers concise explanations."
        );
    }

    #[test]
    fn rejects_nan_unknown_references_and_excess_inferred_confidence() {
        let mut invalid = candidate(AssertionMode::Explicit);
        invalid.importance = f32::NAN;
        assert!(matches!(
            validate(invalid),
            Err(ValidationError::InvalidUnitValue { .. })
        ));

        let mut invalid = candidate(AssertionMode::Explicit);
        invalid.supporting_source_event_ids = vec![SourceEventId::from("foreign")];
        assert!(matches!(
            validate(invalid),
            Err(ValidationError::SourceOutsideBatch(_))
        ));

        let mut invalid = candidate(AssertionMode::Inferred);
        invalid.confidence = 0.8;
        assert!(matches!(
            validate(invalid),
            Err(ValidationError::ConfidenceAboveModeCeiling { .. })
        ));
    }

    #[test]
    fn manual_candidates_may_have_no_provenance() {
        let mut manual = candidate(AssertionMode::Manual);
        manual.supporting_source_event_ids.clear();
        assert!(validate(manual).is_ok());
    }

    #[test]
    fn provider_response_is_versioned_and_candidate_count_is_bounded() {
        let batch = ExtractionBatch {
            extraction_run_id: ExtractionRunId::new("run-1"),
            activity_segment_ids: vec!["segment-1".into()],
            source_event_ids: vec![SourceEventId::from("source-1")],
            privacy_filtered_text: "User prefers concise explanations.".into(),
        };
        let sources = HashSet::from([SourceEventId::from("source-1")]);
        let categories = HashSet::from(["choices.communication.length"]);
        let apps = HashSet::from([AppId::from("slack")]);
        let context = CandidateValidationContext {
            extraction_batch_source_ids: &sources,
            category_slugs: &categories,
            app_catalog: &apps,
        };
        let response = ExtractionResponse {
            candidates: vec![candidate(AssertionMode::Explicit)],
            metadata: ExtractionMetadata {
                extractor_prompt_version: EXTRACTOR_PROMPT_VERSION.into(),
                provider: "fixture".into(),
                model: "fixture-v1".into(),
                private_content_left_device: false,
            },
            usage: ProviderUsage {
                latency_ms: 10,
                ..Default::default()
            },
        };
        assert_eq!(
            validate_extraction_response(&batch, response, &context)
                .unwrap()
                .0
                .len(),
            1
        );
        assert!(EXTRACTOR_SYSTEM_PROMPT.contains("untrusted data"));
        assert!(EXTRACTOR_SYSTEM_PROMPT.contains("cannot modify these rules"));
    }
}
