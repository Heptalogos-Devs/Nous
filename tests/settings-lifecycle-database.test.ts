import { test } from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { z } from 'zod';
import { createStorage, sql, type Kysely } from '@nous-wave/platform/storage';
import {
  createSettings,
  settingsMigration,
  type SettingsDatabase,
  type SettingsModule,
} from '@nous-wave/platform/settings';
import type { CallContext } from '@nous-wave/contracts/common';

// One actual configuration history crosses a Subject bundle, restores and rolls back.
test('settings: portable history, current import actor, transaction authorization and existing consumer snapshots', {
  skip: !process.env.NOUS_SETTINGS_TEST_DATABASE_URL,
  timeout: 20000,
}, async () => {
  const connectionString = process.env.NOUS_SETTINGS_TEST_DATABASE_URL!,
    schema = `settings_${randomUUID().replaceAll('-', '')}`;
  const admin = createStorage<SettingsDatabase>({
    pool: { connectionString },
    datasourceName: `${schema}-admin`,
    runDatasourceMigrations: false,
  });
  await admin.db.schema.createSchema(schema).execute();
  const storage = createStorage<SettingsDatabase>({
    pool: { connectionString, options: `-c search_path=${schema}`, max: 1 },
    datasourceName: schema,
    runDatasourceMigrations: false,
  });
  const module: SettingsModule = {
    id: 'history',
    owner: 'test-owner',
    definitionRevision: '1',
    schema: z.strictObject({
      limit: z.number().positive(),
      caption: z.string(),
    }),
    defaults: { limit: 100, caption: 'initial' },
    fields: {
      '/limit': {
        labelKey: 'history.limit',
        descriptionKey: 'history.limit.description',
        scopes: ['installation', 'subject', 'role'],
        visibility: 'standard',
        effect: 'next_call',
        restriction: 'upper_bound',
        consumerIds: ['history'],
      },
      '/caption': {
        labelKey: 'history.caption',
        descriptionKey: 'history.caption.description',
        scopes: ['installation', 'subject', 'role'],
        visibility: 'standard',
        effect: 'next_call',
        consumerIds: ['history'],
      },
    },
  };
  const source = {
      kind: 'subject' as const,
      subjectId: '019af306-5547-7abc-8000-000000000001',
    },
    target = {
      kind: 'subject' as const,
      subjectId: '019af306-5547-7abc-8000-000000000002',
    };
  const ctx: CallContext = {
    actor: {
      id: 'source-operator',
      kind: 'service',
      authorityRef: 'protected-process:history',
    },
    consumer: { kind: 'maintenance', host: 'nous', id: schema },
    purpose: 'settings-history',
    audience: [],
    trace: { traceId: 'e'.repeat(32), operationId: 'source/1' },
  };
  const operation = (operationId: string): CallContext => ({
    ...ctx,
    trace: { ...ctx.trace, operationId },
  });
  let inside = false,
    checks = 0;
  const settings = createSettings({
    db: storage.db,
    installationId: schema,
    modules: [module],
    async authorize(context, request, client) {
      assert.equal(context.actor.authorityRef, ctx.actor.authorityRef);
      assert.equal(request.namespace, 'history');
      assert.ok(client);
      checks++;
      if (inside) {
        assert.equal(client.isTransaction, true);
        const probe = await sql<{
          marker: string;
        }>`select marker from authorization_probe`.execute(client);
        assert.equal(probe.rows[0].marker, 'uncommitted');
      } else await sql`select 1`.execute(client);
    },
  });
  try {
    await settingsMigration.up(storage.db as Kysely<unknown>);
    await settings.registerDefinitions();
    await sql`create table authorization_probe(marker text)`.execute(
      storage.db,
    );
    for (let i = 1; i <= 3; i++)
      await settings.update(operation(`source/${i}`), {
        namespace: 'history',
        scope: source,
        expectedRevision: i - 1,
        patch: [
          { op: 'add', path: '/limit', value: 30 - i * 5 },
          { op: 'add', path: '/caption', value: `source-${i}` },
        ],
      });
    const role = {
      kind: 'role' as const,
      subjectId: source.subjectId,
      roleId: 'worker',
    };
    await settings.update(operation('role/1'), {
      namespace: 'history',
      scope: role,
      expectedRevision: 0,
      patch: [{ op: 'add', path: '/limit', value: 12 }],
    });
    const first = settings.registerConsumer({
        processBootId: schema,
        consumerId: 'history',
        namespace: 'history',
        scope: source,
      }),
      second = settings.registerConsumer({
        processBootId: schema,
        consumerId: 'history',
        namespace: 'history',
        scope: role,
      });
    const pair = await settings.snapshotConsumers([first, second]);
    assert.equal(pair[0].values.limit, 15);
    assert.equal(pair[1].values.limit, 12);
    first.reportApplied(pair[0]);
    second.reportApplied(pair[1]);
    first.close();
    await assert.rejects(settings.snapshotConsumers([first, second]), {
      code: 'settings.consumer_closed',
    });
    second.close();
    const bundle = await settings.export(ctx, { scopes: [source] });
    assert.equal(bundle.revisions.length, 4);
    assert.equal(bundle.heads.length, 2);
    const importedCtx = {
        ...operation('import/one'),
        actor: { ...ctx.actor, id: 'actual-import-operator' },
      },
      input = {
        bundle,
        scopeMap: [
          { from: source, to: target, expectedRevisions: { history: 0 } },
        ],
      };
    const imported = await settings.import(importedCtx, input);
    assert.deepEqual(
      (await settings.import(importedCtx, input)).heads,
      imported.heads,
    );
    const history = await settings.history(ctx, {
      namespace: 'history',
      scope: target,
    });
    assert.equal(history.items.length, 3);
    assert.equal(history.items[0].actor.id, 'actual-import-operator');
    assert.equal(history.items[0].origin!.actor.id, 'source-operator');
    assert.equal(history.items[0].origin!.revision, 3);
    await settings.restore(operation('restore/target'), {
      namespace: 'history',
      scope: target,
      revision: 1,
      expectedRevision: 3,
    });
    assert.equal(
      (await settings.read(ctx, { namespaces: ['history'], scope: target }))
        .modules.history.values.caption,
      'source-1',
    );
    const before = (
      await settings.history(ctx, { namespace: 'history', scope: target })
    ).items.length;
    await assert.rejects(
      storage.db.transaction().execute(async (tx) => {
        await sql`insert into authorization_probe values ('uncommitted')`.execute(
          tx,
        );
        inside = true;
        await settings.describe(ctx, { namespace: 'history' }, tx);
        await settings.readMany(
          ctx,
          { reads: [{ namespaces: ['history'], scope: target }] },
          tx,
        );
        await settings.history(
          ctx,
          { namespace: 'history', scope: target },
          tx,
        );
        await settings.readRevision(
          ctx,
          { namespace: 'history', scope: target, revision: 1 },
          tx,
        );
        await settings.applied(
          ctx,
          { namespaces: ['history'], scope: target },
          tx,
        );
        await settings.export(ctx, { scopes: [target] }, tx);
        await settings.import(
          operation('rollback-import'),
          {
            bundle,
            scopeMap: [
              { from: source, to: target, expectedRevisions: { history: 4 } },
              {
                from: role,
                to: { ...role, subjectId: target.subjectId },
                expectedRevisions: { history: 1 },
              },
            ],
          },
          tx,
        );
        await settings.deleteScopes(ctx, { scopes: [target] }, tx);
        throw new Error('rollback requested');
      }),
      { message: 'rollback requested' },
    );
    inside = false;
    assert.equal(
      (await settings.history(ctx, { namespace: 'history', scope: target }))
        .items.length,
      before,
    );
    await settings.deleteScopes(ctx, { scopes: [target] });
    assert.equal(
      (await settings.history(ctx, { namespace: 'history', scope: target }))
        .items.length,
      0,
    );
    assert.equal(
      (
        await settings.history(ctx, {
          namespace: 'history',
          scope: { ...role, subjectId: target.subjectId },
        })
      ).items.length,
      0,
    );
    assert.ok(checks > 20);
  } finally {
    await storage.close();
    await admin.db.schema.dropSchema(schema).cascade().execute();
    await admin.close();
  }
});
