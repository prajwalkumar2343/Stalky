import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type { RecentContextEventInput } from "../../src/contracts/index.js";
import { openMemoryDatabase } from "../../src/database/index.js";
import {
  cleanupExpiredRecentContext,
  getRecentContextEvents,
  insertRecentContextEvent,
} from "../../src/recent/events.js";
import { withTempDir } from "../helpers/temp-dir.js";

const baseInput: RecentContextEventInput = {
  user_id: "default_user",
  agent_id: "screen_memories_v3",
  idempotency_key: "ctx_default_user_20260421T090000Z",
  source: "browser",
  content: "Reading AuraBot Memory v2 implementation plan.",
  occurred_at: "2026-04-21T09:00:00Z",
  ttl_seconds: 21600,
  importance: 0.62,
  metadata: {
    context: "Browse",
    activities: ["reading"],
    key_elements: ["Memory v2", "contracts"],
    user_intent: "implementation planning",
    display_num: 1,
    browser: "Safari",
    url: "https://example.test/aurabot/memory-v2",
    capture_reason: "browser_context",
  },
};

describe("recent context events", () => {
  it("inserts and reads a recent context event", async () => {
    await withTempDir(async (dir) => {
      const database = await openMemoryDatabase({ dataDir: dir, embeddingDimensions: 3 });
      try {
        const event = await insertRecentContextEvent(database, baseInput, {
          embedder: async () => [[0.1, 0.2, 0.3]],
        });

        assert.equal(event.user_id, "default_user");
        assert.equal(event.agent_id, "screen_memories_v3");
        assert.equal(event.source, "browser");
        assert.equal(event.content, baseInput.content);
        assert.equal(event.occurred_at, "2026-04-21T09:00:00.000Z");
        assert.equal(event.ttl_seconds, 21600);
        assert.equal(event.importance, 0.62);
        assert.equal(event.metadata.browser, "Safari");
        assert.equal(event.metadata.url, "https://example.test/aurabot/memory-v2");
        assert.equal(event.content_hash.startsWith("sha256:"), true);

        const items = await getRecentContextEvents(database, {
          userId: "default_user",
          agentId: "screen_memories_v3",
          startedAt: "2026-04-21T08:30:00Z",
          endedAt: "2026-04-21T09:30:00Z",
          domain: "www.example.test",
        });

        assert.equal(items.length, 1);
        assert.equal(items[0]?.id, event.id);
      } finally {
        await database.close();
      }
    });
  });

  it("treats idempotency key repeats as the same event", async () => {
    await withTempDir(async (dir) => {
      const database = await openMemoryDatabase({ dataDir: dir, embeddingDimensions: 3 });
      try {
        const first = await insertRecentContextEvent(database, baseInput);
        const second = await insertRecentContextEvent(database, {
          ...baseInput,
          content: "Changed content should not create another event for the same key.",
        });

        assert.equal(second.id, first.id);
        assert.equal(second.content, first.content);

        const items = await getRecentContextEvents(database, { userId: "default_user" });
        assert.equal(items.length, 1);
      } finally {
        await database.close();
      }
    });
  });

  it("filters by repo path and source", async () => {
    await withTempDir(async (dir) => {
      const database = await openMemoryDatabase({ dataDir: dir, embeddingDimensions: 3 });
      try {
        await insertRecentContextEvent(database, {
          ...baseInput,
          idempotency_key: "repo_event",
          source: "repo",
          content: "Editing TypeScript recent context ingestion.",
          occurred_at: "2026-04-21T09:08:00Z",
          metadata: {
            ...baseInput.metadata,
            context: "Code",
            repo_path: "/Users/administrator/Downloads/aurabot",
            file_path: "services/memory-pglite/src/recent/events.ts",
          },
        });

        const repoItems = await getRecentContextEvents(database, {
          userId: "default_user",
          source: "repo",
          repoPath: "/Users/administrator/Downloads/aurabot",
        });

        assert.equal(repoItems.length, 1);
        assert.equal(repoItems[0]?.metadata.repo_path, "/Users/administrator/Downloads/aurabot");
      } finally {
        await database.close();
      }
    });
  });

  it("persists dynamic terminal and system context sources", async () => {
    await withTempDir(async (dir) => {
      const database = await openMemoryDatabase({ dataDir: dir, embeddingDimensions: 3 });
      try {
        await insertRecentContextEvent(database, {
          ...baseInput,
          idempotency_key: "terminal_event",
          source: "terminal",
          content: "Running local memory verification commands in Terminal.",
          occurred_at: "2026-04-21T09:10:00Z",
          metadata: {
            ...baseInput.metadata,
            context: "terminal_debugging",
            activities: ["terminal", "verification"],
            key_elements: ["npm test", "memory-pglite"],
            app_name: "Terminal",
            window_title: "aurabot - npm test",
          },
        });

        await insertRecentContextEvent(database, {
          ...baseInput,
          idempotency_key: "system_event",
          source: "system",
          content: "AuraBot capture loop started and health checks are green.",
          occurred_at: "2026-04-21T09:11:00Z",
          metadata: {
            ...baseInput.metadata,
            context: "system_status",
            activities: ["capture_loop", "health_check"],
            key_elements: ["memory backend", "local capture"],
          },
        });

        const terminalItems = await getRecentContextEvents(database, {
          userId: "default_user",
          source: "terminal",
          app: "Terminal",
        });
        const systemItems = await getRecentContextEvents(database, {
          userId: "default_user",
          source: "system",
        });

        assert.equal(terminalItems.length, 1);
        assert.equal(terminalItems[0]?.metadata.window_title, "aurabot - npm test");
        assert.equal(systemItems.length, 1);
        assert.equal(systemItems[0]?.content, "AuraBot capture loop started and health checks are green.");
      } finally {
        await database.close();
      }
    });
  });

  it("cleans up events older than the configured horizon", async () => {
    await withTempDir(async (dir) => {
      const database = await openMemoryDatabase({ dataDir: dir, embeddingDimensions: 3 });
      try {
        await insertRecentContextEvent(database, {
          ...baseInput,
          idempotency_key: "old_event",
          occurred_at: "2026-04-21T01:00:00Z",
        });
        await insertRecentContextEvent(database, {
          ...baseInput,
          idempotency_key: "new_event",
          occurred_at: "2026-04-21T09:00:00Z",
        });

        const dryRun = await cleanupExpiredRecentContext(database, {
          now: "2026-04-21T10:00:00Z",
          olderThanHours: 6,
          dryRun: true,
        });
        assert.equal(dryRun.deleted_ids.length, 1);

        const deleted = await cleanupExpiredRecentContext(database, {
          now: "2026-04-21T10:00:00Z",
          olderThanHours: 6,
        });
        assert.equal(deleted.deleted_ids.length, 1);

        const remaining = await getRecentContextEvents(database, { userId: "default_user" });
        assert.equal(remaining.length, 1);
        assert.equal(remaining[0]?.occurred_at, "2026-04-21T09:00:00.000Z");
      } finally {
        await database.close();
      }
    });
  });

  it("rejects invalid payloads before touching the database", async () => {
    await withTempDir(async (dir) => {
      const database = await openMemoryDatabase({ dataDir: dir, embeddingDimensions: 3 });
      try {
        await assert.rejects(
          () =>
            insertRecentContextEvent(database, {
              ...baseInput,
              idempotency_key: "",
            }),
          /idempotency_key is required/,
        );
        await assert.rejects(
          () =>
            insertRecentContextEvent(database, {
              ...baseInput,
              occurred_at: "not-a-date",
            }),
          /occurred_at must be an ISO 8601 timestamp/,
        );
      } finally {
        await database.close();
      }
    });
  });
});
