import { z } from 'zod';
import { settingsLifecycle } from './lifecycle.js';
// Definition/revision/activation separation adapted from Heptalogos 21f00804
// packages/system/configuration; Copyright 2026 Aravine Zhu, AGPL-3.0-only.
import {
  JsonValueSchema,
  type CallContext,
  type Problem,
} from '@nous-wave/contracts/common';
import type { Kysely } from 'kysely';
import type { Client } from 'pg';
import jsonpatch from 'fast-json-patch';
import { problem, ProblemError } from '../errors.js';
import { sql } from '../storage/index.js';
import {
  at,
  assertSparse,
  createCatalog,
  digest,
  immutable,
  parse,
  scopeId,
  scopeKey,
  scopes,
} from './catalog.js';
import { evaluate, patchSparse, resetSparse, type Layer } from './evaluate.js';
import { createRuntimeState } from './runtime.js';
import {
  ConfigScopeSchema,
  SettingsChangeSchema,
  SettingsDescribeSchema,
  SettingsHistorySchema,
  SettingsReadSchema,
  SettingsResetSchema,
  SettingsRestoreSchema,
} from './types.js';
import type {
  ConfigScope,
  EffectiveSettingsView,
  JsonObject,
  ModuleDescription,
  PatchOperation,
  SettingsAuthorize,
  SettingsChange,
  SettingsChangeResult,
  SettingsModule,
  SettingsReset,
  SettingsRestore,
  SettingsRevision,
} from './types.js';
import type { SettingsDatabase } from './migration.js';
export { settingsMigration, settingsOriginMigration } from './migration.js';
export type { SettingsDatabase } from './migration.js';
export * from './types.js';
export { evaluate } from './evaluate.js';

export interface SettingsOptions {
  installationId: string;
  db: Kysely<SettingsDatabase>;
  modules: readonly SettingsModule[];
  authorize: SettingsAuthorize;
  onError?: (problem: Problem) => void;
}
const json = <T>(value: T) => sql<T>`${JSON.stringify(value)}::jsonb`;
function revision(value: string | undefined): number {
  const result = value === undefined ? 0 : Number(value);
  if (!Number.isSafeInteger(result) || result < 0)
    throw problem(
      'settings.invalid_revision',
      'failed',
      '配置修订超出有效范围',
    );
  return result;
}

