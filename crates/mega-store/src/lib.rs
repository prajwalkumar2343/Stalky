//! Encrypted, local-only persistence for Stalky's structured memory.
//!
//! SQL and encryption stay in this adapter crate. Memory policy and ranking
//! belong to `mega-memory` so they can be tested without a database.

mod context_repo;
mod embedding_repo;
mod entity_repo;
mod history_repo;
mod memory_repo;
mod operations_repo;
mod profile_repo;
mod queue_repo;
mod source_repo;

pub use history_repo::{
    AudioAsset, AudioAssetInput, AudioAssetStatus, HistoryAdmission, HistoryMediaKind,
    HistoryRetentionPolicy, HistoryRetentionReport, HistorySourceKind, TimelineEntry,
    TimelineEntryInput, TimelineMediaKind, TimelineSearchFilter, TimelineSourceKind,
};
pub use memory_repo::{
    ManualMemoryInput, MemoryAppFilterRole, MemorySearchFilter, MemoryStore, MemoryStoreConfig,
    StoreError,
};
pub use operations_repo::{DeleteMode, MemoryEvent, MemoryEventType, RetentionReport};
pub use profile_repo::{ProfileRecord, ProfileType};
pub use queue_repo::{ExtractionJob, ExtractionJobCompletion, ExtractionJobFailure, JobState};
pub use source_repo::{
    ActivitySegmentInput, SegmentCloseReason, SourceEventAdmission, SourceEventInput, SourceKind,
};

const MIGRATION_0001: &str = include_str!("../migrations/0001_memory.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_operations.sql");
const MIGRATION_0003: &str = include_str!("../migrations/0003_history.sql");
const MIGRATION_0004: &str = include_str!("../migrations/0004_history_legacy.sql");
const TAXONOMY_VERSION: i64 = 1;
pub use context_repo::MemoryContextAssembly;
pub use embedding_repo::{EmbeddingInput, EmbeddingMatch};
pub use entity_repo::{EntityRecord, EntityResolution};
