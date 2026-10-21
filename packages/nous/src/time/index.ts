import { createHash } from 'node:crypto';
import {
  type CallContext,
  type Consumer,
  canonicalRefKey,
  type Ref,
  RefSchema,
  TimePointInputSchema,
} from '@nous-wave/contracts/common';
import {
  type CapturedTime,
  CapturedTimeSchema,
  ClockRefSchema,
  type TimeCaptureReceipt,
  TimeCaptureReceiptSchema,
  type TimeRecordings,
  type TimeSelection,
  TimeSelectionSchema,
} from '@nous-wave/contracts/time';
import { problem } from '@nous-wave/platform/errors';
import type {
  ConfigScope,
  Settings,
  SettingsModule,
} from '@nous-wave/platform/settings';
import { sql } from '@nous-wave/platform/storage';
import { readReal } from '@nous-wave/platform/time';
import { canonicalize } from 'json-canonicalize';
import type { Kysely } from 'kysely';
import { v7 as uuid } from 'uuid';
import { z } from 'zod';
import type { TimeDatabase } from './migration.js';
import { createTimeData } from './data.js';

const json = <T>(value: T) => sql<T>`${JSON.stringify(value)}::jsonb`;
export interface TimeStore {
  installationId: string;
  db: Kysely<TimeDatabase>;
  transaction<T>(use: (db: Kysely<TimeDatabase>) => Promise<T>): Promise<T>;
  authorize(
    ctx: CallContext,
    operation: string,
    target?: Ref,
    db?: Kysely<any>,
  ): Promise<void>;
}

const ReferenceSchema = z
  .strictObject({
    domainId: z.string().min(1),
    connectionRef: RefSchema.omit({
      revision: true,
      selector: true,
      subjectId: true,
    })
      .extend({ kind: z.literal('ClockSource') })
      .strict()
      .nullable(),
    clockRef: ClockRefSchema.omit({ revision: true }).nullable(),
  })
  .superRefine((value, ctx) => {
    if (
      value.domainId === 'real'
        ? value.connectionRef !== null || value.clockRef !== null
        : !value.connectionRef || !value.clockRef
    )
      ctx.addIssue({
        code: 'custom',
        message: '现实域无虚拟时钟；其他域必须绑定正式ClockSource与Clock身份',
      });
  });
export const timeSettingsModule: SettingsModule<
  z.infer<typeof ReferenceSchema>
