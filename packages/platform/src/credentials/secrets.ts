// Secret generations and purpose/resource binding adapted from Heptalogos
// 21f0080460f3ee639deb9f5ea47282fb6c59df95 secret/service.ts.
// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import { createHash } from 'node:crypto';
import { DBOS } from '@dbos-inc/dbos-sdk';
import { v7 as uuid } from 'uuid';
import { canonicalize } from 'json-canonicalize';
import { z } from 'zod';
import {
  ConsumerSchema,
  RefSchema,
  TimePointSchema,
  ProblemSchema,
  SubjectIdSchema,
  type CallContext,
  type Consumer,
  type Ref,
} from '@nous-wave/contracts/common';
import { sql, type Kysely, type ColumnType } from '../storage/index.js';
import { readReal, parseInstant } from '../time/index.js';
import { problem, ProblemError } from '../errors.js';
import { createFileMaterials } from './files.js';
import { createOsCredentialStore } from './os-credential.js';

export {
  SecretRefSchema,
  SecretBackendSchema,
  SecretStatusSchema,
} from '@nous-wave/contracts/credentials';
export type {
  SecretBackendInput,
  SecretCreate,
  SecretRotate,
  SecretRevoke,
  SecretUseMetadata,
  SecretStatus,
} from '@nous-wave/contracts/credentials';
import {
  SecretRefSchema,
  SecretBackendSchema,
  SecretStatusSchema,
  SecretUseSchema,
  type SecretUse,
  type SecretBackendInput,
  type SecretCreate,
  type SecretRotate,
  type SecretRevoke,
  type SecretUseMetadata,
  type SecretStatus,
} from '@nous-wave/contracts/credentials';
type Locator =
  | { kind: 'environment'; variable: string }
  | { kind: 'os'; service: string; account: string }
  | { kind: 'protected_file'; path: string };
interface SecretRow {
  id: string;
  installationId: string;
  purpose: string;
  resourceRef: Ref;
  allowedConsumers: Consumer[];
  revision: number;
  generation: number;
  state: 'active' | 'revoked';
  createdAt: string;
  replacedAt: string | null;
  revokedAt: string | null;
}
interface GenerationRow {
  secretId: string;
  generation: number;
  backend: SecretStatus['backend'];
  locator: Locator;
  createdAt: string;
  removedAt: string | null;
}
interface ChangeRow {
  submissionKey: string;
  secretId: string;
  operation: string;
  requestHash: string;
  revision: number;
  recordedAt: string;
}
export interface SecretDatabase {
  secretRegistry: SecretRow;
  secretGenerations: Omit<GenerationRow, 'locator'> & {
    locator: ColumnType<Locator, Locator | string, Locator | string>;
  };
  secretChanges: ChangeRow;
}
export interface SecretRegistryOptions {
  db: Kysely<SecretDatabase>;
  installationId: string;
  product: 'nous' | 'hept';
  secretRoot: string;
  authorize(
    ctx: CallContext,
    operation: string,
    target?: Ref,
    client?: Kysely<any>,
  ): Promise<void>;
  purposes: Readonly<
    Record<
      string,
      {
        operation: string;
        resourceKinds: readonly string[];
        resolveUse?: (
          ctx: CallContext,
          input: SecretUse & { useRef: Ref },
        ) => Promise<{ operation: string; target: Ref }>;
      }
    >
  >;
}
export const secretMigration = {
  async up(db: Kysely<unknown>) {
    await sql`create table secret_registry(id uuid primary key,installation_id text not null,purpose text not null,resource_ref jsonb not null,allowed_consumers jsonb not null,revision integer not null check(revision>0),generation integer not null check(generation>0),state text not null check(state in ('active','revoked')),created_at timestamptz not null,replaced_at timestamptz,revoked_at timestamptz);
   create table secret_generations(secret_id uuid not null references secret_registry(id),generation integer not null check(generation>0),backend text not null check(backend in ('environment','os','protected_file')),locator jsonb not null,created_at timestamptz not null,removed_at timestamptz,primary key(secret_id,generation));
   create table secret_changes(submission_key text primary key,secret_id uuid not null references secret_registry(id),operation text not null,request_hash text not null,revision integer not null,recorded_at timestamptz not null);
   create index secret_generations_cleanup on secret_generations(secret_id,generation) where removed_at is null`.execute(
      db,
    );
  },
  async down(db: Kysely<unknown>) {
    await sql`drop table secret_changes;drop table secret_generations;drop table secret_registry`.execute(
      db,
    );
  },
};

