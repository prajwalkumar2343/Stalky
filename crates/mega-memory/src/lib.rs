//! Pure domain policy for Stalky's structured, derived memory subsystem.
//!
//! This crate performs no persistence and never accepts raw screen or audio
//! buffers. Model output enters through [`MemoryCandidate::validate`], and only
//! a validated candidate may be reconciled into a mutation plan.

mod extraction;
mod model;
mod privacy;
mod profile;
mod reconcile;
mod retrieval;
mod segmentation;

pub use extraction::{
    AppCatalog, CandidateEntity, CandidateScope, CandidateValidationContext,
    EXTRACTOR_PROMPT_VERSION, EXTRACTOR_SYSTEM_PROMPT, Embedding, EmbeddingProvider,
    ExtractionBatch, ExtractionMetadata, ExtractionResponse, ManualMemoryInput, MemoryCandidate,
    MemoryExtractor, ProviderUsage, ValidatedMemoryCandidate, ValidationError,
    validate_extraction_response,
};
pub use model::*;
pub use privacy::{PrivacyRejection, inspect_private_content};
pub use profile::{
    ProfileItem, ProfileProjection, ProfileProjectionType, build_profile,
    entity_profile_is_eligible,
};
pub use reconcile::{
    CandidateRelationship, MemoryMutationPlan, MemoryMutationResult, ReconciliationError,
    ReconciliationInput, ReconciliationMatch, reconcile_candidate,
};
pub use retrieval::{
    DEFAULT_CONTEXT_TOKEN_BUDGET, MemoryContextRequest, RetrievalSignals, RetrievedMemory,
    VectorError, VectorIndex, conservative_token_estimate, exact_cosine_similarity, rank_memories,
    render_memory_context,
};
pub use segmentation::{
    ActivitySegmentDraft, ActivitySegmenter, SegmentBoundary, SegmentInput, SegmentTransition,
    split_extraction_batches,
};

/// Persistence boundary implemented by `mega-store`.
///
/// Implementations may perform I/O. A plan must be applied atomically and use
/// its idempotency key to make retries safe.
pub trait MemoryRepository {
    type Error;

    fn apply(
        &self,
        plan: MemoryMutationPlan,
    ) -> impl Future<Output = Result<MemoryMutationResult, Self::Error>> + Send;

    fn retrieve(
        &self,
        request: MemoryContextRequest,
    ) -> impl Future<Output = Result<Vec<RetrievedMemory>, Self::Error>> + Send;
}
