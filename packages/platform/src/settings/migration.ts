// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: AGPL-3.0-only
import type { CallContext } from '@nous-wave/contracts/common';
import type { ColumnType, Kysely } from 'kysely';
import type { Migration } from 'kysely/migration';
import { sql } from '../storage/index.js';
import type {
  JsonObject,
  ModuleDescription,
  PatchOperation,
  SettingsChangeResult,
  SettingsRevisionOrigin,
} from './types.js';

type Timestamp = ColumnType<string, string | undefined, never>;
export interface SettingsDatabase {
  settingsDefinitions: {
    namespace: string;
    definitionRevision: string;
    jsonSchema: ModuleDescription['jsonSchema'];
    defaults: JsonObject;
    metadata: { owner: string; fields: ModuleDescription['fields'] };
    recordedAt: Timestamp;
  };
  settingsChanges: {
    operationId: string;
    namespace: string;
    scopeKind: string;
    scopeId: string;
    actorRef: string;
    requestDigest: string;
    result: SettingsChangeResult;
    recordedAt: Timestamp;
  };
  settingsRevisions: {
    namespace: string;
    scopeKind: string;
    scopeId: string;
    revision: string;
    definitionRevision: string;
    sparse: JsonObject;
    patch: PatchOperation[];
    actor: CallContext['actor'];
    reason: string | null;
    recordedAt: Timestamp;
    origin: ColumnType<
      SettingsRevisionOrigin | null,
      SettingsRevisionOrigin | string | null | undefined,
      never
    >;
  };
  settingsHeads: {
    namespace: string;
    scopeKind: string;
    scopeId: string;
    revision: string;
  };
}

/** Register this ordinary migration in each product's existing migration provider. */
export const settingsMigration: Migration = {
  async up(db: Kysely<any>) {
    await db.schema
      .createTable('settingsDefinitions')
      .addColumn('namespace', 'text', (col) => col.notNull())
      .addColumn('definitionRevision', 'text', (col) => col.notNull())
      .addColumn('jsonSchema', 'jsonb', (col) => col.notNull())
      .addColumn('defaults', 'jsonb', (col) => col.notNull())
      .addColumn('metadata', 'jsonb', (col) => col.notNull())
      .addColumn('recordedAt', sql`timestamptz(6)`, (col) =>
        col.notNull().defaultTo(sql`clock_timestamp()`),
      )
      .addPrimaryKeyConstraint('settingsDefinitionsPk', [
        'namespace',
        'definitionRevision',
      ])
      .execute();
    await db.schema
      .createTable('settingsChanges')
      .addColumn('operationId', 'text', (col) => col.primaryKey())
      .addColumn('namespace', 'text', (col) => col.notNull())
      .addColumn('scopeKind', 'text', (col) => col.notNull())
      .addColumn('scopeId', 'text', (col) => col.notNull())
      .addColumn('actorRef', 'text', (col) => col.notNull())
      .addColumn('requestDigest', 'text', (col) => col.notNull())
      .addColumn('result', 'jsonb', (col) => col.notNull())
      .addColumn('recordedAt', sql`timestamptz(6)`, (col) =>
        col.notNull().defaultTo(sql`clock_timestamp()`),
      )
      .execute();
    await db.schema
      .createTable('settingsRevisions')
      .addColumn('namespace', 'text', (col) => col.notNull())
      .addColumn('scopeKind', 'text', (col) => col.notNull())
      .addColumn('scopeId', 'text', (col) => col.notNull())
      .addColumn('revision', 'bigint', (col) => col.notNull())
      .addColumn('definitionRevision', 'text', (col) => col.notNull())
      .addColumn('sparse', 'jsonb', (col) => col.notNull())
      .addColumn('patch', 'jsonb', (col) => col.notNull())
      .addColumn('actor', 'jsonb', (col) => col.notNull())
      .addColumn('reason', 'text')
      .addColumn('origin', 'jsonb')
      .addColumn('recordedAt', sql`timestamptz(6)`, (col) =>
        col.notNull().defaultTo(sql`clock_timestamp()`),
      )
      .addPrimaryKeyConstraint('settingsRevisionsPk', [
        'namespace',
        'scopeKind',
        'scopeId',
        'revision',
      ])
      .addForeignKeyConstraint(
        'settingsRevisionDefinitionFk',
        ['namespace', 'definitionRevision'],
        'settingsDefinitions',
        ['namespace', 'definitionRevision'],
      )
      .addCheckConstraint('settingsRevisionPositive', sql`revision > 0`)
      .addCheckConstraint(
        'settingsRevisionScope',
        sql`scope_kind in ('installation','subject','role')`,
      )
      .execute();
    await db.schema
      .createTable('settingsHeads')
      .addColumn('namespace', 'text', (col) => col.notNull())
      .addColumn('scopeKind', 'text', (col) => col.notNull())
      .addColumn('scopeId', 'text', (col) => col.notNull())
      .addColumn('revision', 'bigint', (col) => col.notNull())
      .addPrimaryKeyConstraint('settingsHeadsPk', [
        'namespace',
        'scopeKind',
        'scopeId',
      ])
      .addForeignKeyConstraint(
        'settingsHeadRevisionFk',
        ['namespace', 'scopeKind', 'scopeId', 'revision'],
        'settingsRevisions',
        ['namespace', 'scopeKind', 'scopeId', 'revision'],
      )
      .addCheckConstraint('settingsHeadPositive', sql`revision > 0`)
      .execute();
    // Revisions are inserted before a head is advanced; no deferred writes are required.
  },
  async down(db: Kysely<any>) {
    for (const table of [
      'settingsHeads',
      'settingsRevisions',
      'settingsChanges',
      'settingsDefinitions',
    ])
      await db.schema.dropTable(table).execute();
  },
};

/** Apply after the initial settings migration in installations created earlier. */
export const settingsOriginMigration: Migration = {
  async up(db) {
    await sql`alter table settings_revisions add column if not exists origin jsonb`.execute(
      db,
    );
  },
  async down(db) {
    await sql`alter table settings_revisions drop column if exists origin`.execute(
      db,
    );
  },
};