> = {
  id: 'time.reference',
  definitionRevision: '1',
  owner: 'time',
  schema: ReferenceSchema,
  defaults: { domainId: 'real', connectionRef: null, clockRef: null },
  fields: Object.fromEntries(
    ['domainId', 'connectionRef', 'clockRef'].map((field) => [
      `/${field}`,
      {
        labelKey: `time.${field}`,
        descriptionKey: `time.${field}.description`,
        scopes: ['installation', 'subject'],
        visibility: 'advanced',
        effect: 'next_call',
        consumerIds: ['time.clock'],
      },
    ]),
  ) as SettingsModule['fields'],
};
/** Supplied by a trusted installed connector; an external clock remains its source product's state. */
export interface ClockSource {
  product: Ref['product'];
  installationId: string;
  read(ctx: CallContext, selection: TimeSelection): Promise<CapturedTime>;
  capture(
    ctx: CallContext,
    input: { operationId: string; selection: TimeSelection },
  ): Promise<TimeCaptureReceipt>;
  /** Read the original stable capture submission; recovery never creates a new time point. */
  recover(
    ctx: CallContext,
    input: { operationId: string; selection: TimeSelection },
  ): Promise<TimeCaptureReceipt | null>;
  record?: TimeRecordings['captureTerminal'];
  readCapture(ctx: CallContext, ref: Ref): Promise<TimeCaptureReceipt>;
}
export interface TimeOptions {
  settings: Settings;
  processBootId: string;
  sources?: Readonly<Record<string, ClockSource>>;
  recordings?: (capability: TimeRecordings) => void;
}
export function createTime(store: TimeStore, options: TimeOptions) {
  const registrations = new Map<
    string,
    ReturnType<Settings['registerConsumer']>
  >();
  const sources = Object.freeze({ ...options.sources });
  const scope = (ctx: CallContext): ConfigScope =>
    ctx.subjectId
      ? { kind: 'subject', subjectId: ctx.subjectId }
      : { kind: 'installation', installationId: store.installationId };
  async function reference(ctx: CallContext, db?: Kysely<TimeDatabase>) {
    const key = canonicalize(scope(ctx));
    let registration = registrations.get(key);
    if (!registration) {
      registration = options.settings.registerConsumer({
        namespace: 'time.reference',
        scope: scope(ctx),
        processBootId: options.processBootId,
        consumerId: 'time.clock',
      });
      registrations.set(key, registration);
    }
    const snapshot = await registration.snapshot(db);
    if (!db?.isTransaction) registration.reportApplied(snapshot);
    return ReferenceSchema.parse(snapshot.values);
  }
  async function selected(
    ctx: CallContext,
    input: TimeSelection | undefined,
    db?: Kysely<TimeDatabase>,
  ) {
    const selection = TimeSelectionSchema.parse(input ?? {}),
      binding = await reference(ctx, db);
    if (
      selection.subjectId !== undefined &&
      selection.subjectId !== ctx.subjectId
    )
      throw problem(
        'time.subject_mismatch',
        'forbidden',
        '时间选择不属于当前主体',
      );
    const domainId = selection.domainId ?? binding.domainId;
    if (domainId !== binding.domainId)
      throw problem(
        'time.unbound_domain',
        'forbidden',
        '时间域未绑定到当前消费范围',
      );
    if (domainId === 'real') {
      if (selection.preferClock)
        throw problem(
          'time.clock_mismatch',
          'validation',
          '现实域没有虚拟Clock',
        );
      return { kind: 'real' as const };
    }
    const connection = binding.connectionRef!;
    if (
      connection.product !== 'nous' ||
      connection.installationId !== store.installationId
    )
      throw problem(
        'time.connection_scope',
        'forbidden',
        'ClockSource连接不属于当前安装',
      );
    const source = sources[connection.id];
    if (!source)
      throw problem(
        'time.source_unavailable',
        'unavailable',
        '已绑定的ClockSource不可用',
      );
    const clockRef: z.infer<typeof ClockRefSchema> =
      selection.preferClock ?? binding.clockRef!;
    if (
      clockRef.product !== source.product ||
      clockRef.installationId !== source.installationId ||
      canonicalRefKey({ ...clockRef, revision: undefined }) !==
        canonicalRefKey(binding.clockRef!)
    )
      throw problem(
        'time.clock_mismatch',
        'forbidden',
        '请求Clock不属于已绑定来源',
      );
    return {
      kind: 'external' as const,
      connectionRef: binding.connectionRef!,
      source,
      selection: { domainId, preferClock: clockRef },
    };
  }
  function checked(
    captured: CapturedTime,
    selection: Awaited<ReturnType<typeof selected>>,
  ): CapturedTime {
    if (selection.kind === 'real') {
      if (captured.point.domainId !== 'real')
        throw problem('time.invalid_capture', 'conflict', '现实捕获域不一致');
      return captured;
    }
    const expected = selection.selection.preferClock;
    if (
      captured.point.domainId !== selection.selection.domainId ||
      !('clockRef' in captured) ||
      !captured.clockRef ||
      canonicalRefKey({ ...captured.clockRef, revision: undefined }) !==
        canonicalRefKey({ ...expected, revision: undefined }) ||
      (expected.revision !== undefined &&
        captured.clockRef.revision !== expected.revision)
    )
      throw problem(
        'time.source_revision_conflict',
        'conflict',
        'ClockSource返回了不同的映射或修订',
      );
    return captured;
  }
  const requestDigest = (
    ctx: CallContext,
    input: {
      operationId: string;
      selection?: TimeSelection;
      realAt?: string;
      originRef?: Ref;
      recordedConsumer?: Consumer;
    },
  ) =>
    createHash('sha256')
      .update(
        canonicalize({
          consumer: input.recordedConsumer ?? ctx.consumer,
          subjectId: input.originRef
            ? (input.originRef.subjectId ?? null)
            : (ctx.subjectId ?? null),
          selection: input.selection ?? null,
          originRef: input.originRef ?? null,
        }),
      )
      .digest('hex');
  async function previous(
    ctx: CallContext,
    input: {
      operationId: string;
      selection?: TimeSelection;
      realAt?: string;
      originRef?: Ref;
      recordedConsumer?: Consumer;
    },
    db = store.db,
  ) {
    const value = await db
      .selectFrom('timeCaptures')
      .selectAll()
      .where('operationId', '=', input.operationId)
      .executeTakeFirst();
    if (!value) return;
    if (value.requestDigest !== requestDigest(ctx, input))
      throw problem(
        'time.capture_conflict',
        'conflict',
        '该捕获身份已对应另一消费或时间请求',
      );
    return TimeCaptureReceiptSchema.parse(value.receipt);
  }
  async function persist(
    ctx: CallContext,
    input: {
      operationId: string;
      selection?: TimeSelection;
      realAt?: string;
      originRef?: Ref;
      recordedConsumer?: Consumer;
    },
    receipt: TimeCaptureReceipt,
    db: Kysely<TimeDatabase>,
    connectionRef: Ref | null = null,
  ) {
    const id =
      receipt.captureRef.product === 'nous' &&
      receipt.captureRef.installationId === store.installationId
        ? receipt.captureRef.id
        : uuid();
    await db
      .insertInto('timeCaptures')
      .values({
        id,
        operationId: input.operationId,
        requestDigest: requestDigest(ctx, input),
        captureKey: canonicalRefKey(receipt.captureRef),
        connectionRef: connectionRef ? json(connectionRef) : null,
        subjectId: input.originRef
          ? (input.originRef.subjectId ?? null)
          : (ctx.subjectId ?? null),
        consumer: json(input.recordedConsumer ?? ctx.consumer),
        actor: json(ctx.actor),
        originRef: input.originRef ? json(input.originRef) : null,
        receipt: json(receipt),
        recordedAt: readReal().at,
      })
      .onConflict((oc) => oc.column('operationId').doNothing())
      .execute();
    return (await previous(ctx, input, db))!;
  }
  function real(
    subjectId: string | undefined,
    realAt?: string,
  ): TimeCaptureReceipt {
    const at = realAt
      ? TimePointInputSchema.parse({ domainId: 'real', at: realAt }).at
      : readReal().at;
    const point = { domainId: 'real' as const, at };
    return {
      point,
      realAt: point.at,
      captureRef: {
        product: 'nous',
        installationId: store.installationId,
        subjectId,
        kind: 'TimeCapture',
        id: uuid(),
      },
    };
  }
  const time = {
    data: createTimeData(store),
    sources: {
      async list(ctx: CallContext) {
        await store.authorize(ctx, 'clock.references.read');
        return Object.entries(sources).map(([id, source]) => ({
          ref: {
            product: 'nous' as const,
            installationId: store.installationId,
            kind: 'ClockSource',
            id,
          },
          authority: {
            product: source.product,
            installationId: source.installationId,
          },
        }));
      },
      async read(ctx: CallContext, ref: Ref) {
        await store.authorize(ctx, 'clock.references.read', ref);
        if (
          ref.product !== 'nous' ||
          ref.installationId !== store.installationId ||
          ref.kind !== 'ClockSource' ||
          ref.subjectId ||
          ref.revision ||
          ref.selector ||
          !sources[ref.id]
        )
          throw problem(
            'time.source_not_found',
            'not_found',
            '正式ClockSource连接不存在',
          );
        const source = sources[ref.id];
        return {
          ref,
          authority: {
            product: source.product,
            installationId: source.installationId,
          },
        };
      },
    },
    async read(
      ctx: CallContext,
      selection?: TimeSelection,
    ): Promise<CapturedTime> {
      await store.authorize(ctx, 'clock.read');
      const target = await selected(ctx, selection);
      if (target.kind === 'real') {
        const point = { domainId: 'real' as const, at: readReal().at };
        return { point, realAt: point.at };
      }
      return checked(
        CapturedTimeSchema.parse(
          await target.source.read(ctx, target.selection),
        ),
        target,
      );
    },
    async capture(
      ctx: CallContext,
      input: {
        operationId: string;
        selection?: TimeSelection;
        realAt?: string;
        existingOnly?: boolean;
      },
    ): Promise<TimeCaptureReceipt> {
      z.string().min(1).max(2048).parse(input.operationId);
      await store.authorize(ctx, 'clock.capture');
      const prior = await previous(ctx, input);
      if (prior) return prior;
      if (input.existingOnly)
        throw problem(
          'time.capture_not_found',
          'not_found',
          '原时间捕获不存在',
        );
      const target = await selected(ctx, input.selection);
      let receipt: TimeCaptureReceipt;
      if (target.kind === 'real') receipt = real(ctx.subjectId, input.realAt);
      else {
        const request = {
          operationId: `nous:${store.installationId}:${input.operationId}`,
          selection: target.selection,
        };
        const value = input.realAt
          ? await target.source.recover(ctx, request)
          : await target.source.capture(ctx, request);
        if (!value)
          throw problem(
            'time.capture_recovery_unavailable',
            'unavailable',
            '虚拟域的原时间捕获未持久保存，不能用恢复时刻补写',
          );
        receipt = TimeCaptureReceiptSchema.parse(value);
      }
      checked(receipt, target);
      return store.transaction((db) =>
        persist(
          ctx,
          input,
          receipt,
          db,
          target.kind === 'external' ? target.connectionRef : null,
        ),
      );
    },
    async captureIn(
      db: Kysely<TimeDatabase>,
      ctx: CallContext,
      input: {
        operationId: string;
        selection?: TimeSelection;
        realAt?: string;
        existingOnly?: boolean;
      },
    ): Promise<TimeCaptureReceipt> {
      z.string().min(1).max(2048).parse(input.operationId);
      await store.authorize(ctx, 'clock.capture', undefined, db);
      const prior = await previous(ctx, input, db);
      if (prior) return prior;
      if (input.existingOnly)
        throw problem(
          'time.capture_not_found',
          'not_found',
          '原时间捕获不存在',
        );
      const target = await selected(ctx, input.selection, db);
      if (target.kind !== 'real')
        throw problem(
          'time.external_capture_required',
          'validation',
          '外部ClockSource须在本库事务前取得持久回执',
        );
      return persist(ctx, input, real(ctx.subjectId, input.realAt), db);
    },
    async readCapture(ctx: CallContext, ref: Ref): Promise<TimeCaptureReceipt> {
      await store.authorize(ctx, 'clock.references.read', ref);
      if (ref.kind !== 'TimeCapture' || ref.subjectId !== ctx.subjectId)
        throw problem(
          'time.capture_not_found',
          'not_found',
          '时间捕获回执不属于当前主体',
        );
      if (
        ref.product === 'nous' &&
        ref.installationId === store.installationId
      ) {
        const row = await store.db
          .selectFrom('timeCaptures')
          .selectAll()
          .where('id', '=', ref.id)
          .executeTakeFirst();
        if (!row || row.subjectId !== (ctx.subjectId ?? null))
          throw problem(
            'time.capture_not_found',
            'not_found',
            '时间捕获回执不存在',
          );
        return TimeCaptureReceiptSchema.parse(row.receipt);
      }
      const saved = await store.db
        .selectFrom('timeCaptures')
        .selectAll()
        .where('captureKey', '=', canonicalRefKey(ref))
        .where('subjectId', ctx.subjectId ? '=' : 'is', ctx.subjectId ?? null)
        .executeTakeFirst();
      const source = saved?.connectionRef
        ? sources[saved.connectionRef.id]
        : undefined;
      if (
        !source ||
        source.product !== ref.product ||
        source.installationId !== ref.installationId
      )
        throw problem(
          'time.source_unavailable',
          'unavailable',
          '该历史捕获的原ClockSource连接不可用',
        );
      const receipt = TimeCaptureReceiptSchema.parse(
        await source.readCapture(ctx, ref),
      );
      if (canonicalize(receipt) !== canonicalize(saved!.receipt))
        throw problem(
          'time.capture_mismatch',
          'conflict',
          '源回执与已经取得的捕获不同',
        );
      return receipt;
    },
    async validateCapture(
      ctx: CallContext,
      input: TimeCaptureReceipt,
    ): Promise<TimeCaptureReceipt> {
      const receipt = TimeCaptureReceiptSchema.parse(input),
        stored = await time.readCapture(ctx, receipt.captureRef);
      if (canonicalize(stored) !== canonicalize(receipt))
        throw problem(
          'time.capture_mismatch',
          'conflict',
          '回执与实际时间所有者记录不一致',
        );
      return stored;
    },
    close() {
      for (const registration of registrations.values()) registration.close();
      registrations.clear();
    },
  };
  options.recordings?.({
    async captureTerminal(ctx, input) {
      z.string().min(1).max(2048).parse(input.operationId);
      const observed = TimeCaptureReceiptSchema.parse(input.observed),
        origin = RefSchema.parse(input.originRef);
      if (
        origin.product !== 'nous' ||
        origin.installationId !== store.installationId ||
        (ctx.subjectId !== undefined && origin.subjectId !== ctx.subjectId) ||
        !['ModelInvocation', 'EffectOperation'].includes(origin.kind) ||
        origin.revision ||
        origin.selector
      )
        throw problem(
          'time.recording_origin_invalid',
          'validation',
          '终止记录需要本安装实际物理执行的原始身份',
        );
      const basis = await store.db
        .selectFrom('timeCaptures')
        .selectAll()
        .where('captureKey', '=', canonicalRefKey(observed.captureRef))
        .where(
          'subjectId',
          origin.subjectId ? '=' : 'is',
          origin.subjectId ?? null,
        )
        .executeTakeFirst();
      if (
        !basis ||
        canonicalize(basis.receipt) !== canonicalize(observed) ||
        canonicalize(basis.consumer) !== canonicalize(input.consumer)
      )
        throw problem(
          'time.observation_untrusted',
          'forbidden',
          '终止记录缺少本消费者已取得的原观察回执',
        );
      const selection: TimeSelection = {
        domainId: observed.point.domainId,
        ...('clockRef' in observed
          ? { preferClock: { ...observed.clockRef, revision: undefined } }
          : {}),
      };
      const request = {
        operationId: input.operationId,
        selection,
        originRef: origin,
        recordedConsumer: input.consumer,
        realAt: input.realAt,
      };
      const prior = await previous(ctx, request);
      if (prior) return prior;
      let receipt: TimeCaptureReceipt;
      if (observed.point.domainId === 'real')
        receipt = real(origin.subjectId, input.realAt);
      else {
        if (!('clockRef' in observed))
          throw problem(
            'time.observation_untrusted',
            'validation',
            '虚拟观察缺少原Clock引用',
          );
        const source = basis.connectionRef
          ? sources[basis.connectionRef.id]
          : undefined;
        if (!source?.record)
          throw problem(
            'time.terminal_source_unavailable',
            'unavailable',
            '实际ClockSource的终止记录入口不可用',
          );
        receipt = TimeCaptureReceiptSchema.parse(
          await source.record(ctx, {
            ...input,
            operationId: `nous:${store.installationId}:${input.operationId}`,
          }),
        );
        if (
          receipt.point.domainId !== observed.point.domainId ||
          !('clockRef' in receipt) ||
          canonicalRefKey({ ...receipt.clockRef, revision: undefined }) !==
            canonicalRefKey({ ...observed.clockRef, revision: undefined })
        )
          throw problem(
            'time.recording_source_mismatch',
            'conflict',
            '终止回执改变了原观察的实际时间来源',
          );
      }
      return store.transaction((db) =>
        persist(ctx, request, receipt, db, basis.connectionRef),
      );
    },
  });
  return time;
}
export type Time = ReturnType<typeof createTime>;
