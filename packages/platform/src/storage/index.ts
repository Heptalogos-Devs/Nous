// Adapted from Nous-Wave platform/database.ts; Copyright 2026 Aravine Zhu.
import { createRequire } from 'node:module';
import { KyselyDataSource } from '@dbos-inc/kysely-datasource';
import type { Kysely } from 'kysely';
import type { MigrationProvider, MigrationResultSet } from 'kysely/migration';
import { Migrator } from 'kysely/migration';
import type { Client, PoolConfig } from 'pg';
import { Pool } from 'pg';
import type { ProblemError } from '../errors.js';
import { postgresProblem, postgresTypes, StorageClient } from './postgres.js';

// Match the constructor actually imported by the official CJS datasource.
const require = createRequire(import.meta.url);
const requireDatasource = createRequire(
  require.resolve('@dbos-inc/kysely-datasource'),
);
const kysely = requireDatasource('kysely') as typeof import('kysely');

export const sql = kysely.sql;
export { registerProblemSerialization } from './serialization.js';
export { dbosSystemDatabaseURL, resumeDurableWorkflow, describeDurableWorkflow, describeDurableSteps } from './dbos.js';
export { KyselyDataSource } from '@dbos-inc/kysely-datasource';
export type {
  ColumnType,
  Generated,
  Insertable,
  IsolationLevel,
  Kysely,
  Selectable,
  Transaction,
  Updateable,
} from 'kysely';
export type {
  Migration,
  MigrationProvider,
  MigrationResultSet,
} from 'kysely/migration';
export type { PoolConfig } from 'pg';

export interface StorageOptions {
  pool: PoolConfig;
  datasourceName: string;
  runDatasourceMigrations?: boolean;
  onError?: (error: ProblemError) => void;
}

export interface Storage<DB> {
  readonly pool: Pool;
  readonly db: Kysely<DB>;
  readonly datasource: KyselyDataSource<DB>;
  createSession(): Client;
  migrate(provider: MigrationProvider): Promise<MigrationResultSet>;
  close(): Promise<void>;
}

/** Compose one product's storage. Connections/migrations/DBOS launch are explicit. */
export function createStorage<DB>(options: StorageOptions): Storage<DB> {
  const pool = new Pool({
    ...options.pool,
    Client: StorageClient,
    log: undefined,
    types: postgresTypes(options.pool.types),
  });
  pool.on('error', (error) =>
    options.onError?.(postgresProblem(error, 'connect')),
  );
  const db = new kysely.Kysely<DB>({
    dialect: new kysely.PostgresDialect({ pool }),
    plugins: [new kysely.CamelCasePlugin({ maintainNestedObjectKeys: true })],
  });
  // 5.3 accepts schemaName but its checkpoint queries hard-code dbos.
  const datasource = new KyselyDataSource<DB>(
    options.datasourceName,
    db,
    'dbos',
    {
      runMigrations: options.runDatasourceMigrations,
    },
  );
  const sessions = new Set<Client>();
  let closing: Promise<void> | undefined;
  return {
    pool,
    db,
    datasource,
    createSession() {
      if (closing) throw postgresProblem({ code: '08003' }, 'connect');
      const client = new StorageClient({
        ...options.pool,
        types: postgresTypes(options.pool.types),
      });
      sessions.add(client);
      client.once('end', () => sessions.delete(client));
      client.on('error', (error) =>
        options.onError?.(postgresProblem(error, 'connect')),
      );
      return client;
    },
    async migrate(provider) {
      const result = await new Migrator({ db, provider }).migrateToLatest();
      return result.error
        ? { ...result, error: postgresProblem(result.error, 'migration') }
        : result;
    },
    close: () =>
      (closing ??= (async () => {
        try {
          const released = await Promise.allSettled(
            [...sessions].map((client) => client.end()),
          );
          await db.destroy();
          // Kysely does not destroy a Pool it has never initialized; auth may use it.
          if (!pool.ending) await pool.end();
          const failed = released.find(
            (result) => result.status === 'rejected',
          );
          if (failed?.status === 'rejected') throw failed.reason;
        } catch (error) {
          throw postgresProblem(error, 'close');
        }
      })()),
  };
}