const materialSchema = z
  .instanceof(Uint8Array)
  .refine((value) => value.byteLength > 0 && value.byteLength <= 65536);
const backendSchema = z.discriminatedUnion('kind', [
  z.strictObject({
    kind: z.literal('environment'),
    variable: z.string().regex(/^[A-Za-z_][A-Za-z0-9_]*$/),
  }),
  z.strictObject({
    kind: z.literal('protected_file'),
    material: materialSchema,
  }),
  z.strictObject({ kind: z.literal('os'), material: materialSchema }),
]);
const consumerList = z
  .array(ConsumerSchema)
  .min(1)
  .max(256)
  .refine((values) =>
    values.every(
      (value) =>
        !value.host.includes('*') &&
        (value.id === '*' || !value.id.includes('*')),
    ),
  );
const createSchema = z.strictObject({
  purpose: z.string().min(1).max(256),
  resourceRef: RefSchema.refine((ref) => !ref.selector),
  allowedConsumers: consumerList,
  backend: backendSchema,
});
const rotateSchema = z.strictObject({
  ref: SecretRefSchema,
  expectedRevision: z.number().int().positive().safe(),
  backend: backendSchema,
  allowedConsumers: consumerList.optional(),
});
const revokeSchema = z.strictObject({
  ref: SecretRefSchema,
  expectedRevision: z.number().int().positive().safe(),
});
const fingerprint = (value: unknown) =>
  createHash('sha256').update(canonicalize(value)).digest('hex');
const json = <T>(value: T) => sql<T>`${JSON.stringify(value)}::jsonb`;
function parse<T>(schema: z.ZodType<T>, value: unknown): T {
  const result = schema.safeParse(value);
  if (!result.success)
    throw problem('secrets.invalid_input', 'validation', '秘密登记输入无效', {
      details: {
        issues: result.error.issues.map((issue) => ({
          path: issue.path.map(String).join('.'),
          code: issue.code,
        })),
      },
    });
  return result.data;
}
function safe(error: unknown, code: string, message: string): ProblemError {
  const value =
    error && typeof error === 'object' && 'problem' in error
      ? ProblemSchema.safeParse(error.problem)
      : undefined;
  return value?.success
    ? new ProblemError(value.data)
    : problem(code, 'unavailable', message);
}
function ordinaryMaterial() {
  if (DBOS.isWithinWorkflow())
    throw problem(
      'secrets.material_workflow_denied',
      'validation',
      '秘密写入须使用普通受信操作，内容不得进入工作流输入',
    );
}
function sameResource(allowed: Ref, requested: Ref) {
  return (
    allowed.product === requested.product &&
    allowed.installationId === requested.installationId &&
    allowed.subjectId === requested.subjectId &&
    allowed.kind === requested.kind &&
    allowed.id === requested.id &&
    (allowed.revision === undefined ||
      allowed.revision === requested.revision) &&
    !requested.selector
  );
}

