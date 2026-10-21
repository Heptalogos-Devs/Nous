// Settings owns portable revisions and their import provenance; products pass
// their actual Kysely transaction when composing a larger lifecycle operation.
import { z } from 'zod';
import jsonpatch from 'fast-json-patch';
import type { CallContext } from '@nous-wave/contracts/common';
import {
  ConfigScopeSchema,
  SettingsBundleSchema,
  SettingsScopeMapSchema,
  type SettingsBundle,
  type SettingsScopeMap,
  type SettingsRevision,
  type SettingsRevisionOrigin,
  type ConfigScope,
  type SettingsChangeResult,
} from '@nous-wave/contracts/settings';
import { sql, type Kysely } from '../storage/index.js';
import { problem, ProblemError } from '../errors.js';
import {
  at,
  assertJson,
  createCatalog,
  digest,
  immutable,
  scopeId,
  scopeKey,
  scopes,
} from './catalog.js';
import { evaluate, patchSparse, type Layer } from './evaluate.js';
import type { SettingsAuthorize } from './types.js';
import type { Selectable } from '../storage/index.js';
import type { SettingsDatabase } from './migration.js';
import type { ModuleDescription, JsonObject, PatchOperation } from './types.js';
const json = <T>(value: T) => sql<T>`${JSON.stringify(value)}::jsonb`;
const key = (namespace: string, scope: ConfigScope) =>
  digest([namespace, scope]);
