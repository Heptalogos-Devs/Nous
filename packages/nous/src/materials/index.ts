import { requireSubject } from '../platform/security.js';
import { parseInstant } from '@nous-wave/platform/time';
import { createHash } from 'node:crypto';
import cacache from 'cacache';
import { Readable, Transform } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { sql } from 'kysely';
import {
  executeWithCursorPagination,
  defaultDecodeCursor,
} from 'kysely-paginate';
import { z } from 'zod';
import { v7 as uuid } from 'uuid';
import {
  canonicalRefKey,
  type CallContext,
  type Ref,
  type Policy,
  type TimePoint,
  type TimeRange,
} from '@nous-wave/contracts/common';
import {
  BlobMetadataSchema,
  MaterialCaptureSchema,
  MaterialViewSchema,
  MaterialPublishSchema,
  EncounterAcceptSchema,
  ExperienceListSchema,
  type ExperiencePage,
  type MaterialView,
  type ExperienceItem,
  type SourceResolution,
  type BlobMetadata,
  type BlobResult,
} from '@nous-wave/contracts/materials';
import {
  mayRead,
  privatePolicy,
  intersectPolicy,
  isAccessDenied,
  mayReadScope,
  requireConsumer,
} from '../platform/security.js';
import { DomainError, fail } from '../platform/errors.js';
import { json, now, reference, type Storage } from '../platform/context.js';
import { selectText } from '../platform/text.js';
import { createSourceRepository } from './source-repository.js';
import jsonpointer from 'jsonpointer';
import type { Database } from '../platform/database.js';
export function createMaterials(
  store: Storage & {
    blobPath: string;
    readableSources(ctx: CallContext, refs: Ref[]): Promise<Ref[]>;
    resolveSources(ctx: CallContext, refs: Ref[]): Promise<SourceResolution>;
    inheritSources(ctx: CallContext, refs: Ref[]): Promise<SourceResolution>;
    externalPolicies(ctx: CallContext, roots: Ref[]): Promise<Policy[]>;
  },
) {
  const sourceRepository = createSourceRepository(store);
  async function linkInputs(
    subjectId: string,
    fromRef: Ref,
    inputs: Ref[],
    roots: Ref[],
    db = store.db,
  ) {
    if (inputs.length)
      await db
        .insertInto('sourceLinks')
        .values(
          inputs.map((toRef) => ({
            id: uuid(),
            subjectId: subjectId,
            fromRef: json(fromRef),
            toRef: json(toRef),
            relation: 'derived_from',
            roots: json(roots),
          })),
        )
        .onConflict((conflict) =>
          conflict
            .columns(['subjectId', 'fromRef', 'toRef', 'relation'])
            .doNothing(),
        )
        .execute();
  }
  const uploaderKey = (ctx: CallContext) =>
    JSON.stringify([
      ctx.consumer.kind,
      ctx.consumer.host,
      ctx.consumer.id,
      ctx.subjectId ?? null,
      ctx.actor.kind === 'human' ? ctx.actor.id : null,
    ]);
  const submissionKey = (ctx: CallContext) =>
    JSON.stringify([uploaderKey(ctx), ctx.trace.operationId]);
  const uploadKey = (ctx: CallContext) =>
    `blob:${ctx.subjectId}:${submissionKey(ctx)}`;
  async function registerBlob(
    ctx: CallContext,
    metadata: BlobMetadata,
    key: string,
    installation = false,
    client?: Storage['db'],
    operation?: string,
  ): Promise<BlobResult> {
    if(!installation)requireSubject(ctx);
    const subjectId=installation?null:ctx.subjectId!;
    const adopt=async (db:Storage['db']):Promise<BlobResult> => {
      if(operation)await store.authorize(ctx,operation,undefined,db);
      await db
        .insertInto('blobs')
        .values({
          id: uuid(),
          subjectId,
          submissionKey: key,
          ...metadata,
          recordedAt: now().at,
        })
        .onConflict((conflict) =>
          conflict.columns(['subjectId', 'submissionKey']).doNothing(),
        )
        .execute();
      let query = db
        .selectFrom('blobs')
        .selectAll()
        .where('submissionKey', '=', key);
      query=subjectId===null?query.where('subjectId','is',null):query.where('subjectId','=',subjectId);
      const actual=await query.executeTakeFirstOrThrow();
      if (
        actual.digest !== metadata.digest ||
        Number(actual.bytes) !== metadata.bytes ||
        actual.mediaType !== metadata.mediaType
      )
        fail('conflict', '同一上传提交已保存不同的原件');
      return {
        ref: {product:'nous',installationId:store.installationId,...(subjectId?{subjectId}:{}),kind:'Blob',id:actual.id},
        digest: actual.digest,
        bytes: Number(actual.bytes),
        mediaType: actual.mediaType,
      };
    };
    return client?adopt(client):store.transaction(adopt);
  }
  async function acceptBlob(
    ctx: CallContext,
    input: BlobMetadata,
  ): Promise<BlobResult> {
    requireSubject(ctx);
    await store.authorize(ctx, 'materials.capture');
    const metadata = BlobMetadataSchema.parse(input),
      stored = await cacache.get.info(store.blobPath, uploadKey(ctx));
    if (
      !stored ||
      String(stored.integrity) !== metadata.digest ||
      stored.size !== metadata.bytes ||
      (stored.metadata as { mediaType?: string })?.mediaType !==
        metadata.mediaType
    )
      fail('invalid_input', '原件元数据没有对应本次已完成的上传');
    return registerBlob(ctx, metadata, submissionKey(ctx),false,undefined,'materials.capture');
  }
  async function storeBytes(
    input: Readable,
    mediaType: string,
    maxBytes: number,
    key: string,
  ): Promise<BlobMetadata> {
    if (!Number.isSafeInteger(maxBytes) || maxBytes <= 0)
      fail('invalid_input', '上传大小上限无效');
    let bytes = 0,
      digest = '';
    const measure = new Transform({
      transform(chunk: Buffer, _encoding, done) {
        bytes += chunk.length;
        if (bytes > maxBytes)
          done(new DomainError('budget_exhausted', '原件超过单件上传大小'));
        else done(null, chunk);
      },
    });
    const stagingKey=`${key}:upload:${uuid()}`;
    const writer = cacache.put.stream(
      store.blobPath,
      stagingKey,
      { metadata: { mediaType } },
    );
    writer.on('integrity', (value) => {
      digest = String(value);
    });
    await pipeline(input, measure, writer);
    if (!digest) fail('dependency_unavailable', '原件存储未返回完整性摘要');
    await cacache.index.insert(store.blobPath,`blob-content:${digest}`,digest,{size:bytes,metadata:{mediaType}});
    await cacache.index.insert(store.blobPath,key,digest,{size:bytes,metadata:{mediaType}});
    await cacache.rm.entry(store.blobPath,stagingKey);
    return { digest, bytes, mediaType };
  }
  async function storeBlob(ctx:CallContext,input:Readable,mediaType:string,maxBytes:number,key:string,operation?:string):Promise<BlobResult>{requireSubject(ctx);return registerBlob(ctx,await storeBytes(input,mediaType,maxBytes,`blob:${ctx.subjectId}:${key}`),key,false,undefined,operation);}
  async function putBlobStream(
    ctx: CallContext,
    input: Readable,
    mediaType: string,
    maxBytes = 64 * 1024 * 1024,
  ): Promise<BlobResult> {
    requireSubject(ctx);
    await store.authorize(ctx, 'materials.capture');
    return storeBlob(ctx, input, mediaType, maxBytes, submissionKey(ctx),'materials.capture');
  }
  async function blobRecord(ctx: CallContext, ref: Ref) {
    requireSubject(ctx);
    if (
      ref.product !== 'nous' ||
      ref.installationId !== store.installationId ||
      ref.subjectId !== ctx.subjectId ||
      ref.kind !== 'Blob' ||
      ref.revision ||
      ref.selector
    )
      fail('not_found', '原件不属于当前安装与主体');
    const row = await store.db
      .selectFrom('blobs')
      .selectAll()
      .where('id', '=', ref.id)
      .where('subjectId', '=', ctx.subjectId)
      .executeTakeFirst();
    if (!row) fail('not_found', '原件不存在');
    return row;
  }
  async function blobHolders(ctx: CallContext, ref: Ref) {
    requireSubject(ctx);
    return store.db
      .selectFrom('materialVersions')
      .innerJoin('materials', 'materials.id', 'materialVersions.materialId')
      .select(['materials.id', 'materialVersions.version'])
      .where('materials.subjectId', '=', ctx.subjectId)
      .where('materials.status', '!=', 'purged')
      .where(sql<boolean>`material_versions.body->'blobRef'->>'id' = ${ref.id}`)
      .execute();
  }
  async function readableHolder(
    ctx: CallContext,
    holders: Array<{ id: string; version: number }>,
  ) {
    requireSubject(ctx);
    for (const holder of holders) {
      try {
        return await read(ctx, {
          ref: reference(
            store.installationId,
            ctx.subjectId,
            'Material',
            holder.id,
            holder.version,
          ),
        });
      } catch (error) {
        if (
          isAccessDenied(error) ||
          (error instanceof DomainError && error.code === 'not_found')
        )
          continue;
        throw error;
      }
    }
    fail('forbidden', '原件没有本次用途与听众可读的材料持有者');
  }
  async function read(
    ctx: CallContext,
    input: { ref: Ref },
    db = store.db,
  ): Promise<MaterialView> {
    requireSubject(ctx);
    const { ref } = input;
    await store.authorize(ctx, 'materials.read', ref, db);
    if (
      ref.product !== 'nous' ||
      ref.installationId !== store.installationId ||
      ref.subjectId !== ctx.subjectId
    )
      fail('not_found', '资料不属于当前认知安装与主体');
    const row = await db
      .selectFrom('materials')
      .selectAll()
      .where('id', '=', ref.id)
      .where('subjectId', '=', ctx.subjectId)
      .executeTakeFirst();
    if (!row || row.status === 'purged') fail('not_found', '材料不存在');
    if (!mayRead(ctx, row.policy)) fail('forbidden', '材料的用途或受众不可读');
    const version = await db
      .selectFrom('materialVersions')
      .select('body')
      .where('materialId', '=', ref.id)
      .where('version', '=', ref.revision ?? row.currentVersion)
      .executeTakeFirst();
    if (!version) fail('not_found', '材料版本不存在');
    const view = MaterialViewSchema.parse({
      ...version.body,
      policy: row.policy,
    });
    if (!mayReadScope(ctx, view.scope))
      await store.authorize(ctx, 'data.manage', view.ref, db);
    const inherited = view.source.inputs.length
      ? (await store.inheritSources(ctx, view.source.inputs)).policies
      : await store.externalPolicies(ctx, view.source.roots);
    view.policy = intersectPolicy([row.policy, ...inherited]);
    if (!mayRead(ctx, view.policy))
      fail('forbidden', '材料来源已限制本次用途或听众');
    if (ref.selector) {
      const selector = ref.selector;
      if (view.text === undefined)
        fail('capability_unavailable', '该材料尚未提供所选区域的表示');
      if (selector.kind === 'node') {
        if (
          !view.mediaType.includes('json') ||
          !(selector.pointer === '' || selector.pointer.startsWith('/')) ||
          selector.within
        )
          fail('invalid_input', '节点区域需要 JSON 材料与绝对 JSON Pointer');
        let selected: unknown;
        try {
          const document = JSON.parse(view.text);
          selected =
            selector.pointer === ''
              ? document
              : jsonpointer.get(document, selector.pointer);
        } catch {
          fail('invalid_input', '材料没有可读取的 JSON 节点');
        }
        if (typeof selected !== 'string')
          fail('invalid_input', '所选 JSON 节点不是文本');
        view.text = selected;
        view.mediaType = 'text/plain';
        view.ref = { ...view.ref, selector };
      } else {
        const selection = selectText(view.text, selector);
        view.text = selection.text;
        view.ref = { ...view.ref, selector: selection.selector };
      }
    }
    return view;
  }
  async function writeView(
    ctx: CallContext,
    view: MaterialView,
    db: Storage['db'],
  ) {
    requireSubject(ctx);
    await db
      .insertInto('materials')
      .values({
        id: view.ref.id,
        subjectId: ctx.subjectId,
        currentVersion: 1,
        policy: json(view.policy),
        status: 'active',
        createdAt: view.recordedAt,
      })
      .execute();
    await db
      .insertInto('materialVersions')
      .values({
        id: uuid(),
        materialId: view.ref.id,
        version: 1,
        body: json(view),
        recordedAt: view.recordedAt,
      })
      .execute();
    await linkInputs(
      ctx.subjectId,
      view.ref,
      view.source.inputs,
      view.source.roots,
      db,
    );
    return view;
  }
  async function writeEncounter(
    subjectId: string,
    body: import('@nous-wave/contracts/materials').EncounterAccept,
    roots: Ref[],
  ) {
    const stepKey = JSON.stringify([
      body.consumer.host,
      body.invocationId,
      body.step,
    ]);
    const hash = createHash('sha256')
      .update(JSON.stringify(body))
      .digest('hex');
    for (const portion of body.portions)
      if (
        !['Encounter', 'ExperienceItem'].includes(portion.ref.kind) &&
        portion.ref.revision === undefined
      )
        fail('invalid_input', '实际输入必须固定读取的修订');
    return store.transaction(async (db) => {
      const id = uuid(),
        at = now().at;
      await db
        .insertInto('encounters')
        .values({
          id,
          subjectId: subjectId,
          stepKey,
          inputHash: hash,
          body: json(body),
          recordedAt: at,
        })
        .onConflict((oc) => oc.columns(['subjectId', 'stepKey']).doNothing())
        .execute();
      const actual = await db
        .selectFrom('encounters')
        .selectAll()
        .where('subjectId', '=', subjectId)
        .where('stepKey', '=', stepKey)
        .executeTakeFirstOrThrow();
      if (actual.inputHash !== hash)
        fail('conflict', '同一实际步骤已经接纳不同输入');
      const ref = reference(
        store.installationId,
        subjectId,
        'Encounter',
        actual.id,
      );
      if (actual.id !== id)
        return {
          ...actual.body,
          ref,
          recordedAt: parseInstant(actual.recordedAt),
        };
      await linkInputs(
        subjectId,
        ref,
        body.portions.map((portion) => portion.ref),
        [],
        db,
      );
      if (body.consumer.kind === 'subject') {
        const experience: ExperienceItem = {
          ref: reference(store.installationId, subjectId, 'ExperienceItem', id),
          encounterRef: ref,
          observed: body.observed,
          portions: body.portions,
          activityRef: body.activityRef,
          contextRef: body.contextRef,
          sourceRoots: roots,
        };
        await db
          .insertInto('experienceItems')
          .values({
            id,
            subjectId: subjectId,
            encounterId: actual.id,
            body: json(experience),
            observedAt: body.observed.at,
            domainId: body.observed.domainId,
            recordedAt: at,
            processed: false,
          })
          .onConflict((oc) => oc.column('encounterId').doNothing())
          .execute();
        const savedExperience = await db
          .selectFrom('experienceItems')
          .select('id')
          .where('encounterId', '=', actual.id)
          .executeTakeFirstOrThrow();
        await linkInputs(
          subjectId,
          reference(
            store.installationId,
            subjectId,
            'ExperienceItem',
            savedExperience.id,
          ),
          body.portions.map((portion) => portion.ref),
          roots,
          db,
        );
        await db
          .insertInto('useEvents')
          .values({
            id: uuid(),
            subjectId: subjectId,
            stepKey,
            kind: 'presented',
            refs: json(body.portions.map((p) => p.ref)),
            recordedAt: at,
          })
          .onConflict((oc) =>
            oc.columns(['subjectId', 'stepKey', 'kind']).doNothing(),
          )
          .execute();
      }
      return {
        ...actual.body,
        ref,
        recordedAt: parseInstant(actual.recordedAt),
      };
    });
  }
  const materials = {
    read,
    async sourceTimes(ctx: CallContext, refs: Ref[]) {
      requireSubject(ctx);
      const occurred: NonNullable<MaterialView['occurred']>[] = [],
        observed: TimePoint[] = [];
      for (const ref of refs) {
        if (ref.kind === 'Material') {
          const material = await read(ctx, { ref });
          if (material.occurred) occurred.push(material.occurred);
        }
      }
      const rows = await store.db
        .selectFrom('encounters')
        .select('body')
        .where('subjectId', '=', ctx.subjectId)
        .execute();
      for (const row of rows) {
        if (
          row.body.consumer.kind === 'subject' &&
          row.body.portions.some((p) =>
            refs.some(
              (r) =>
                r.id === p.ref.id &&
                (r.revision === undefined || r.revision === p.ref.revision),
            ),
          )
        )
          observed.push(row.body.observed);
      }
      return { occurred, observed };
    },
    async capture(
      ctx: CallContext,
      input: unknown,
      client?: Storage['db'],
    ): Promise<MaterialView> {
      requireSubject(ctx);
      await store.authorize(ctx, 'materials.capture');
      const body = MaterialCaptureSchema.parse(input);
      const id = uuid();
      const at = now().at;
      const ref = reference(
        store.installationId,
        ctx.subjectId,
        'Material',
        id,
        1,
      );
      if (body.blobRef) {
        const blob = await blobRecord(ctx, body.blobRef);
        const holders = await blobHolders(ctx, body.blobRef);
        if (holders.length) {
          const holder = await readableHolder(ctx, holders);
          body.source.inputs.push(holder.ref);
        } else {
          let uploader: unknown;
          try {
            uploader = JSON.parse(blob.submissionKey);
          } catch {}
          if (!Array.isArray(uploader) || uploader[0] !== uploaderKey(ctx))
            fail('forbidden', '本次材料没有取得该原件的上传或读取授权');
        }
      }
      const dependency = await store.resolveSources(ctx, body.source.inputs);
      const policy = intersectPolicy([
        ...(body.policy
          ? [body.policy]
          : dependency.policies.length
            ? []
            : [privatePolicy(ctx.subjectId)]),
        ...dependency.policies,
      ]);
      if (
        !body.source.inputs.length &&
        [
          ...body.source.roots,
          ...(body.source.ref ? [body.source.ref] : []),
        ].some(
          (root) =>
            root.product === 'nous' &&
            root.installationId === store.installationId &&
            [
              'Material',
              'Memory',
              'Episode',
              'Journal',
              'Schema',
              'Concept',
              'Self',
              'Social',
              'LanguageConvention',
              'Motivation',
              'Encounter',
              'ExperienceItem',
            ].includes(root.kind),
        )
      )
        fail(
          'invalid_input',
          '内部来源表示通过 source.inputs 声明，来源根由所属对象解析',
        );
      const roots = dependency.resolvedRefs.length
        ? dependency.roots
        : body.source.roots.length
          ? body.source.roots
          : [body.source.ref ?? ref];
      const view: MaterialView = {
        ...body,
        scope: ctx.consumer,
        source: {
          ...body.source,
          inputs: dependency.resolvedRefs,
          roots: [
            ...new Map(
              roots.map((root) => [canonicalRefKey(root), root]),
            ).values(),
          ],
        },
        ref,
        policy,
        recordedAt: at,
      };
      const write = (db: Storage['db']) => writeView(ctx, view, db);
      return client ? write(client) : store.transaction(write);
    },
    async publish(ctx: CallContext, input: unknown): Promise<MaterialView> {
      requireSubject(ctx);
      const body = MaterialPublishSchema.parse(input);
      await store.authorize(ctx, 'materials.publish', body.ref);
      const selected = await read(ctx, { ref: body.ref });
      if (['model_input_raw', 'model_output_raw'].includes(selected.kind))
        fail('invalid_input', '请选择模型实际结果的可读成果投影');
      if (selected.scope.kind === 'subject')
        fail('invalid_input', '选定材料已经属于主体可读范围');
      return store.transaction(async (db) => {
        const id = uuid(),
          at = now().at,
          ref = reference(
            store.installationId,
            ctx.subjectId,
            'Material',
            id,
            1,
          );
        const view: MaterialView = {
          ...selected,
          kind: 'worker_result',
          title: body.title ?? selected.title,
          ref,
          scope: { kind: 'subject', host: 'nous', id: ctx.subjectId },
          recordedAt: at,
          source: { inputs: [selected.ref], roots: selected.source.roots },
        };
        if (selected.ref.selector) delete view.blobRef;
        return writeView(ctx, view, db);
      });
    },
    acceptBlob,
    putBlobStream,
    async putBlob(ctx: CallContext, bytes: Buffer, mediaType: string) {
      requireSubject(ctx);
      return putBlobStream(ctx, Readable.from([bytes]), mediaType);
    },
    async readBlob(ctx: CallContext, ref: Ref) {
      requireSubject(ctx);
      await store.authorize(ctx, 'materials.read');
      const row = await blobRecord(ctx, ref);
      await readableHolder(ctx, await blobHolders(ctx, ref));
      return {
        bytes: await cacache.get.byDigest(store.blobPath, row.digest),
        mediaType: row.mediaType,
      };
    },
    async updatePolicy(ctx: CallContext, input: { ref: Ref; policy: Policy }) {
      requireSubject(ctx);
      await store.authorize(ctx, 'policy.update', input.ref);
      return store.transaction(async (db) => {
        const updated = await db
          .updateTable('materials')
          .set({ policy: json(input.policy) })
          .where('id', '=', input.ref.id)
          .where('subjectId', '=', ctx.subjectId)
          .returning('id')
          .executeTakeFirst();
        if (!updated) fail('not_found', '材料不存在');
        return { ref: input.ref, policy: input.policy };
      });
    },
    async list(ctx: CallContext) {
      requireSubject(ctx);
      await store.authorize(ctx, 'materials.read');
      const rows = await store.db
        .selectFrom('materials')
        .selectAll()
        .where('subjectId', '=', ctx.subjectId)
        .where('status', '!=', 'purged')
        .execute();
      const items: MaterialView[] = [];
      for (const row of rows.filter((r) => mayRead(ctx, r.policy))) {
        try {
          items.push(
            await read(ctx, {
              ref: reference(
                store.installationId,
                ctx.subjectId,
                'Material',
                row.id,
              ),
            }),
          );
        } catch (error) {
          if (
            isAccessDenied(error) ||
            (error instanceof DomainError && error.code === 'not_found')
          )
            continue;
          throw error;
        }
      }
      return items;
    },
  };
  const encounters = {
    async inputs(ctx: CallContext, ref: Ref) {
      requireSubject(ctx);
      await store.authorize(ctx, 'encounters.read', ref);
      if (ref.kind === 'Encounter') {
        const row = await store.db
          .selectFrom('encounters')
          .select('body')
          .where('subjectId', '=', ctx.subjectId)
          .where('id', '=', ref.id)
          .executeTakeFirst();
        if (!row) fail('not_found', '实际接触不存在');
        if (!mayReadScope(ctx, row.body.consumer))
          await store.authorize(ctx, 'data.manage', ref);
        return row.body.portions.map((p) => p.ref);
      }
      const row = await store.db
        .selectFrom('experienceItems')
        .select('body')
        .where('subjectId', '=', ctx.subjectId)
        .where('id', '=', ref.id)
        .executeTakeFirst();
      if (!row) fail('not_found', '实际经历不存在');
      return row.body.portions.map((p) => p.ref);
    },
    async collect(
      ctx: CallContext,
      input: {
        limit: number;
        refs?: Ref[];
        after?: { observed: TimePoint; id: string };
      },
    ) {
      requireSubject(ctx);
      await store.authorize(ctx, 'encounters.read');
      let query = store.db
        .selectFrom('experienceItems')
        .select('body')
        .where('subjectId', '=', ctx.subjectId)
        .where('processed', '=', false);
      if (input.refs?.length)
        query = query.where(
          'id',
          'in',
          input.refs.map((r) => r.id),
        );
      if (input.after) {
        const after = input.after;
        query = query.where((eb) =>
          eb.or([
            eb('observedAt', '>', after.observed.at),
            eb.and([
              eb('observedAt', '=', after.observed.at),
              eb('id', '>', after.id),
            ]),
          ]),
        );
      }
      return (
        await query
          .orderBy('observedAt')
          .orderBy('id')
          .limit(input.limit)
          .execute()
      ).map((r) => r.body);
    },
    async markProcessed(ctx: CallContext, refs: Ref[]) {
      requireSubject(ctx);
      await store.authorize(ctx, 'growth.run');
      if (refs.length)
        await store.db
          .updateTable('experienceItems')
          .set({ processed: true })
          .where('subjectId', '=', ctx.subjectId)
          .where(
            'id',
            'in',
            refs.map((r) => r.id),
          )
          .execute();
    },
    async accept(ctx: CallContext, input: unknown) {
      requireSubject(ctx);
      await store.authorize(ctx, 'encounters.accept');
      const body = EncounterAcceptSchema.parse(input);
      requireConsumer(ctx, body.consumer);
      const roots: Ref[] = [];
      if (body.consumer.kind === 'subject')
        for (const portion of body.portions) {
          if (portion.ref.kind === 'Material') {
            const m = await read(ctx, { ref: portion.ref });
            roots.push(...m.source.roots);
          }
        }
      return writeEncounter(ctx.subjectId, body, roots);
    },
    async list(ctx: CallContext) {
      requireSubject(ctx);
      await store.authorize(ctx, 'encounters.read');
      const rows = await store.db
        .selectFrom('encounters')
        .selectAll()
        .where('subjectId', '=', ctx.subjectId)
        .orderBy('recordedAt')
        .execute();
      const items = [];
      for (const row of rows) {
        const ref = reference(
          store.installationId,
          ctx.subjectId,
          'Encounter',
          row.id,
        );
        try {
          await store.authorize(ctx, 'encounters.read', ref);
          if (!mayReadScope(ctx, row.body.consumer))
            await store.authorize(ctx, 'data.manage', ref);
          items.push(row);
        } catch (error) {
          if (isAccessDenied(error)) continue;
          throw error;
        }
      }
      return items;
    },
    async experiences(
      ctx: CallContext,
      input: z.input<typeof ExperienceListSchema> = {},
    ): Promise<ExperiencePage> {
      requireSubject(ctx);
      await store.authorize(ctx, 'encounters.read');
      const request = ExperienceListSchema.parse(input),
        { cursor, ...filter } = request,
        scope = createHash('sha256')
          .update(
            JSON.stringify({
              subjectId: ctx.subjectId,
              actor: ctx.actor,
              consumer: ctx.consumer,
              purpose: ctx.purpose,
              audience: ctx.audience,
              filter,
            }),
          )
          .digest('hex');
      let query = store.db
        .selectFrom('experienceItems')
        .select(['id', 'body', 'observedAt as position'])
        .select(sql<string>`${scope}`.as('scope'))
        .where('subjectId', '=', ctx.subjectId);
      if (request.refs) {
        if (!request.refs.length) return { items: [] };
        query = query.where(
          'id',
          'in',
          request.refs.map((ref) => ref.id),
        );
      }
      if (request.range?.start)
        query = query
          .where('domainId', '=', request.range.start.domainId)
          .where('observedAt', '>=', request.range.start.at);
      if (request.range?.end)
        query = query
          .where('domainId', '=', request.range.end.domainId)
          .where('observedAt', '<', request.range.end.at);
      const fields = [
        { expression: sql<string>`${scope}`, key: 'scope', direction: 'asc' },
        { expression: 'observedAt', key: 'position', direction: 'asc' },
        { expression: 'id', key: 'id', direction: 'asc' },
      ] as const;
      const page = await executeWithCursorPagination(query, {
        perPage: request.limit,
        after: cursor,
        fields,
        decodeCursor: (value, keys) => {
          try {
            return defaultDecodeCursor<
              Database,
              'experienceItems',
              {
                id: string;
                body: ExperienceItem;
                position: string;
                scope: string;
              },
              typeof fields
            >(value, keys);
          } catch {
            fail('invalid_input', '经历游标无效');
          }
        },
        parseCursor: (value) => {
          if (
            value.scope !== scope ||
            !z.uuid().safeParse(value.id).success ||
            !z.iso.datetime().safeParse(value.position).success
          )
            fail('invalid_input', '经历游标不属于本次读取范围');
          return value;
        },
      });
      const readable = new Set(
        (
          await store.readableSources(
            ctx,
            page.rows.map((row) => row.body.ref),
          )
        ).map((ref) => ref.id),
      );
      return {
        items: page.rows
          .map((row) => row.body)
          .filter((item) => readable.has(item.ref.id)),
        nextCursor: page.hasNextPage ? page.endCursor : undefined,
      };
    },
  };
  // Held only by the actual model receiver in this composition. Public material
  // reads keep current authorization; completed Provider facts do not re-invoke.
  async function modelRecord(
    subjectId: string,
    invocationId: string,
    phase: 'input' | 'output' | 'output-text',
    db = store.db,
  ) {
    const row = await db
      .selectFrom('materialVersions')
      .innerJoin('materials', 'materials.id', 'materialVersions.materialId')
      .select('materialVersions.body')
      .where('materials.subjectId', '=', subjectId)
      .where(
        sql<boolean>`material_versions.body->'source'->'ref'->>'kind'='ModelInvocation'`,
      )
      .where(
        sql<boolean>`material_versions.body->'source'->'ref'->>'id'=${invocationId}`,
      )
      .where(
        sql<boolean>`material_versions.body->'source'->>'originVersion'=${phase}`,
      )
      .where('materials.status', '!=', 'purged')
      .executeTakeFirst();
    return row ? MaterialViewSchema.parse(row.body) : undefined;
  }
  const recordings = {
    async save(
      ctx: CallContext,
      input: {
        invocationId: string;
        phase: 'input' | 'output';
        bytes: Uint8Array;
        text?: string;
        sourceRefs: Ref[];
      },
    ): Promise<Ref[]> {
      requireSubject(ctx);
      let dependency: SourceResolution,
        scope = ctx.consumer;
      if (input.phase === 'input') {
        await store.authorize(ctx, 'materials.capture');
        dependency = await store.resolveSources(ctx, input.sourceRefs);
      } else {
        const saved = await modelRecord(
          ctx.subjectId,
          input.invocationId,
          'input',
        );
        if (!saved) fail('conflict', '实际模型结果缺少原输入记录');
        dependency = {
          resolvedRefs: [saved.ref],
          roots: saved.source.roots,
          policies: [saved.policy],
        };
        scope = saved.scope;
      }
      const blob = await storeBlob(
        ctx,
        Readable.from([input.bytes]),
        'application/vnd.nous.model-record+v8',
        Math.max(input.bytes.byteLength, 1),
        `model:${input.invocationId}:${input.phase}`,
      );
      return store.transaction(async (db) => {
        await sql`select pg_advisory_xact_lock(hashtextextended(${`model-material:${input.invocationId}:${input.phase}`},0))`.execute(
          db,
        );
        const existing = await modelRecord(
          ctx.subjectId,
          input.invocationId,
          input.phase,
          db,
        );
        if (existing) {
          if (existing.blobRef?.id !== blob.ref.id)
            fail('conflict', '同一实际模型步骤已有不同记录');
          const projected = await modelRecord(
            ctx.subjectId,
            input.invocationId,
            'output-text',
            db,
          );
          return [existing.ref, ...(projected ? [projected.ref] : [])];
        }
        const ref = reference(
          store.installationId,
          ctx.subjectId,
          'Material',
          uuid(),
          1,
        );
        const policy = intersectPolicy(
          dependency.policies.length
            ? dependency.policies
            : [privatePolicy(ctx.subjectId)],
        );
        const view: MaterialView = {
          ref,
          kind: `model_${input.phase}_raw`,
          mediaType: blob.mediaType,
          blobRef: blob.ref,
          scope,
          source: {
            ref: reference(
              store.installationId,
              ctx.subjectId,
              'ModelInvocation',
              input.invocationId,
            ),
            originVersion: input.phase,
            inputs: dependency.resolvedRefs,
            roots: dependency.roots.length ? dependency.roots : [ref],
          },
          policy,
          recordedAt: now().at,
        };
        await writeView(ctx, view, db);
        if (input.phase === 'output' && input.text !== undefined) {
          const { blobRef: _blob, ...metadata } = view;
          const projected: MaterialView = {
            ...metadata,
            ref: reference(
              store.installationId,
              ctx.subjectId,
              'Material',
              uuid(),
              1,
            ),
            kind: 'model_output',
            mediaType: 'text/plain',
            text: input.text,
            source: {
              ...view.source,
              inputs: [ref],
              originVersion: 'output-text',
            },
          };
          await writeView(ctx, projected, db);
          return [ref, projected.ref];
        }
        return [ref];
      });
    },
    async adopt(
      ctx: CallContext,
      call: import('@nous-wave/contracts/models').ModelCallView,
    ) {
      if (
        !call.subjectId ||
        (ctx.subjectId && call.subjectId !== ctx.subjectId) ||
        !call.observed
      )
        fail('conflict', '实际模型接纳缺少原主体或观察回执');
      const subjectId = call.subjectId;
      const input = await modelRecord(
          subjectId,
          call.identity.invocationId,
          'input',
        ),
        output = await modelRecord(
          subjectId,
          call.identity.invocationId,
          'output',
        );
      if (!input || !output) fail('conflict', '实际模型接纳缺少输入或结果记录');
      const presented =
        (await modelRecord(
          subjectId,
          call.identity.invocationId,
          'output-text',
        )) ?? output;
      return writeEncounter(
        subjectId,
        EncounterAcceptSchema.parse({
          consumer: call.consumer,
          invocationId: call.identity.invocationId,
          step: call.identity.physicalOrdinal,
          contextViewRef: call.preparation?.contextRef,
          portions: [{ ref: input.ref }, { ref: presented.ref }],
          resultRef: presented.ref,
          observed: call.observed.point,
          purpose: call.purpose,
          completion: 'complete',
        }),
        input.source.roots,
      );
    },
  };
  const artifactBlobs={
    async store(ctx:CallContext,input:Readable,options:{operation:'data.import'|'data.export';mediaType:string;maxBytes:number;key:string}){await store.authorize(ctx,options.operation);return storeBytes(input,options.mediaType,options.maxBytes,`data:${options.key}`);},
    async accept(ctx:CallContext,metadata:BlobMetadata,key:string,db:Storage['db']){const actual=await cacache.get.info(store.blobPath,`data:${key}`);if(!actual||String(actual.integrity)!==metadata.digest||actual.size!==metadata.bytes)fail('invalid_input','归档原件缺少本次完整上传');return registerBlob(ctx,metadata,`data:${key}`,true,db);},
    async open(ref:Ref):Promise<Readable>{if(ref.product!=='nous'||ref.installationId!==store.installationId||ref.kind!=='Blob'||ref.subjectId||ref.selector||ref.revision)fail('forbidden','需要本安装归档原件');const row=await store.db.selectFrom('blobs').selectAll().where('id','=',ref.id).where('subjectId','is',null).executeTakeFirst();if(!row)fail('not_found','归档原件不存在');return Readable.from(cacache.get.stream.byDigest(store.blobPath,row.digest));},
    async release(ref:Ref,db:Storage['db']){const row=await db.deleteFrom('blobs').where('id','=',ref.id).where('subjectId','is',null).returning('digest').executeTakeFirst();return row?.digest;},
  };
  return { materials, encounters, sourceRepository, recordings, artifactBlobs };
}