export function createSecretRegistry(options: SecretRegistryOptions) {
  const { db } = options,
    files = createFileMaterials(options.secretRoot),
    os = createOsCredentialStore();
  const purposes = Object.fromEntries(
    Object.entries(options.purposes).map(([id, value]) => [
      id,
      { ...value, resourceKinds: [...value.resourceKinds] },
    ]),
  );
  function purpose(id: string, ref: Ref) {
    const value = purposes[id];
    if (
      !value ||
      !value.operation ||
      !value.resourceKinds.includes(ref.kind) ||
      ref.product !== options.product ||
      ref.installationId !== options.installationId ||
      ref.selector
    )
      throw problem(
        'secrets.purpose_denied',
        'forbidden',
        '秘密用途或资源类型未登记',
      );
    return value;
  }
  const key = (ctx: CallContext, operation: string) =>
    fingerprint([
      options.installationId,
      ctx.consumer,
      ctx.subjectId ?? null,
      ctx.actor.kind === 'human' ? ctx.actor.id : null,
      ctx.trace.operationId,
      operation,
    ]);
  const idOf = (ref: string) => parse(SecretRefSchema, ref).slice(7);
  const refOf = (id: string) => `secret:${id}`;
  async function row(ref: string, client = db) {
    const value = await client
      .selectFrom('secretRegistry')
      .selectAll()
      .where('id', '=', idOf(ref))
      .where('installationId', '=', options.installationId)
      .executeTakeFirst();
    if (!value)
      throw problem('secrets.not_found', 'not_found', '秘密登记不存在');
    return value;
  }
  async function receipt(
    submissionKey: string,
    requestHash: string,
    client = db,
  ) {
    const value = await client
      .selectFrom('secretChanges')
      .selectAll()
      .where('submissionKey', '=', submissionKey)
      .executeTakeFirst();
    if (value && value.requestHash !== requestHash)
      throw problem(
        'secrets.submission_conflict',
        'conflict',
        '同一提交身份已有不同的秘密操作',
      );
    return value;
  }
  const payload = (backend: SecretBackendInput) =>
    backend.kind === 'environment'
      ? backend
      : {
          kind: backend.kind,
          materialDigest: createHash('sha256')
            .update(backend.material)
            .digest('hex'),
        };
  async function prepare(
    backend: SecretBackendInput,
    submissionKey: string,
    requestHash: string,
  ): Promise<Locator> {
    const slot = fingerprint([submissionKey, requestHash]);
    if (backend.kind === 'environment')
      return { kind: 'environment', variable: backend.variable };
    const copy = Uint8Array.from(backend.material);
    try {
      if (backend.kind === 'os') {
        const locator = {
          kind: 'os' as const,
          service: `${options.product}/secret/${options.installationId}`,
          account: `generation/${slot}`,
        };
        await os.set(locator, copy);
        return locator;
      }
      const locator = {
        kind: 'protected_file' as const,
        path: `registry/${slot}.secret`,
      };
      await files.put(locator.path, copy);
      return locator;
    } catch (error) {
      throw safe(
        error,
        'secrets.backend_unavailable',
        '秘密后端无法保存新内容',
      );
    } finally {
      copy.fill(0);
    }
  }
  async function remove(locator: Locator) {
    if (locator.kind === 'os') await os.delete(locator);
    else if (locator.kind === 'protected_file')
      await files.delete(locator.path);
  }
  async function cleanup(id: string) {
    const current = await row(refOf(id)),
      generations = await db
        .selectFrom('secretGenerations')
        .selectAll()
        .where('secretId', '=', id)
        .where('removedAt', 'is', null)
        .execute();
    for (const generation of generations) {
      if (
        current.state === 'active' &&
        generation.generation === current.generation
      )
        continue;
      try {
        await remove(generation.locator);
        await db
          .updateTable('secretGenerations')
          .set({ removedAt: readReal().at })
          .where('secretId', '=', id)
          .where('generation', '=', generation.generation)
          .execute();
      } catch {
        /* Status retains the undeleted generation for the same operation's retry. */
      }
    }
  }
  async function available(locator: Locator) {
    try {
      return locator.kind === 'environment'
        ? Boolean(process.env[locator.variable])
        : locator.kind === 'os'
          ? await os.exists(locator)
          : await files.exists(locator.path);
    } catch {
      return false;
    }
  }
  async function metadata(current: SecretRow): Promise<SecretStatus> {
    const generations = await db
      .selectFrom('secretGenerations')
      .selectAll()
      .where('secretId', '=', current.id)
      .execute();
    const generation = generations.find(
      (value) => value.generation === current.generation,
    );
    if (!generation)
      throw problem(
        'secrets.repository_invalid',
        'unavailable',
        '秘密代际登记不完整',
      );
    const retired = generations.filter(
        (value) =>
          current.state === 'revoked' ||
          value.generation !== current.generation,
      ),
      present =
        current.state === 'active' && (await available(generation.locator));
    return {
      ref: refOf(current.id),
      revision: current.revision,
      generation: current.generation,
      state:
        current.state === 'revoked'
          ? 'revoked'
          : present
            ? 'active'
            : 'unavailable',
      purpose: current.purpose,
      resourceRef: current.resourceRef,
      allowedConsumers: current.allowedConsumers,
      backend: generation.backend,
      configured: current.state === 'active',
      available: present,
      createdAt: { domainId: 'real', at: parseInstant(current.createdAt) },
      ...(current.replacedAt
        ? {
            replacedAt: {
              domainId: 'real',
              at: parseInstant(current.replacedAt),
            },
          }
        : {}),
      ...(current.revokedAt
        ? {
            revokedAt: {
              domainId: 'real',
              at: parseInstant(current.revokedAt),
            },
          }
        : {}),
      cleanupPending: retired.some(
        (value) => value.backend !== 'environment' && !value.removedAt,
      ),
      externalRotationRequired: retired.some(
        (value) => value.backend === 'environment',
      ),
    };
  }
  async function changed(
    client: Kysely<SecretDatabase>,
    submissionKey: string,
    requestHash: string,
    operation: string,
    current: SecretRow,
  ) {
    await client
      .insertInto('secretChanges')
      .values({
        submissionKey,
        requestHash,
        operation,
        secretId: current.id,
        revision: current.revision,
        recordedAt: readReal().at,
      })
      .execute();
  }
  async function adoptStaged(
    ctx: CallContext,
    submissionKey: string,
    requestHash: string,
    locator: Locator,
    write: (client: Kysely<SecretDatabase>) => Promise<string>,
  ): Promise<string> {
    try {
      return await db.transaction().execute(async (client) => {
        await sql`select pg_advisory_xact_lock(hashtextextended(${submissionKey},0))`.execute(
          client,
        );
        const old = await receipt(submissionKey, requestHash, client);
        return old ? old.secretId : write(client);
      });
    } catch (error) {
      // An acknowledgement can fail after commit. Confirm the receipt before cleanup.
      let committed: ChangeRow | undefined;
      try {
        committed = await db
          .selectFrom('secretChanges')
          .selectAll()
          .where('submissionKey', '=', submissionKey)
          .executeTakeFirst();
      } catch {
        throw problem(
          'secrets.commit_unknown',
          'effect_unknown',
          '秘密登记提交结果待确认；使用原提交身份继续',
          { operationId: ctx.trace.operationId },
        );
      }
      if (committed?.requestHash === requestHash) return committed.secretId;
      try {
        await remove(locator);
      } catch {
        throw problem(
          'secrets.cleanup_pending',
          'unavailable',
          '秘密操作未采用，新后端内容待清理',
          { operationId: ctx.trace.operationId },
        );
      }
      throw safe(error, 'secrets.write_failed', '秘密登记无法写入');
    }
  }
  async function manage(
    ctx: CallContext,
    current: SecretRow,
    client?: Kysely<SecretDatabase>,
  ) {
    await options.authorize(ctx, 'secrets.manage', current.resourceRef, client);
  }
  const registry = {
    /** Called by the Data owner in its Subject transaction; backend I/O follows commit. */
    async deleteSubject(
      ctx: CallContext,
      input: { subjectId: string },
      client?: Kysely<any>,
    ) {
      const subjectId = SubjectIdSchema.parse(input.subjectId);
      if (ctx.subjectId && ctx.subjectId !== subjectId)
        throw problem(
          'secrets.subject_mismatch',
          'forbidden',
          '秘密清理主体与当前操作不符',
        );
      if (client && !client.isTransaction)
        throw problem(
          'secrets.transaction_required',
          'validation',
          '秘密撤销需要当前业务事务',
        );
      const write = async (tx: Kysely<any>) => {
        await options.authorize(
          { ...ctx, subjectId },
          'data.purge',
          {
            product: options.product,
            installationId: options.installationId,
            subjectId,
            kind: 'subject',
            id: subjectId,
          },
          tx,
        );
        const result =
          await sql`update secret_registry set state='revoked',revision=revision+1,revoked_at=clock_timestamp() where installation_id=${options.installationId} and resource_ref->>'subjectId'=${subjectId} and state='active'`.execute(
            tx,
          );
        return { revoked: Number(result.numAffectedRows ?? 0n) };
      };
      return client ? write(client) : db.transaction().execute(write);
    },
    /** Internal post-commit cleanup capability. Only persisted revoked Subject secrets qualify. */
    async cleanupSubject(input: { subjectId: string }) {
      const subjectId = SubjectIdSchema.parse(input.subjectId);
      const rows = await sql<{
        id: string;
      }>`select id from secret_registry where installation_id=${options.installationId} and resource_ref->>'subjectId'=${subjectId} and state='revoked' order by id`.execute(
        db,
      );
      for (const current of rows.rows) await cleanup(current.id);
      const result = await sql<{
        cleanupPending: string;
        externalRotationRequired: string;
      }>`select count(*) filter(where g.backend<>'environment' and g.removed_at is null)::text as cleanup_pending,count(distinct r.id) filter(where g.backend='environment')::text as external_rotation_required from secret_registry r join secret_generations g on g.secret_id=r.id where r.installation_id=${options.installationId} and r.resource_ref->>'subjectId'=${subjectId} and r.state='revoked'`.execute(
        db,
      );
      return {
        cleanupPending: Number(result.rows[0]?.cleanupPending ?? 0),
        externalRotationRequired: Number(
          result.rows[0]?.externalRotationRequired ?? 0,
        ),
      };
    },
    async create(ctx: CallContext, input: SecretCreate): Promise<SecretStatus> {
      ordinaryMaterial();
      const request = parse(createSchema, input);
      purpose(request.purpose, request.resourceRef);
      await options.authorize(ctx, 'secrets.manage', request.resourceRef);
      const submissionKey = key(ctx, 'create'),
        requestHash = fingerprint({
          ...request,
          backend: payload(request.backend),
        }),
        prior = await receipt(submissionKey, requestHash);
      if (prior) return registry.status(ctx, { ref: refOf(prior.secretId) });
      const locator = await prepare(
        request.backend,
        submissionKey,
        requestHash,
      );
      const id = await adoptStaged(
        ctx,
        submissionKey,
        requestHash,
        locator,
        async (client) => {
          await options.authorize(
            ctx,
            'secrets.manage',
            request.resourceRef,
            client,
          );
          const id = uuid(),
            at = readReal().at,
            current: SecretRow = {
              id,
              installationId: options.installationId,
              purpose: request.purpose,
              resourceRef: request.resourceRef,
              allowedConsumers: request.allowedConsumers,
              revision: 1,
              generation: 1,
              state: 'active',
              createdAt: at,
              replacedAt: null,
              revokedAt: null,
            };
          await client
            .insertInto('secretRegistry')
            .values({
              ...current,
              resourceRef: json(current.resourceRef),
              allowedConsumers: json(current.allowedConsumers),
            })
            .execute();
          await client
            .insertInto('secretGenerations')
            .values({
              secretId: id,
              generation: 1,
              backend: locator.kind,
              locator,
              createdAt: at,
              removedAt: null,
            })
            .execute();
          await changed(client, submissionKey, requestHash, 'create', current);
          return id;
        },
      );
      return registry.status(ctx, { ref: refOf(id) });
    },
    async status(
      ctx: CallContext,
      input: { ref: string },
    ): Promise<SecretStatus> {
      try {
        const current = await row(input.ref);
        await manage(ctx, current);
        return await metadata(current);
      } catch (error) {
        throw safe(error, 'secrets.read_failed', '秘密状态不可用');
      }
    },
    async rotate(ctx: CallContext, input: SecretRotate): Promise<SecretStatus> {
      ordinaryMaterial();
      const request = parse(rotateSchema, input),
        current = await row(request.ref);
      await manage(ctx, current);
      const submissionKey = key(ctx, 'rotate'),
        requestHash = fingerprint({
          ...request,
          backend: payload(request.backend),
        }),
        prior = await receipt(submissionKey, requestHash);
      if (prior) {
        await cleanup(prior.secretId);
        return registry.status(ctx, { ref: refOf(prior.secretId) });
      }
      if (
        current.revision !== request.expectedRevision ||
        current.state !== 'active'
      )
        throw problem(
          'secrets.revision_conflict',
          'conflict',
          '秘密已修订或撤销',
          { currentRevision: current.revision },
        );
      const locator = await prepare(
        request.backend,
        submissionKey,
        requestHash,
      );
      const id = await adoptStaged(
        ctx,
        submissionKey,
        requestHash,
        locator,
        async (client) => {
          await manage(ctx, current, client);
          const locked = await client
            .selectFrom('secretRegistry')
            .selectAll()
            .where('id', '=', current.id)
            .forUpdate()
            .executeTakeFirstOrThrow();
          if (
            locked.revision !== request.expectedRevision ||
            locked.state !== 'active'
          )
            throw problem(
              'secrets.revision_conflict',
              'conflict',
              '秘密已修订或撤销',
              { currentRevision: locked.revision },
            );
          const generation = locked.generation + 1,
            at = readReal().at,
            next = {
              ...locked,
              generation,
              revision: locked.revision + 1,
              allowedConsumers:
                request.allowedConsumers ?? locked.allowedConsumers,
              replacedAt: at,
            };
          await client
            .insertInto('secretGenerations')
            .values({
              secretId: locked.id,
              generation,
              backend: locator.kind,
              locator,
              createdAt: at,
              removedAt: null,
            })
            .execute();
          await client
            .updateTable('secretRegistry')
            .set({
              generation,
              revision: next.revision,
              allowedConsumers: json(next.allowedConsumers),
              replacedAt: at,
            })
            .where('id', '=', locked.id)
            .execute();
          await changed(client, submissionKey, requestHash, 'rotate', next);
          return locked.id;
        },
      );
      await cleanup(id);
      return registry.status(ctx, { ref: refOf(id) });
    },
    async revoke(ctx: CallContext, input: SecretRevoke): Promise<SecretStatus> {
      const request = parse(revokeSchema, input),
        current = await row(request.ref);
      await manage(ctx, current);
      const submissionKey = key(ctx, 'revoke'),
        requestHash = fingerprint(request);
      const id = await db.transaction().execute(async (client) => {
        await sql`select pg_advisory_xact_lock(hashtextextended(${submissionKey},0))`.execute(
          client,
        );
        const prior = await receipt(submissionKey, requestHash, client);
        if (prior) return prior.secretId;
        const locked = await client
          .selectFrom('secretRegistry')
          .selectAll()
          .where('id', '=', current.id)
          .forUpdate()
          .executeTakeFirstOrThrow();
        await manage(ctx, locked, client);
        if (locked.revision !== request.expectedRevision)
          throw problem(
            'secrets.revision_conflict',
            'conflict',
            '秘密已有新修订',
            { currentRevision: locked.revision },
          );
        const next =
          locked.state === 'revoked'
            ? locked
            : {
                ...locked,
                state: 'revoked' as const,
                revision: locked.revision + 1,
                revokedAt: readReal().at,
              };
        if (locked.state !== 'revoked')
          await client
            .updateTable('secretRegistry')
            .set({
              state: 'revoked',
              revision: next.revision,
              revokedAt: next.revokedAt,
            })
            .where('id', '=', locked.id)
            .execute();
        await changed(client, submissionKey, requestHash, 'revoke', next);
        return locked.id;
      });
      await cleanup(id);
      return registry.status(ctx, { ref: refOf(id) });
    },
    async withSecret<T>(
      ctx: CallContext,
      input: SecretUse,
      use: (bytes: Uint8Array, metadata: SecretUseMetadata) => Promise<T>,
    ): Promise<T> {
      input = parse(SecretUseSchema, input);
      const definition = purpose(input.purpose, input.resourceRef);
      const authorizeUse = async () => {
        if (input.useRef && !definition.resolveUse)
          throw problem(
            'secrets.use_target_denied',
            'forbidden',
            '此秘密用途没有登记资源关系解析器',
          );
        const use = input.useRef
          ? await definition.resolveUse!(ctx, {
              ...input,
              useRef: input.useRef,
            })
          : { operation: definition.operation, target: input.resourceRef };
        if (!use.operation || use.operation.includes('*'))
          throw problem(
            'secrets.use_target_denied',
            'forbidden',
            '秘密用途解析必须返回正式操作',
          );
        await options.authorize(ctx, use.operation, use.target);
      };
      await authorizeUse();
      const current = await row(input.ref);
      if (current.state !== 'active')
        throw problem('secrets.revoked', 'forbidden', '秘密已撤销');
      if (
        current.purpose !== input.purpose ||
        !sameResource(current.resourceRef, input.resourceRef) ||
        !current.allowedConsumers.some(
          (consumer) =>
            consumer.kind === ctx.consumer.kind &&
            consumer.host === ctx.consumer.host &&
            (consumer.id === ctx.consumer.id || consumer.id === '*'),
        )
      )
        throw problem(
          'secrets.use_denied',
          'forbidden',
          '秘密的消费者、用途或资源不匹配',
        );
      const generation = await db
        .selectFrom('secretGenerations')
        .selectAll()
        .where('secretId', '=', current.id)
        .where('generation', '=', current.generation)
        .executeTakeFirst();
      if (!generation)
        throw problem(
          'secrets.repository_invalid',
          'unavailable',
          '秘密代际登记不完整',
        );
      let entered = false;
      const invoke = async (bytes: Uint8Array) => {
        await authorizeUse();
        const latest = await row(input.ref);
        if (
          latest.state !== 'active' ||
          latest.revision !== current.revision ||
          latest.generation !== current.generation
        )
          throw problem(
            'secrets.changed',
            'conflict',
            '秘密在使用前已修订或撤销',
          );
        entered = true;
        return use(bytes, {
          ref: input.ref,
          revision: latest.revision,
          generation: latest.generation,
        });
      };
      try {
        const locator = generation.locator;
        if (locator.kind === 'os')
          return await os.withCredential(locator, invoke);
        if (locator.kind === 'protected_file')
          return await files.withMaterial(locator.path, invoke);
        const text = process.env[locator.variable];
        if (!text)
          throw problem(
            'secrets.material_unavailable',
            'unavailable',
            '秘密环境绑定不可用',
          );
        const bytes = Buffer.from(text);
        try {
          if (bytes.byteLength > 65536)
            throw problem(
              'secrets.invalid_material',
              'validation',
              '秘密内容大小无效',
            );
          return await invoke(bytes);
        } finally {
          bytes.fill(0);
        }
      } catch (error) {
        if (entered) throw error;
        throw safe(error, 'secrets.material_unavailable', '秘密内容不可用');
      }
    },
  };
  return registry;
}
export type SecretRegistry = ReturnType<typeof createSecretRegistry>;
