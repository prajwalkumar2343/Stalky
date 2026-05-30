export const SCHEMA_VERSION = "memory-v2";
export const MIGRATION_CONFIG_PREFIX = "migration:";
export const EMBEDDING_CONFIG_KEY = "embedding";
export const ACTIVE_SCHEMA_CONFIG_KEY = "active_schema";

export const TABLES = {
  memoryStoreConfig: "memory_store_config",
  recentContextEvents: "recent_context_events",
  recentContextSummaries: "recent_context_summaries",
  brainPages: "brain_pages",
  brainChunks: "brain_chunks",
  entities: "entities",
  entityAliases: "entity_aliases",
  entityLinks: "entity_links",
  timelineEvents: "timeline_events",
  memoryJobs: "memory_jobs",
} as const;

export const ENTITY_TYPES = [
  "user",
  "person",
  "company",
  "project",
  "app",
  "website",
  "repo",
  "file",
  "workflow",
  "concept",
  "decision",
  "task",
  "meeting",
  "document",
  "preference",
] as const;

export const RELATION_TYPES = [
  "works_on",
  "uses",
  "visited",
  "opened",
  "edited",
  "mentioned_in",
  "discussed_with",
  "decided_in",
  "evidence_for",
  "related_to",
  "depends_on",
  "blocks",
  "belongs_to",
  "part_of",
  "authored",
  "created",
  "prefers",
] as const;

export const RECENT_CONTEXT_SOURCES = [
  "screen",
  "app",
  "browser",
  "repo",
  "file",
  "terminal",
  "system",
] as const;

export const BRAIN_CHUNK_TYPES = ["frontmatter", "compiled_truth", "timeline"] as const;

export const MEMORY_JOB_STATUSES = [
  "queued",
  "running",
  "completed",
  "failed",
  "cancelled",
] as const;

export type TableName = (typeof TABLES)[keyof typeof TABLES];
export type EntityType = (typeof ENTITY_TYPES)[number];
export type RelationType = (typeof RELATION_TYPES)[number];
export type RecentContextSource = (typeof RECENT_CONTEXT_SOURCES)[number];
export type BrainChunkType = (typeof BRAIN_CHUNK_TYPES)[number];
export type MemoryJobStatus = (typeof MEMORY_JOB_STATUSES)[number];