/** One product's static settings definitions and transactional sparse overrides. */
export function createSettings(options: SettingsOptions) {
  const { db, authorize, installationId } = options;
  const catalog = createCatalog(options.modules);
  const runtime = createRuntimeState(catalog, options.onError);
  function selection(scope: ConfigScope): ConfigScope[] {
    return scopes(installationId, parse(ConfigScopeSchema, scope));
  }
  type ReadSelection = { namespaces: string[]; scope: ConfigScope };
  async function loadMany(
    reads: ReadSelection[],
    database = db,
  ): Promise<Array<Map<string, Layer[]>>> {
    const plans = reads.map((input) => {
      const namespaces = [...new Set(input.namespaces)],
        chain = selection(input.scope);
      for (const namespace of namespaces) catalog.module(namespace);
      return { namespaces, chain };
    });
    const names = [...new Set(plans.flatMap((plan) => plan.namespaces))];
    if (!names.length) return plans.map(() => new Map());
    const requestedScopes = new Map(
      plans
        .flatMap((plan) => plan.chain)
        .map((scope) => [scopeKey(scope), scope]),
    );
    // All requested namespace heads are selected by one PostgreSQL statement.
    const rows = await database
      .selectFrom('settingsHeads as head')
      .innerJoin('settingsRevisions as revision', (join) =>
        join
          .onRef('head.namespace', '=', 'revision.namespace')
          .onRef('head.scopeKind', '=', 'revision.scopeKind')
          .onRef('head.scopeId', '=', 'revision.scopeId')
          .onRef('head.revision', '=', 'revision.revision'),
      )
      .select([
        'head.namespace',
        'head.scopeKind',
        'head.scopeId',
        'head.revision',
        'revision.sparse',
      ])
      .where('head.namespace', 'in', names)
      .where((eb) =>
        eb.or(
          [...requestedScopes.values()].map((scope) =>
            eb.and([
              eb('head.scopeKind', '=', scope.kind),
              eb('head.scopeId', '=', scopeId(scope)),
            ]),
          ),
        ),
      )
      .execute();
    return plans.map(
      (plan) =>
        new Map(
          plan.namespaces.map((namespace) => [
            namespace,
            plan.chain.map((scope) => {
              const row = rows.find(
                (row) =>
                  row.namespace === namespace &&
                  row.scopeKind === scope.kind &&
                  row.scopeId === scopeId(scope),
              );
              return {
                ref: { namespace, scope, revision: revision(row?.revision) },
                sparse: row?.sparse ?? {},
              };
            }),
          ]),
        ),
    );
  }
  async function load(
    namespaces: readonly string[],
    target: ConfigScope,
    database = db,
  ) {
    return (
      await loadMany([{ namespaces: [...namespaces], scope: target }], database)
    )[0];
  }
  function settingsView(
    input: ReadSelection,
    layers: Map<string, Layer[]>,
  ): EffectiveSettingsView {
    return immutable({
      scope: structuredClone(input.scope),
      modules: Object.fromEntries(
        [...layers].map(([namespace, rows]) => [
          namespace,
          evaluate(catalog.module(namespace), rows),
        ]),
      ),
      targetOverrides: Object.fromEntries(
        [...layers].map(([namespace, rows]) => [
          namespace,
          {
            revision: rows.at(-1)!.ref.revision,
            sparse: structuredClone(rows.at(-1)!.sparse),
          },
        ]),
      ),
    });
  }
  async function readUnchecked(
    input: ReadSelection,
    database = db,
  ): Promise<EffectiveSettingsView> {
    return settingsView(
      input,
      await load(input.namespaces, input.scope, database),
    );
  }
  const registeredReads = new WeakMap<
    object,
    { read: ReadSelection; active: () => void }
  >();
  async function snapshotConsumers(
    handles: readonly ReturnType<typeof runtime.registerConsumer>[],
    database = db,
  ) {
    if (!handles.length || handles.length > 100)
      throw problem(
        'settings.invalid_batch',
        'validation',
        '批量配置消费需要1至100项',
      );
    const entries = handles.map((handle) => {
      const entry = registeredReads.get(handle);
      if (!entry)
        throw problem(
          'settings.consumer_unknown',
          'forbidden',
          '配置消费者不属于此owner',
        );
      entry.active();
      return entry;
    });
    const loaded = await loadMany(
      entries.map((entry) => entry.read),
      database,
    );
    return entries.map((entry, index) => {
      entry.active();
      return settingsView(entry.read, loaded[index]).modules[
        entry.read.namespaces[0]
      ];
    });
  }
  async function authorizeRead(
    ctx: CallContext,
    namespaces: readonly string[],
    scope: ConfigScope,
    action: 'read' | 'applied' = 'read',
    database = db,
  ) {
    selection(scope);
    for (const namespace of new Set(namespaces)) {
      catalog.module(namespace);
      // The effective object includes installation and subject inputs as well as the target layer.
      await authorize(ctx, { action, namespace, scope }, database);
    }
  }
  async function definition(
    namespace: string,
    definitionRevision: string,
    database = db,
  ): Promise<ModuleDescription> {
    const row = await database
      .selectFrom('settingsDefinitions')
      .selectAll()
      .where('namespace', '=', namespace)
      .where('definitionRevision', '=', definitionRevision)
      .executeTakeFirst();
    if (!row)
      throw problem(
        'settings.definition_not_found',
        'not_found',
        '设置定义版本不存在',
      );
    return immutable({
      namespace,
      definitionRevision,
      jsonSchema: row.jsonSchema,
      defaults: row.defaults,
      owner: row.metadata.owner,
      fields: row.metadata.fields,
      recordedAt: row.recordedAt,
    });
  }
  async function archivedRevision(
    namespace: string,
    scope: ConfigScope,
    revisionId: number,
    database = db,
  ): Promise<SettingsRevision> {
    selection(scope);
    catalog.module(namespace);
    if (!Number.isSafeInteger(revisionId) || revisionId < 1)
      throw problem(
        'settings.invalid_revision',
        'validation',
        '配置修订必须是正整数',
      );
    const row = await database
      .selectFrom('settingsRevisions')
      .selectAll()
      .where('namespace', '=', namespace)
      .where('scopeKind', '=', scope.kind)
      .where('scopeId', '=', scopeId(scope))
      .where('revision', '=', String(revisionId))
      .executeTakeFirst();
    if (!row)
      throw problem(
        'settings.revision_not_found',
        'not_found',
        '配置历史修订不存在',
      );
    return immutable({
      namespace,
      scope: structuredClone(scope),
      revision: revisionId,
      definitionRevision: row.definitionRevision,
      sparse: row.sparse,
      patch: row.patch,
      actor: row.actor,
      recordedAt: row.recordedAt,
      ...(row.reason === null ? {} : { reason: row.reason }),
      ...(row.origin ? { origin: row.origin } : {}),
      definition: await definition(namespace, row.definitionRevision, database),
    });
  }
  async function archivedSnapshot(
    namespace: string,
    scope: ConfigScope,
    revisionId: number,
    database = db,
  ) {
    if (scope.kind !== 'installation')
      throw problem(
        'settings.historical_scope',
        'validation',
        '精确历史快照只适用于安装层资源目录',
      );
    const row = await archivedRevision(namespace, scope, revisionId, database),
      description = row.definition;
    return evaluate(
      {
        id: namespace,
        definitionRevision: row.definitionRevision,
        owner: description.owner,
        schema: z.record(z.string(), JsonValueSchema),
        defaults: description.defaults,
        fields: description.fields,
      },
      [{ ref: { namespace, scope, revision: revisionId }, sparse: row.sparse }],
    );
  }
  async function registerDefinitions(): Promise<void> {
    await db.transaction().execute(async (tx) => {
      for (const namespace of [...catalog.ids].sort()) {
        const item = catalog.description(namespace);
        await tx
          .insertInto('settingsDefinitions')
          .values({
            namespace,
            definitionRevision: item.definitionRevision,
            jsonSchema: json(item.jsonSchema),
            defaults: json(item.defaults),
            metadata: json({ owner: item.owner, fields: item.fields }),
          })
          .onConflict((oc) =>
            oc.columns(['namespace', 'definitionRevision']).doNothing(),
          )
          .execute();
        const saved = await definition(namespace, item.definitionRevision, tx);
        if (
          digest({
            jsonSchema: saved.jsonSchema,
            defaults: saved.defaults,
            owner: saved.owner,
            fields: saved.fields,
          }) !==
          digest({
            jsonSchema: item.jsonSchema,
            defaults: item.defaults,
            owner: item.owner,
            fields: item.fields,
          })
        ) {
          throw problem(
            'settings.definition_revision_conflict',
            'conflict',
            '同一设置定义版本包含不同内容',
            {
              details: {
                namespace,
                definitionRevision: item.definitionRevision,
              },
            },
          );
        }
      }
    });
  }

  async function mutate(
    ctx: CallContext,
    action: 'update' | 'reset' | 'restore',
    input: SettingsChange | SettingsReset | SettingsRestore,
    database?: Kysely<SettingsDatabase>,
  ): Promise<SettingsChangeResult> {
    const module = catalog.module(input.namespace);
    const chain = selection(input.scope);
    const paths =
      'patch' in input
        ? input.patch.flatMap((op) =>
            'from' in op ? [op.path, op.from] : [op.path],
          )
        : 'paths' in input
          ? input.paths
          : undefined;
    if (!ctx.trace.operationId || !ctx.actor.authorityRef)
      throw problem(
        'settings.context_required',
        'forbidden',
        '设置修改需要受信提交身份',
      );
    const operationId = ctx.trace.operationId;
    const requestDigest = digest({
      action,
      input,
      actor: ctx.actor,
      consumer: ctx.consumer,
      subjectId: ctx.subjectId ?? null,
      purpose: ctx.purpose,
      audience: ctx.audience,
    });
    if (database && !database.isTransaction)
      throw problem(
        'settings.transaction_required',
        'validation',
        '设置写入client必须是已建立的事务',
      );
    const execute = <T>(
      use: (client: Kysely<SettingsDatabase>) => Promise<T>,
    ) => (database ? use(database) : db.transaction().execute(use));
    const committed = await execute(async (tx) => {
      await authorize(
        ctx,
        {
          action,
          namespace: input.namespace,
          scope: input.scope,
          paths,
        },
        tx,
      );
      await sql`select pg_advisory_xact_lock(hashtextextended(${`settings:operation:${operationId}`}, 0))`.execute(
        tx,
      );
      const previous = await tx
        .selectFrom('settingsChanges')
        .selectAll()
        .where('operationId', '=', operationId)
        .executeTakeFirst();
      if (previous) {
        if (previous.requestDigest !== requestDigest)
          throw problem(
            'settings.submission_conflict',
            'conflict',
            '同一提交身份包含不同设置修改',
          );
        return { result: immutable(previous.result), replayed: true };
      }
      // Shared ancestor locks keep this validation stable while allowing unrelated subjects to write.
      for (const scope of chain) {
        const key = `settings:scope:${input.namespace}:${scopeKey(scope)}`;
        if (scopeKey(scope) === scopeKey(input.scope))
          await sql`select pg_advisory_xact_lock(hashtextextended(${key}, 0))`.execute(
            tx,
          );
        else
          await sql`select pg_advisory_xact_lock_shared(hashtextextended(${key}, 0))`.execute(
            tx,
          );
      }
      const headQuery = tx
        .selectFrom('settingsHeads')
        .selectAll()
        .where('namespace', '=', input.namespace)
        .where('scopeKind', '=', input.scope.kind)
        .where('scopeId', '=', scopeId(input.scope));
      const head = await headQuery.forUpdate().executeTakeFirst();
      const current = revision(head?.revision);
      if (current !== input.expectedRevision)
        throw problem(
          'settings.revision_conflict',
          'conflict',
          '设置已有新修订',
          { currentRevision: current },
        );
      const layers = (await load([input.namespace], input.scope, tx)).get(
        input.namespace,
      )!;
      const own = layers.at(-1)!;
      let sparse: JsonObject;
      let patch: PatchOperation[];
      if (action === 'restore') {
        const source = await tx
          .selectFrom('settingsRevisions')
          .select(['sparse'])
          .where('namespace', '=', input.namespace)
          .where('scopeKind', '=', input.scope.kind)
          .where('scopeId', '=', scopeId(input.scope))
          .where('revision', '=', String((input as SettingsRestore).revision))
          .executeTakeFirst();
        if (!source)
          throw problem(
            'settings.revision_not_found',
            'not_found',
            '设置历史修订不存在',
          );
        assertSparse(module, source.sparse);
        sparse = structuredClone(source.sparse);
        patch = jsonpatch.compare(own.sparse, sparse) as PatchOperation[];
        sparse = patchSparse(module, input.scope, own.sparse, patch);
      } else if (action === 'reset') {
        const reset = resetSparse(
          module,
          input.scope,
          own.sparse,
          (input as SettingsReset).paths,
        );
        sparse = reset.sparse;
        patch = reset.patch;
      } else {
        patch = (input as SettingsChange).patch;
        sparse = patchSparse(module, input.scope, own.sparse, patch);
      }
      const changed = digest(sparse) !== digest(own.sparse);
      const next = current + (changed ? 1 : 0);
      if (!Number.isSafeInteger(next))
        throw problem(
          'settings.invalid_revision',
          'failed',
          '配置修订超出有效范围',
        );
      let before: ReturnType<typeof evaluate> | undefined;
      try {
        before = evaluate(module, layers);
      } catch (error) {
        // A reset may repair an overlay left invalid by a changed code-owned definition.
        if (
          !(error instanceof ProblemError) ||
          error.problem.category !== 'validation'
        )
          throw error;
      }
      own.sparse = sparse;
      own.ref.revision = next;
      const snapshot = evaluate(module, layers);
      if (module.validate)
        await module.validate(snapshot.values, {
          ctx,
          namespace: input.namespace,
          scope: input.scope,
          db: tx,
        });
      if (changed) {
        await tx
          .insertInto('settingsRevisions')
          .values({
            namespace: input.namespace,
            scopeKind: input.scope.kind,
            scopeId: scopeId(input.scope),
            revision: String(next),
            definitionRevision: module.definitionRevision,
            sparse: json(sparse),
            patch: json(patch),
            actor: json(ctx.actor),
            reason: input.reason ?? null,
          })
          .execute();
        if (head) {
          const advanced = await tx
            .updateTable('settingsHeads')
            .set({ revision: String(next) })
            .where('namespace', '=', input.namespace)
            .where('scopeKind', '=', input.scope.kind)
            .where('scopeId', '=', scopeId(input.scope))
            .where('revision', '=', head.revision)
            .returning('revision')
            .executeTakeFirst();
          if (!advanced)
            throw problem(
              'settings.revision_conflict',
              'conflict',
              '设置已有新修订',
            );
        } else {
          const inserted = await tx
            .insertInto('settingsHeads')
            .values({
              namespace: input.namespace,
              scopeKind: input.scope.kind,
              scopeId: scopeId(input.scope),
              revision: String(next),
            })
            .onConflict((oc) =>
              oc.columns(['namespace', 'scopeKind', 'scopeId']).doNothing(),
            )
            .returning('revision')
            .executeTakeFirst();
          if (!inserted)
            throw problem(
              'settings.revision_conflict',
              'conflict',
              '设置已有新修订',
            );
        }
      }
      const effects: SettingsChangeResult['effects'] = {
        hot: [],
        next_call: [],
        restart: [],
      };
      for (const [path, metadata] of Object.entries(module.fields))
        if (
          !before ||
          digest(at(before.values, path)) !== digest(at(snapshot.values, path))
        )
          effects[metadata.effect].push(path);
      const result: SettingsChangeResult = {
        namespace: input.namespace,
        scope: input.scope,
        operationId,
        revision: next,
        changed,
        snapshot,
        effects,
      };
      await tx
        .insertInto('settingsChanges')
        .values({
          operationId,
          namespace: input.namespace,
          scopeKind: input.scope.kind,
          scopeId: scopeId(input.scope),
          actorRef: ctx.actor.authorityRef,
          requestDigest,
          result: json(result),
        })
        .execute();
      if (changed)
        await sql`select pg_notify('settings_changed', ${JSON.stringify({ namespace: input.namespace, scope: input.scope, revision: next })})`.execute(
          tx,
        );
      return { result: immutable(result), replayed: false };
    });
    if (!database && !committed.replayed && committed.result.changed)
      runtime.changed({
        namespace: input.namespace,
        scope: input.scope,
        revision: committed.result.revision,
      });
    return committed.result;
  }

  async function readMany(
    ctx: CallContext,
    input: { reads: ReadSelection[] },
    database = db,
  ): Promise<EffectiveSettingsView[]> {
    if (!Array.isArray(input.reads) || input.reads.length > 100)
      throw problem(
        'settings.invalid_batch',
        'validation',
        '批量设置读取最多100项',
      );
    const reads = input.reads.map((value) => parse(SettingsReadSchema, value));
    for (const read of reads)
      await authorizeRead(ctx, read.namespaces, read.scope, 'read', database);
    const loaded = await loadMany(reads, database);
    return reads.map((read, index) => settingsView(read, loaded[index]));
  }

  return {
    ...settingsLifecycle({
      db,
      installationId,
      catalog,
      authorize,
      definition,
      load,
    }),
    registerDefinitions,
    readMany,
    snapshotConsumers,
    async describe(
      ctx: CallContext,
      input: {
        namespace?: string;
        scope?: ConfigScope;
        definitionRevision?: string;
      } = {},
      database = db,
    ) {
      input = parse(SettingsDescribeSchema, input);
      const target = input.scope ?? {
        kind: 'installation' as const,
        installationId,
      };
      selection(target);
      if (input.definitionRevision && !input.namespace)
        throw problem(
          'settings.namespace_required',
          'validation',
          '查看历史定义需要命名空间',
        );
      const names = input.namespace ? [input.namespace] : [...catalog.ids];
      const result: ModuleDescription[] = [];
      for (const namespace of names) {
        await authorize(
          ctx,
          { action: 'describe', namespace, scope: target },
          database,
        );
        result.push(
          input.definitionRevision
            ? await definition(namespace, input.definitionRevision, database)
            : catalog.description(namespace),
        );
      }
      return immutable(result);
    },
    async read(
      ctx: CallContext,
      input: { namespaces: string[]; scope: ConfigScope },
      database = db,
    ) {
      return (await readMany(ctx, { reads: [input] }, database))[0];
    },
    update: (
      ctx: CallContext,
      input: SettingsChange,
      database?: Kysely<SettingsDatabase>,
    ) => mutate(ctx, 'update', parse(SettingsChangeSchema, input), database),
    reset: (
      ctx: CallContext,
      input: SettingsReset,
      database?: Kysely<SettingsDatabase>,
    ) => mutate(ctx, 'reset', parse(SettingsResetSchema, input), database),
    restore: (
      ctx: CallContext,
      input: SettingsRestore,
      database?: Kysely<SettingsDatabase>,
    ) => mutate(ctx, 'restore', parse(SettingsRestoreSchema, input), database),
    async readRevision(
      ctx: CallContext,
      input: { namespace: string; scope: ConfigScope; revision: number },
      database = db,
    ): Promise<SettingsRevision> {
      await authorize(
        ctx,
        {
          action: 'history',
          namespace: input.namespace,
          scope: input.scope,
        },
        database,
      );
      return archivedRevision(
        input.namespace,
        input.scope,
        input.revision,
        database,
      );
    },
    async history(
      ctx: CallContext,
      input: {
        namespace: string;
        scope: ConfigScope;
        cursor?: string;
        limit?: number;
      },
      database = db,
    ): Promise<{ items: SettingsRevision[]; nextCursor?: string }> {
      input = parse(SettingsHistorySchema, input);
      selection(input.scope);
      await authorize(
        ctx,
        {
          action: 'history',
          namespace: input.namespace,
          scope: input.scope,
        },
        database,
      );
      const limit = input.limit ?? 50;
      if (!Number.isSafeInteger(limit) || limit < 1 || limit > 200)
        throw problem(
          'settings.invalid_page',
          'validation',
          '历史页大小必须为1至200',
        );
      let before: number | undefined;
      if (input.cursor) {
        try {
          const cursor = JSON.parse(
            Buffer.from(input.cursor, 'base64url').toString('utf8'),
          );
          if (
            cursor.namespace !== input.namespace ||
            cursor.scope !== scopeKey(input.scope) ||
            !Number.isSafeInteger(cursor.before) ||
            cursor.before < 1
          )
            throw new Error();
          before = cursor.before;
        } catch {
          throw problem(
            'settings.invalid_cursor',
            'validation',
            '设置历史游标无效',
          );
        }
      }
      let query = database
        .selectFrom('settingsRevisions')
        .selectAll()
        .where('namespace', '=', input.namespace)
        .where('scopeKind', '=', input.scope.kind)
        .where('scopeId', '=', scopeId(input.scope));
      if (before !== undefined)
        query = query.where('revision', '<', String(before));
      const rows = await query
        .orderBy('revision', 'desc')
        .limit(limit + 1)
        .execute();
      const items: SettingsRevision[] = [];
      const definitions = new Map<string, ModuleDescription>();
      for (const row of rows.slice(0, limit)) {
        if (!definitions.has(row.definitionRevision))
          definitions.set(
            row.definitionRevision,
            await definition(row.namespace, row.definitionRevision, database),
          );
        items.push({
          namespace: row.namespace,
          scope: structuredClone(input.scope),
          revision: revision(row.revision),
          definitionRevision: row.definitionRevision,
          sparse: row.sparse,
          patch: row.patch,
          actor: row.actor,
          ...(row.reason === null ? {} : { reason: row.reason }),
          recordedAt: row.recordedAt,
          ...(row.origin ? { origin: row.origin } : {}),
          definition: definitions.get(row.definitionRevision)!,
        });
      }
      return immutable({
        items,
        ...(rows.length > limit
          ? {
              nextCursor: Buffer.from(
                JSON.stringify({
                  namespace: input.namespace,
                  scope: scopeKey(input.scope),
                  before: items.at(-1)!.revision,
                }),
              ).toString('base64url'),
            }
          : {}),
      });
    },
    subscribe: runtime.subscribe,
    listen: (createSession: () => Client) => runtime.listen(createSession),
    registerConsumer(input: Parameters<typeof runtime.registerConsumer>[0]) {
      selection(input.scope);
      let closed = false;
      const registered = runtime.registerConsumer(
        input,
        async (database = db) =>
          (
            await readUnchecked({
              namespaces: [input.namespace],
              scope: input.scope,
            }, database)
          ).modules[input.namespace],
      );
      const active = () => {
        if (closed)
          throw problem(
            'settings.consumer_closed',
            'conflict',
            '配置消费者已关闭',
          );
      };
      const handle = {
        ...registered,
        async snapshotRevision(revision: number, database = db) {
          if (closed)
            throw problem(
              'settings.consumer_closed',
              'conflict',
              '配置消费者已关闭',
            );
          return archivedSnapshot(
            input.namespace,
            input.scope,
            revision,
            database,
          );
        },
        close() {
          closed = true;
          registered.close();
        },
      };
      registeredReads.set(handle, {
        read: { namespaces: [input.namespace], scope: input.scope },
        active,
      });
      return handle;
    },
    registerConsumers(input: {
      processBootId: string;
      consumerId: string;
      reads: { namespace: string; scope: ConfigScope }[];
    }) {
      if (!input.reads.length || input.reads.length > 100)
        throw problem(
          'settings.invalid_batch',
          'validation',
          '配置消费组需要1至100项',
        );
      const keys = input.reads.map((read) =>
        digest([read.namespace, read.scope]),
      );
      if (new Set(keys).size !== keys.length)
        throw problem(
          'settings.consumer_conflict',
          'conflict',
          '配置消费组不能重复注册同一项',
        );
      const registrations: ReturnType<typeof runtime.registerConsumer>[] = [];
      try {
        for (const read of input.reads) {
          selection(read.scope);
          registrations.push(
            runtime.registerConsumer(
              { ...input, ...read },
              async (database = db) =>
                (
                  await readUnchecked({
                    namespaces: [read.namespace],
                    scope: read.scope,
                  }, database)
                ).modules[read.namespace],
            ),
          );
        }
      } catch (error) {
        for (const item of registrations) item.close();
        throw error;
      }
      let closed = false;
      const active = () => {
        if (closed)
          throw problem(
            'settings.consumer_closed',
            'conflict',
            '配置消费组已关闭',
          );
      };
      return {
        async snapshot(database = db) {
          active();
          const reads = input.reads.map((read) => ({
            namespaces: [read.namespace],
            scope: read.scope,
          }));
          const loaded = await loadMany(reads, database);
          active();
          return reads.map(
            (read, index) =>
              settingsView(read, loaded[index]).modules[read.namespaces[0]],
          );
        },
        reportApplied(
          snapshots: import('./types.js').ConfigSnapshot[],
          paths?: readonly (readonly string[])[],
        ) {
          active();
          if (snapshots.length !== registrations.length)
            throw problem(
              'settings.snapshot_mismatch',
              'validation',
              '配置消费组快照数量不符',
            );
          registrations.forEach((item, index) =>
            item.reportApplied(snapshots[index], paths?.[index]),
          );
        },
        reportFailure(error: Problem) {
          active();
          for (const item of registrations) item.reportFailure(error);
        },
        close() {
          if (closed) return;
          closed = true;
          for (const item of registrations) item.close();
        },
      };
    },
    async applied(
      ctx: CallContext,
      input: { namespaces: string[]; scope: ConfigScope },
      database = db,
    ) {
      input = parse(SettingsReadSchema, input);
      await authorizeRead(
        ctx,
        input.namespaces,
        input.scope,
        'applied',
        database,
      );
      return runtime.applied(await readUnchecked(input, database));
    },
  };
}

export type Settings = ReturnType<typeof createSettings>;