const number = (value: string) => {
  const n = Number(value);
  if (!Number.isSafeInteger(n) || n < 0)
    throw problem(
      'settings.invalid_revision',
      'failed',
      '配置修订超出有效范围',
    );
  return n;
};
interface LifecycleOptions {
  db: Kysely<SettingsDatabase>;
  installationId: string;
  catalog: ReturnType<typeof createCatalog>;
  authorize: SettingsAuthorize;
  definition: (
    namespace: string,
    revision: string,
    db?: Kysely<SettingsDatabase>,
  ) => Promise<ModuleDescription>;
  load: (
    namespaces: readonly string[],
    scope: ConfigScope,
    db?: Kysely<SettingsDatabase>,
  ) => Promise<Map<string, Layer[]>>;
}
export function settingsLifecycle(options: LifecycleOptions) {
  const { db, catalog, authorize, installationId } = options;
  function currentScope(scope: ConfigScope) {
    const value = ConfigScopeSchema.parse(scope);
    scopes(installationId, value);
    return value;
  }
  async function within<T>(
    client: Kysely<SettingsDatabase> | undefined,
    use: (tx: Kysely<SettingsDatabase>) => Promise<T>,
    readOnly = false,
  ): Promise<T> {
    if (client) {
      if (!client.isTransaction)
        throw problem(
          'settings.transaction_required',
          'validation',
          '生命周期client必须是已建立的事务',
        );
      return use(client);
    }
    const transaction = db.transaction();
    return (
      readOnly ? transaction.setIsolationLevel('repeatable read') : transaction
    ).execute(use);
  }
  async function closure(
    requested: ConfigScope[],
    client: Kysely<SettingsDatabase>,
  ) {
    if (!requested.length || requested.length > 1000)
      throw problem(
        'settings.invalid_scopes',
        'validation',
        '配置生命周期需要1至1000个范围',
      );
    const result = new Map(
      requested.map((scope) => [scopeKey(currentScope(scope)), scope]),
    );
    for (const parent of requested) {
      if (parent.kind !== 'subject') continue;
      const rows = await client
        .selectFrom('settingsRevisions')
        .select('scopeId')
        .distinct()
        .where('scopeKind', '=', 'role')
        .where('scopeId', 'like', `["${parent.subjectId}",%`)
        .execute();
      for (const row of rows) {
        const value = JSON.parse(row.scopeId) as [string, string];
        const scope: ConfigScope = {
          kind: 'role',
          subjectId: parent.subjectId,
          roleId: value[1],
        };
        result.set(scopeKey(scope), scope);
      }
    }
    return [...result.values()];
  }
  const selected = (scope: ConfigScope) => ({
    scopeKind: scope.kind,
    scopeId: scopeId(scope),
  });
  async function revisionRecord(
    row: Selectable<SettingsDatabase['settingsRevisions']>,
    scope: ConfigScope,
    client: Kysely<SettingsDatabase>,
  ): Promise<SettingsRevision> {
    return {
      namespace: row.namespace,
      scope,
      revision: number(row.revision),
      definitionRevision: row.definitionRevision,
      sparse: row.sparse,
      patch: row.patch,
      actor: row.actor,
      recordedAt: row.recordedAt,
      ...(row.reason === null ? {} : { reason: row.reason }),
      ...(row.origin ? { origin: row.origin } : {}),
      definition: await options.definition(
        row.namespace,
        row.definitionRevision,
        client,
      ),
    };
  }
  async function archiveDefinition(
    value: ModuleDescription,
    client: Kysely<SettingsDatabase>,
  ) {
    assertJson(value.jsonSchema);
    assertJson(value.defaults);
    assertJson(value.fields);
    catalog.module(value.namespace);
    await client
      .insertInto('settingsDefinitions')
      .values({
        namespace: value.namespace,
        definitionRevision: value.definitionRevision,
        jsonSchema: json(value.jsonSchema),
        defaults: json(value.defaults),
        metadata: json({ owner: value.owner, fields: value.fields }),
      })
      .onConflict((oc) =>
        oc.columns(['namespace', 'definitionRevision']).doNothing(),
      )
      .execute();
    const stored = await options.definition(
      value.namespace,
      value.definitionRevision,
      client,
    );
    if (
      digest([
        stored.jsonSchema,
        stored.defaults,
        stored.owner,
        stored.fields,
      ]) !==
      digest([value.jsonSchema, value.defaults, value.owner, value.fields])
    )
      throw problem(
        'settings.definition_revision_conflict',
        'conflict',
        '同一归档定义版本包含不同内容',
      );
  }
  function mapScope(scope: ConfigScope, maps: SettingsScopeMap) {
    const exact = maps.find(
      (value) => scopeKey(value.from) === scopeKey(scope),
    );
    if (exact) return exact;
    if (scope.kind === 'role') {
      const parent = maps.find(
        (value) =>
          value.from.kind === 'subject' &&
          value.from.subjectId === scope.subjectId &&
          value.to.kind === 'subject',
      );
      if (parent && parent.to.kind === 'subject')
        return {
          ...parent,
          from: scope,
          to: {
            kind: 'role' as const,
            subjectId: parent.to.subjectId,
            roleId: scope.roleId,
          },
        };
    }
    throw problem(
      'settings.scope_map_missing',
      'validation',
      '配置导入缺少范围映射',
    );
  }
  return {
    async export(
      ctx: CallContext,
      input: { scopes: ConfigScope[]; includeHistory?: boolean },
      client?: Kysely<SettingsDatabase>,
    ): Promise<SettingsBundle> {
      return within(
        client,
        async (tx) => {
          const actual = await closure(input.scopes, tx),
            heads: SettingsBundle['heads'] = [],
            revisions: SettingsBundle['revisions'] = [],
            definitions = new Map<string, ModuleDescription>();
          for (const scope of actual) {
            for (const namespace of catalog.ids)
              await authorize(ctx, { action: 'export', namespace, scope }, tx);
            const headRows = await tx
              .selectFrom('settingsHeads')
              .selectAll()
              .where('scopeKind', '=', scope.kind)
              .where('scopeId', '=', scopeId(scope))
              .execute();
            for (const head of headRows) {
              const revision = number(head.revision);
              heads.push({ namespace: head.namespace, scope, revision });
              let query = tx
                .selectFrom('settingsRevisions')
                .selectAll()
                .where('namespace', '=', head.namespace)
                .where('scopeKind', '=', scope.kind)
                .where('scopeId', '=', scopeId(scope))
                .where('revision', '<=', head.revision);
              if (input.includeHistory === false)
                query = query.where('revision', '=', head.revision);
              const rows = await query.orderBy('revision', 'asc').execute();
              for (const row of rows) {
                const record = await revisionRecord(row, scope, tx);
                definitions.set(
                  digest([record.namespace, record.definitionRevision]),
                  record.definition,
                );
                let origin = record.origin;
                let depth = 0;
                while (origin) {
                  if (++depth > 32)
                    throw problem(
                      'settings.origin_depth',
                      'validation',
                      '配置来源链过深',
                    );
                  const definition = await options.definition(
                    record.namespace,
                    origin.definitionRevision,
                    tx,
                  );
                  definitions.set(
                    digest([
                      definition.namespace,
                      definition.definitionRevision,
                    ]),
                    definition,
                  );
                  origin = origin.previous;
                }
                const { definition, ...value } = record;
                revisions.push(value);
              }
            }
          }
          return immutable({
            formatVersion: 1,
            historyComplete: input.includeHistory !== false,
            definitions: [...definitions.values()],
            heads,
            revisions,
          });
        },
        true,
      );
    },
    async import(
      ctx: CallContext,
      input: { bundle: SettingsBundle; scopeMap: SettingsScopeMap },
      client?: Kysely<SettingsDatabase>,
    ) {
      const bundle = SettingsBundleSchema.parse(input.bundle),
        maps = SettingsScopeMapSchema.min(1).parse(input.scopeMap);
      if (Buffer.byteLength(JSON.stringify(bundle)) > 16 * 1024 * 1024)
        throw problem(
          'settings.bundle_too_large',
          'resource_pressure',
          '配置包超过16MiB',
        );
      const seen = new Set<string>();
      const plans = bundle.heads.map((head) => {
        const mapping = mapScope(head.scope, maps),
          scope = currentScope(mapping.to),
          id = key(head.namespace, scope);
        catalog.module(head.namespace);
        if (seen.has(id))
          throw problem(
            'settings.target_conflict',
            'conflict',
            '多个来源配置映射到同一目标',
          );
        seen.add(id);
        const expected = mapping.expectedRevisions[head.namespace];
        if (expected === undefined)
          throw problem(
            'settings.expected_revision_required',
            'validation',
            '导入需要每个目标命名空间的expectedRevision',
          );
        const records = bundle.revisions
          .filter(
            (row) =>
              row.namespace === head.namespace &&
              scopeKey(row.scope) === scopeKey(head.scope),
          )
          .sort((a, b) => a.revision - b.revision);
        if (
          !records.length ||
          records.at(-1)!.revision !== head.revision ||
          new Set(records.map((row) => row.revision)).size !== records.length ||
          records.some((row) => row.revision > head.revision) ||
          (bundle.historyComplete &&
            records.some((row, index) => row.revision !== index + 1))
        )
          throw problem(
            'settings.bundle_incomplete',
            'validation',
            '配置包历史或head闭包不完整',
          );
        return {
          head,
          scope,
          expected,
          records,
          id,
          operationId: `${ctx.trace.operationId}:settings-import:${id}`,
        };
      });
      return within(client, async (tx) => {
        for (const map of maps)
          for (const definition of bundle.definitions)
            await authorize(
              ctx,
              {
                action: 'import',
                namespace: definition.namespace,
                scope: currentScope(map.to),
              },
              tx,
            );
        for (const plan of plans)
          await authorize(
            ctx,
            {
              action: 'import',
              namespace: plan.head.namespace,
              scope: plan.scope,
            },
            tx,
          );
        const requestDigest = digest({
          action: 'import',
          input: { bundle, scopeMap: maps },
          actor: ctx.actor,
          consumer: ctx.consumer,
          purpose: ctx.purpose,
          audience: ctx.audience,
        });
        await sql`select pg_advisory_xact_lock(hashtextextended(${`settings:import:${ctx.trace.operationId}`},0))`.execute(
          tx,
        );
        const receipts = plans.length
          ? await tx
              .selectFrom('settingsChanges')
              .selectAll()
              .where(
                'operationId',
                'in',
                plans.map((plan) => plan.operationId),
              )
              .execute()
          : [];
        if (receipts.length) {
          if (
            receipts.length !== plans.length ||
            receipts.some((row) => row.requestDigest !== requestDigest)
          )
            throw problem(
              'settings.submission_conflict',
              'conflict',
              '同一提交身份包含不同配置导入',
            );
          return {
            heads: plans.map((plan) => {
              const row = receipts.find(
                (row) => row.operationId === plan.operationId,
              )!;
              return {
                namespace: row.result.namespace,
                scope: row.result.scope,
                revision: row.result.revision,
              };
            }),
            historyComplete: bundle.historyComplete,
            replayed: true,
          };
        }
        const locks = new Map<
          string,
          { namespace: string; scope: ConfigScope; exclusive: boolean }
        >();
        for (const plan of plans)
          for (const scope of scopes(installationId, plan.scope)) {
            const id = key(plan.head.namespace, scope),
              old = locks.get(id);
            locks.set(id, {
              namespace: plan.head.namespace,
              scope,
              exclusive: Boolean(
                old?.exclusive || scopeKey(scope) === scopeKey(plan.scope),
              ),
            });
          }
        const rank = (scope: ConfigScope) =>
          scope.kind === 'installation' ? 0 : scope.kind === 'subject' ? 1 : 2;
        for (const value of [...locks.values()].sort(
          (a, b) =>
            a.namespace.localeCompare(b.namespace) ||
            rank(a.scope) - rank(b.scope) ||
            scopeKey(a.scope).localeCompare(scopeKey(b.scope)),
        )) {
          const name = `settings:scope:${value.namespace}:${scopeKey(value.scope)}`;
          if (value.exclusive)
            await sql`select pg_advisory_xact_lock(hashtextextended(${name},0))`.execute(
              tx,
            );
          else
            await sql`select pg_advisory_xact_lock_shared(hashtextextended(${name},0))`.execute(
              tx,
            );
        }
        for (const definition of bundle.definitions)
          await archiveDefinition(definition, tx);
        const before = new Map<string, ReturnType<typeof evaluate>>();
        const imported = new Map<
          string,
          { revision: number; sparse: JsonObject }
        >();
        for (const plan of plans) {
          const namespace = plan.head.namespace,
            module = catalog.module(namespace),
            head = await tx
              .selectFrom('settingsHeads')
              .selectAll()
              .where('namespace', '=', namespace)
              .where('scopeKind', '=', plan.scope.kind)
              .where('scopeId', '=', scopeId(plan.scope))
              .forUpdate()
              .executeTakeFirst(),
            current = number(head?.revision ?? '0');
          if (current !== plan.expected)
            throw problem(
              'settings.revision_conflict',
              'conflict',
              '导入目标已有新修订',
              { currentRevision: current },
            );
          const layers = (await options.load([namespace], plan.scope, tx)).get(
            namespace,
          )!;
          try {
            before.set(plan.id, evaluate(module, layers));
          } catch (error) {
            if (
              !(error instanceof ProblemError) ||
              error.problem.category !== 'validation'
            )
              throw error;
          }
          const own = layers.at(-1)!;
          const final = plan.records.at(-1)!;
          patchSparse(
            module,
            plan.scope,
            own.sparse,
            jsonpatch.compare(own.sparse, final.sparse) as PatchOperation[],
          );
          let previous = own.sparse;
          for (let index = 0; index < plan.records.length; index++) {
            const source = plan.records[index];
            assertJson(source.sparse);
            const revision = current + index + 1;
            if (!Number.isSafeInteger(revision))
              throw problem(
                'settings.invalid_revision',
                'failed',
                '导入后的配置修订溢出',
              );
            const definitionRevision =
              index === plan.records.length - 1
                ? module.definitionRevision
                : source.definitionRevision;
            await options.definition(namespace, source.definitionRevision, tx);
            let previousOrigin = source.origin,
              depth = 0;
            while (previousOrigin) {
              if (++depth > 32)
                throw problem(
                  'settings.origin_depth',
                  'validation',
                  '配置来源链过深',
                );
              await options.definition(
                namespace,
                previousOrigin.definitionRevision,
                tx,
              );
              previousOrigin = previousOrigin.previous;
            }
            const origin: SettingsRevisionOrigin = {
              scope: source.scope,
              revision: source.revision,
              actor: source.actor,
              recordedAt: source.recordedAt,
              definitionRevision: source.definitionRevision,
              ...(source.origin ? { previous: source.origin } : {}),
              ...(bundle.sourceBundleRef
                ? { sourceBundleRef: bundle.sourceBundleRef }
                : {}),
            };
            await tx
              .insertInto('settingsRevisions')
              .values({
                namespace,
                scopeKind: plan.scope.kind,
                scopeId: scopeId(plan.scope),
                revision: String(revision),
                definitionRevision,
                sparse: json(source.sparse),
                patch: json(
                  jsonpatch.compare(
                    previous,
                    source.sparse,
                  ) as PatchOperation[],
                ),
                actor: json(ctx.actor),
                reason: '导入配置历史',
                origin: json(origin),
              })
              .execute();
            previous = source.sparse;
          }
          const revision = current + plan.records.length;
          await tx
            .insertInto('settingsHeads')
            .values({
              namespace,
              scopeKind: plan.scope.kind,
              scopeId: scopeId(plan.scope),
              revision: String(revision),
            })
            .onConflict((oc) =>
              oc
                .columns(['namespace', 'scopeKind', 'scopeId'])
                .doUpdateSet({ revision: String(revision) }),
            )
            .execute();
          imported.set(plan.id, { revision, sparse: final.sparse });
        }
        const heads: SettingsBundle['heads'] = [];
        for (const plan of plans) {
          const namespace = plan.head.namespace,
            module = catalog.module(namespace),
            layers = (await options.load([namespace], plan.scope, tx)).get(
              namespace,
            )!,
            snapshot = evaluate(module, layers);
          await module.validate?.(snapshot.values, {
            ctx,
            namespace,
            scope: plan.scope,
            db: tx,
          });
          const revision = imported.get(plan.id)!.revision;
          const result: SettingsChangeResult = {
            namespace,
            scope: plan.scope,
            operationId: plan.operationId,
            revision,
            changed: true,
            snapshot,
            effects: { hot: [], next_call: [], restart: [] },
          };
          for (const [path, metadata] of Object.entries(module.fields)) {
            const old = before.get(plan.id);
            if (
              !old ||
              digest(at(old.values, path)) !== digest(at(snapshot.values, path))
            )
              result.effects[metadata.effect].push(path);
          }
          await tx
            .insertInto('settingsChanges')
            .values({
              operationId: plan.operationId,
              namespace,
              scopeKind: plan.scope.kind,
              scopeId: scopeId(plan.scope),
              actorRef: ctx.actor.authorityRef,
              requestDigest,
              result: json(result),
            })
            .execute();
          await sql`select pg_notify('settings_changed',${JSON.stringify({ namespace, scope: plan.scope, revision })})`.execute(
            tx,
          );
          heads.push({ namespace, scope: plan.scope, revision });
        }
        return immutable({
          heads,
          historyComplete: bundle.historyComplete,
          replayed: false,
        });
      });
    },
    async deleteScopes(
      ctx: CallContext,
      input: { scopes: ConfigScope[] },
      client?: Kysely<SettingsDatabase>,
    ) {
      return within(client, async (tx) => {
        const actual = await closure(input.scopes, tx);
        for (const scope of actual)
          for (const namespace of catalog.ids)
            await authorize(
              ctx,
              { action: 'deleteScopes', namespace, scope },
              tx,
            );
        for (const scope of actual) {
          const condition = selected(scope);
          await tx
            .deleteFrom('settingsHeads')
            .where('scopeKind', '=', condition.scopeKind)
            .where('scopeId', '=', condition.scopeId)
            .execute();
          await tx
            .deleteFrom('settingsRevisions')
            .where('scopeKind', '=', condition.scopeKind)
            .where('scopeId', '=', condition.scopeId)
            .execute();
          await tx
            .deleteFrom('settingsChanges')
            .where('scopeKind', '=', condition.scopeKind)
            .where('scopeId', '=', condition.scopeId)
            .execute();
          for (const namespace of catalog.ids)
            await sql`select pg_notify('settings_changed',${JSON.stringify({ namespace, scope, revision: 0 })})`.execute(
              tx,
            );
        }
        return { scopes: actual };
      });
    },
  };
}
