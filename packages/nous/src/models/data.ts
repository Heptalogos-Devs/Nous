import { serialize, deserialize } from 'node:v8';
import cacache from 'cacache';
import { canonicalize } from 'json-canonicalize';
import { sql, type Kysely } from 'kysely';
import { problem } from '@nous-wave/platform/errors';
import { readReal } from '@nous-wave/platform/time';
import type { ModelArtifactsDatabase } from './migration.js';
import type { Storage } from '../platform/context.js';
import type { ModelData } from '@nous-wave/platform/models';
import type { Ref } from '@nous-wave/contracts/common';
import type { createMaterials } from '../materials/index.js';

/** Actual Provider records belong to Materials; only this host holds the receiver. */
export function createModelData(options: {
  installationId: string;
  db: Kysely<ModelArtifactsDatabase>;
  blobRoot: string;
  authorize: Storage['authorize'];
  materials: ReturnType<typeof createMaterials>['materials'];
  recordings: ReturnType<typeof createMaterials>['recordings'];
  validateSources: ModelData['validateSources'];
}): ModelData&{holdsBlob(digest:string):Promise<boolean>} {
  const bytes = (value: unknown) =>
    serialize(
      value && typeof value === 'object'
        ? Object.fromEntries(
            Object.entries(value).filter(([key]) => key !== 'abortSignal'),
          )
        : value,
    );
  const artifactRef = (id: string, direction: 'input' | 'output'): Ref => ({
    product: 'nous',
    installationId: options.installationId,
    kind: 'ModelArtifact',
    id,
    selector: { kind: 'node', pointer: `/${direction}` },
  });
  async function saveArtifact(
    ctx: import('@nous-wave/contracts/common').CallContext,
    input: {
      identity: { invocationId: string };
      value: unknown;
      inputRefs: readonly Ref[];
    },
    direction: 'input' | 'output',
  ) {
    const raw = bytes(input.value),
      id = input.identity.invocationId,
      digest = String(
        await cacache.put(options.blobRoot, `model:${id}:${direction}`, raw),
      );
    await options.db.transaction().execute(async (db) => {
      await db
        .insertInto('modelArtifacts')
        .values({
          invocationId: id,
          direction,
          digest,
          bytes: String(raw.length),
          consumer: sql`${JSON.stringify(ctx.consumer)}::jsonb`,
          inputRefs: sql`${JSON.stringify(input.inputRefs)}::jsonb`,
          recordedAt: readReal().at,
        })
        .onConflict((oc) =>
          oc.columns(['invocationId', 'direction']).doNothing(),
        )
        .execute();
      const saved = await db
        .selectFrom('modelArtifacts')
        .select('digest')
        .where('invocationId', '=', id)
        .where('direction', '=', direction)
        .executeTakeFirstOrThrow();
      if (saved.digest !== digest)
        throw problem(
          'models.artifact_conflict',
          'conflict',
          '同一实际调用已有不同原件',
        );
    });
    return [artifactRef(id, direction)];
  }
  return {
    async holdsBlob(digest){return Boolean(await options.db.selectFrom('modelArtifacts').select('invocationId').where('digest','=',digest).limit(1).executeTakeFirst());},
    validateSources: options.validateSources,
    saveInput: (ctx, input) =>
      ctx.subjectId
        ? options.recordings.save(ctx, {
            invocationId: input.identity.invocationId,
            phase: 'input',
            bytes: bytes(input.value),
            sourceRefs: [...input.inputRefs],
          })
        : saveArtifact(ctx, input, 'input'),
    saveOutput: (ctx, input) => {
      const content = (input.value as { content?: unknown[] })?.content;
      const text = Array.isArray(content)
        ? content
            .flatMap((part) =>
              part &&
              typeof part === 'object' &&
              (part as { type?: string }).type === 'text' &&
              typeof (part as { text?: unknown }).text === 'string'
                ? [(part as { text: string }).text]
                : [],
            )
            .join('\n')
        : undefined;
      return ctx.subjectId
        ? options.recordings.save(ctx, {
            invocationId: input.identity.invocationId,
            phase: 'output',
            bytes: bytes(input.value),
            text,
            sourceRefs: [...input.inputRefs],
          })
        : saveArtifact(ctx, input, 'output');
    },
    async loadOutput(ctx, input) {
      const ref = input.outputRefs[0];
      if (!ref)
        throw problem(
          'models.output_missing',
          'not_found',
          '模型结果没有材料引用',
        );
      if (ref.kind === 'ModelArtifact') {
        if (
          ref.product !== 'nous' ||
          ref.installationId !== options.installationId ||
          ref.subjectId ||
          ref.id !== input.identity.invocationId ||
          ref.selector?.kind !== 'node' ||
          ref.selector.pointer !== '/output'
        )
          throw problem(
            'models.artifact_scope',
            'forbidden',
            '模型原件不属于本安装调用',
          );
        const row = await options.db
          .selectFrom('modelArtifacts')
          .selectAll()
          .where('invocationId', '=', ref.id)
          .where('direction', '=', 'output')
          .executeTakeFirst();
        if (!row)
          throw problem(
            'models.output_missing',
            'not_found',
            '实际模型结果原件不存在',
          );
        await options.authorize(ctx, 'models.invocations.read', {
          product: 'nous',
          installationId: options.installationId,
          kind: 'ModelInvocation',
          id: ref.id,
        });
        if (canonicalize(row.consumer) !== canonicalize(ctx.consumer))
          await options.authorize(ctx, 'data.manage', ref);
        return deserialize(
          await cacache.get.byDigest(options.blobRoot, row.digest),
        );
      }
      const material = await options.materials.read(ctx, { ref });
      if (!material.blobRef)
        throw problem('models.output_missing', 'not_found', '模型结果没有原件');
      const blob = await options.materials.readBlob(ctx, material.blobRef);
      return deserialize(blob.bytes);
    },
    async adopt(ctx, input) {
      if (
        input.call.subjectId &&
        input.call.snapshot.definition.managedBy !== 'native'
      )
        await options.recordings.adopt(ctx, input.call);
    },
  };
}
