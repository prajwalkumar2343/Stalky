import type { MemoryPgliteDatabase } from "../database/index.js";
import { initialSchemaMigration } from "../migrations/001_initial_schema.js";
import { recentContextSourcesMigration } from "../migrations/002_recent_context_sources.js";
import {
  ACTIVE_SCHEMA_CONFIG_KEY,
  EMBEDDING_CONFIG_KEY,
  MIGRATION_CONFIG_PREFIX,
  SCHEMA_VERSION,
  TABLES,
} from "./constants.js";
import type {
  AppliedMigration,
  MigrationFactory,
  MigrationRunnerResult,
} from "./migration-types.js";

const MIGRATIONS: MigrationFactory[] = [initialSchemaMigration, recentContextSourcesMigration];

export interface RunMigrationsOptions {
  embeddingDimensions: number;
}

interface ConfigRow {
  value: Record<string, unknown>;
}

export async function runMigrations(
  database: MemoryPgliteDatabase,
  options: RunMigrationsOptions,
): Promise<MigrationRunnerResult> {
  await ensureConfigTable(database);
  await ensureEmbeddingConfig(database, options.embeddingDimensions);

  const result: MigrationRunnerResult = {
    applied: [],
    skipped: [],
    optionalFailures: [],
  };

  for (const createMigration of MIGRATIONS) {
    const migration = createMigration({ embeddingDimensions: options.embeddingDimensions });
    const migrationKey = `${MIGRATION_CONFIG_PREFIX}${migration.id}`;

    if (await hasConfigKey(database, migrationKey)) {
      result.skipped.push(migration.id);
      continue;
    }

    await database.transaction(async () => {
      for (const statement of migration.statements) {
        await database.exec(statement);
      }

      await upsertConfig(database, migrationKey, {
        id: migration.id,
        description: migration.description,
        applied_at: new Date().toISOString(),
      });
    });

    for (const statement of migration.optionalStatements ?? []) {
      try {
        await database.exec(statement);
      } catch (error) {
        result.optionalFailures.push({
          migration_id: migration.id,
          statement,
          error: error instanceof Error ? error.message : String(error),
        });
      }
    }

    result.applied.push({
      id: migration.id,
      description: migration.description,
      applied_at: new Date().toISOString(),
    });
  }

  await upsertConfig(database, ACTIVE_SCHEMA_CONFIG_KEY, {
    schema_version: SCHEMA_VERSION,
    latest_migration_id: latestMigrationId(),
    updated_at: new Date().toISOString(),
  });

  return result;
}

export async function appliedMigrations(
  database: MemoryPgliteDatabase,
): Promise<AppliedMigration[]> {
  const result = await database.query<{ key: string; value: Record<string, unknown> }>(
    `
      SELECT key, value
      FROM ${TABLES.memoryStoreConfig}
      WHERE key LIKE $1
      ORDER BY key
    `,
    [`${MIGRATION_CONFIG_PREFIX}%`],
  );

  return result.rows.map((row) => ({
    id: String(row.value.id),
    description: String(row.value.description),
    applied_at: String(row.value.applied_at),
  }));
}

export function latestMigrationId(): string {
  const migration = MIGRATIONS.at(-1);
  if (!migration) {
    return "none";
  }

  return migration({ embeddingDimensions: 1536 }).id;
}

async function ensureConfigTable(database: MemoryPgliteDatabase): Promise<void> {
  await database.exec(`
    CREATE TABLE IF NOT EXISTS ${TABLES.memoryStoreConfig} (
      key TEXT PRIMARY KEY,
      value JSONB NOT NULL,
      created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
      updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
    )
  `);
}

async function ensureEmbeddingConfig(
  database: MemoryPgliteDatabase,
  embeddingDimensions: number,
): Promise<void> {
  const existing = await database.query<ConfigRow>(
    `SELECT value FROM ${TABLES.memoryStoreConfig} WHERE key = $1`,
    [EMBEDDING_CONFIG_KEY],
  );

  const current = existing.rows[0]?.value;
  if (!current) {
    await upsertConfig(database, EMBEDDING_CONFIG_KEY, {
      dimensions: embeddingDimensions,
      created_at: new Date().toISOString(),
    });
    return;
  }

  if (Number(current.dimensions) !== embeddingDimensions) {
    throw new Error(
      `Memory store embedding dimension mismatch. Database expects ${String(
        current.dimensions,
      )}; runtime is ${embeddingDimensions}.`,
    );
  }
}

async function hasConfigKey(
  database: MemoryPgliteDatabase,
  key: string,
): Promise<boolean> {
  const result = await database.query<{ exists: boolean }>(
    `SELECT EXISTS(SELECT 1 FROM ${TABLES.memoryStoreConfig} WHERE key = $1) AS exists`,
    [key],
  );
  return Boolean(result.rows[0]?.exists);
}

async function upsertConfig(
  database: MemoryPgliteDatabase,
  key: string,
  value: Record<string, unknown>,
): Promise<void> {
  await database.query(
    `
      INSERT INTO ${TABLES.memoryStoreConfig} (key, value, updated_at)
      VALUES ($1, $2::jsonb, NOW())
      ON CONFLICT (key) DO UPDATE SET
        value = EXCLUDED.value,
        updated_at = NOW()
    `,
    [key, JSON.stringify(value)],
  );
}
