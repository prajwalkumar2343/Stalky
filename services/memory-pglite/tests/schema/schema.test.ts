import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { openMemoryDatabase } from "../../src/database/index.js";
import { TABLES } from "../../src/schema/constants.js";
import { checkSchema } from "../../src/schema/check.js";
import { appliedMigrations, latestMigrationId, runMigrations } from "../../src/schema/migrations.js";
import { withTempDir } from "../helpers/temp-dir.js";

describe("Memory v2 schema", () => {
  it("creates all required tables and records the applied migration", async () => {
    await withTempDir(async (dir) => {
      const database = await openMemoryDatabase({ dataDir: dir, embeddingDimensions: 3 });
      try {
        const schema = await checkSchema(database);

        assert.equal(schema.ok, true);
        assert.equal(schema.latest_migration_id, latestMigrationId());
        assert.deepEqual(schema.missing_tables, []);
        assert.equal(schema.applied_migration_ids.includes("001_initial_memory_v2_schema"), true);

        const tables = await database.query<{ table_name: string }>(
          `
            SELECT table_name
            FROM information_schema.tables
            WHERE table_schema = 'public'
          `,
        );
        const actualTables = new Set(tables.rows.map((row) => row.table_name));
        for (const table of Object.values(TABLES)) {
          assert.equal(actualTables.has(table), true, `${table} should exist`);
        }
      } finally {
        await database.close();
      }
    });
  });

  it("runs migrations idempotently", async () => {
    await withTempDir(async (dir) => {
      const database = await openMemoryDatabase({ dataDir: dir, embeddingDimensions: 3 });
      try {
        const first = await appliedMigrations(database);
        const secondRun = await runMigrations(database, { embeddingDimensions: 3 });
        const second = await appliedMigrations(database);

        assert.deepEqual(first.map((migration) => migration.id), [
          "001_initial_memory_v2_schema",
          "002_recent_context_sources",
        ]);
        assert.deepEqual(
          second.map((migration) => migration.id),
          first.map((migration) => migration.id),
        );
        assert.deepEqual(secondRun.applied, []);
        assert.deepEqual(secondRun.skipped, first.map((migration) => migration.id));
      } finally {
        await database.close();
      }
    });
  });

  it("rejects mismatched embedding dimensions for an existing store", async () => {
    await withTempDir(async (dir) => {
      const database = await openMemoryDatabase({ dataDir: dir, embeddingDimensions: 3 });
      await database.close();

      await assert.rejects(
        () => openMemoryDatabase({ dataDir: dir, embeddingDimensions: 4 }),
        /embedding dimension mismatch/,
      );
    });
  });
});
