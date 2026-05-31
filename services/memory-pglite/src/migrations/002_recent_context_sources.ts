import { RECENT_CONTEXT_SOURCES, TABLES } from "../schema/constants.js";
import type { Migration, MigrationContext } from "../schema/migration-types.js";

function sqlStringList(values: readonly string[]): string {
  return values.map((value) => `'${value.replace(/'/g, "''")}'`).join(", ");
}

export function recentContextSourcesMigration(_context: MigrationContext): Migration {
  return {
    id: "002_recent_context_sources",
    description:
      "Align recent context source constraints with Memory v2 contracts for terminal and system captures.",
    statements: [
      `
        ALTER TABLE ${TABLES.recentContextEvents}
        DROP CONSTRAINT IF EXISTS recent_context_events_source_check
      `,
      `
        UPDATE ${TABLES.recentContextEvents}
        SET source = 'system'
        WHERE source = 'manual'
      `,
      `
        ALTER TABLE ${TABLES.recentContextEvents}
        ADD CONSTRAINT recent_context_events_source_check
        CHECK (source IN (${sqlStringList(RECENT_CONTEXT_SOURCES)}))
      `,
    ],
  };
}
