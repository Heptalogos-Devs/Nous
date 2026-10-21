import { z } from 'zod';
import {
  InstantSchema,
  JsonValueSchema,
  ObjectIdSchema,
  SubjectIdSchema,
} from './common.js';
export const PromptIdSchema = z
  .string()
  .regex(/^[a-z][a-z0-9_-]*(\.[a-z][a-z0-9_-]*)+$/);
export const PromptLocaleSchema = z
  .string()
  .regex(/^[A-Za-z]{2,8}(?:-[A-Za-z0-9]{1,8})*$/);
export const PromptScopeSchema = z.discriminatedUnion('kind', [
  z.strictObject({
    kind: z.literal('installation'),
    installationId: z.string().min(1),
  }),
  z.strictObject({ kind: z.literal('subject'), subjectId: SubjectIdSchema }),
]);
export const PromptBindingSchema = z
  .strictObject({
    promptId: PromptIdSchema,
    locale: PromptLocaleSchema.optional(),
    revision: z.number().int().positive().safe().optional(),
  })
  .superRefine((value, ctx) => {
    if (value.revision !== undefined && !value.locale)
      ctx.addIssue({
        code: 'custom',
        path: ['locale'],
        message: '锁定修订需要明确语言',
      });
  });
export const PromptSnapshotRefSchema = z.strictObject({
  id: ObjectIdSchema,
  digest: z.string().regex(/^[a-f0-9]{64}$/),
});
export const PromptSnapshotSchema = z.strictObject({
  ref: PromptSnapshotRefSchema,
  promptId: PromptIdSchema,
  locale: PromptLocaleSchema,
  scope: PromptScopeSchema,
  sourceScope: PromptScopeSchema,
  source: z.enum(['program', 'override']),
  revision: z.number().int().positive().safe().optional(),
  catalogRevision: z.string(),
  paramsSchemaRevision: z.string(),
  dependencies: z.array(
    z.strictObject({
      promptId: PromptIdSchema,
      locale: PromptLocaleSchema,
      source: z.enum(['program', 'override']),
      scope: PromptScopeSchema,
      revision: z.number().int().positive().safe().optional(),
      digest: z.string(),
    }),
  ),
  engine: z.strictObject({
    name: z.literal('liquidjs'),
    version: z.string(),
    optionsDigest: z.string(),
  }),
});
export type PromptScope = z.infer<typeof PromptScopeSchema>;
export type PromptBinding = z.infer<typeof PromptBindingSchema>;
export type PromptSnapshotRef = z.infer<typeof PromptSnapshotRefSchema>;
export type PromptSnapshot = z.infer<typeof PromptSnapshotSchema>;
const selection = {
  promptId: PromptIdSchema,
  locale: PromptLocaleSchema,
  scope: PromptScopeSchema,
};
const cas = {
  ...selection,
  expectedRevision: z.number().int().nonnegative().safe(),
};
export const PromptSaveSchema = z.strictObject({
  ...cas,
  text: z.string(),
  reason: z.string().max(4096).optional(),
});
export const PromptActivateSchema = z.strictObject({
  ...cas,
  revision: z.number().int().positive().safe(),
});
export const PromptResetSchema = z.strictObject(cas);
export const PromptReadSchema = z.strictObject({
  ...selection,
  revision: z.number().int().positive().safe().optional(),
});
export const PromptPreviewSchema = z.strictObject({
  binding: PromptBindingSchema,
  scope: PromptScopeSchema,
  params: JsonValueSchema,
  unsavedText: z.string().optional(),
});
export const PromptSnapshotRequestSchema = z.strictObject({
  binding: PromptBindingSchema,
  scope: PromptScopeSchema,
});
export const PromptChangeResultSchema = z.strictObject({
  promptId: PromptIdSchema,
  locale: PromptLocaleSchema,
  scope: PromptScopeSchema,
  revision: z.number().int().nonnegative().safe(),
  latestRevision: z.number().int().nonnegative().safe(),
  activeRevision: z.number().int().positive().safe().nullable(),
  changed: z.boolean(),
  snapshot: PromptSnapshotSchema.optional(),
});
export const PromptRevisionSchema = z.strictObject({
  id: ObjectIdSchema,
  promptId: PromptIdSchema,
  locale: PromptLocaleSchema,
  scope: PromptScopeSchema,
  revision: z.number().int().positive().safe(),
  text: z.string(),
  digest: z.string(),
  paramsSchemaRevision: z.string(),
  reason: z.string().nullable(),
  recordedAt: InstantSchema,
});
export const PromptReadResultSchema = z.strictObject({
  promptId: PromptIdSchema,
  locale: PromptLocaleSchema,
  requestedLocale: PromptLocaleSchema,
  scope: PromptScopeSchema,
  sourceScope: PromptScopeSchema,
  source: z.enum(['program', 'override']),
  revision: z.number().int().nonnegative().safe(),
  sourceRevision: z.number().int().positive().safe().optional(),
  digest: z.string(),
  text: z.string(),
  latestRevision: z.number().int().nonnegative().safe(),
  activeRevision: z.number().int().positive().safe().nullable(),
});
export const PromptRenderRequestSchema = z.strictObject({
  snapshot: PromptSnapshotRefSchema,
  params: JsonValueSchema,
});
export const PromptRenderResultSchema = z.strictObject({
  text: z.string(),
  bytes: z.number().int().nonnegative(),
  snapshot: PromptSnapshotSchema,
});
export const PromptPreviewResultSchema = z.discriminatedUnion('persisted', [
  PromptRenderResultSchema.extend({ persisted: z.literal(true) }),
  z.strictObject({
    persisted: z.literal(false),
    text: z.string(),
    bytes: z.number().int().nonnegative(),
    snapshot: z.null(),
    draft: z.strictObject({
      digest: z.string(),
      promptId: PromptIdSchema,
      locale: PromptLocaleSchema,
      dependencies: PromptSnapshotSchema.shape.dependencies,
      engine: PromptSnapshotSchema.shape.engine,
    }),
  }),
]);
export type PromptSave = z.infer<typeof PromptSaveSchema>;
export type PromptActivate = z.infer<typeof PromptActivateSchema>;
export type PromptReset = z.infer<typeof PromptResetSchema>;
export type PromptRead = z.infer<typeof PromptReadSchema>;
export type PromptPreview = z.infer<typeof PromptPreviewSchema>;
export type PromptChangeResult = z.infer<typeof PromptChangeResultSchema>;
export type PromptRevision = z.infer<typeof PromptRevisionSchema>;
export type PromptReadResult = z.infer<typeof PromptReadResultSchema>;
for (const [id, schema] of Object.entries({
  PromptId: PromptIdSchema,
  PromptLocale: PromptLocaleSchema,
  PromptScope: PromptScopeSchema,
  PromptBinding: PromptBindingSchema,
  PromptSnapshotRef: PromptSnapshotRefSchema,
  PromptSnapshot: PromptSnapshotSchema,
  PromptSaveRequest: PromptSaveSchema,
  PromptActivateRequest: PromptActivateSchema,
  PromptResetRequest: PromptResetSchema,
  PromptReadRequest: PromptReadSchema,
  PromptPreviewRequest: PromptPreviewSchema,
  PromptSnapshotRequest: PromptSnapshotRequestSchema,
  PromptChangeResult: PromptChangeResultSchema,
  PromptRevision: PromptRevisionSchema,
  PromptReadResult: PromptReadResultSchema,
  PromptRenderRequest: PromptRenderRequestSchema,
  PromptRenderResult: PromptRenderResultSchema,
  PromptPreviewResult: PromptPreviewResultSchema,
}))
  z.globalRegistry.add(schema, { id });

export const PromptDescribeSchema = z.strictObject({
  owner: z.string().optional(),
  scope: PromptScopeSchema.optional(),
});
export const PromptDescriptionSchema = z.strictObject({
  id: PromptIdSchema,
  owner: z.string(),
  labelKey: z.string(),
  descriptionKey: z.string(),
  visibility: z.enum(['standard', 'advanced', 'developer']),
  locales: z.array(PromptLocaleSchema),
  defaultLocale: PromptLocaleSchema,
  paramsSchema: z.record(z.string(), JsonValueSchema),
  paramsSchemaRevision: z.string(),
  allowedPartials: z.array(PromptIdSchema),
  catalogRevision: z.string(),
});
export const PromptListSchema = z.strictObject({
  scope: PromptScopeSchema,
  cursor: z.string().max(8192).optional(),
  limit: z.number().int().min(1).max(200).optional(),
});
export const PromptHistorySchema = PromptListSchema.extend({
  promptId: PromptIdSchema,
  locale: PromptLocaleSchema,
});
export const PromptListItemSchema = z.strictObject({
  promptId: PromptIdSchema,
  locale: PromptLocaleSchema,
  scope: PromptScopeSchema,
  revision: z.number().int().nonnegative(),
  latestRevision: z.number().int().nonnegative(),
  activeRevision: z.number().int().positive().nullable(),
});
export const PromptListPageSchema = z.strictObject({
  items: z.array(PromptListItemSchema),
  nextCursor: z.string().optional(),
});
export const PromptHistoryPageSchema = z.strictObject({
  items: z.array(PromptRevisionSchema),
  nextCursor: z.string().optional(),
});
for (const [id, schema] of Object.entries({
  PromptDescribeBody: PromptDescribeSchema,
  PromptDescription: PromptDescriptionSchema,
  PromptListBody: PromptListSchema,
  PromptHistoryBody: PromptHistorySchema,
  PromptListItem: PromptListItemSchema,
  PromptListPage: PromptListPageSchema,
  PromptHistoryPage: PromptHistoryPageSchema,
}))
  z.globalRegistry.add(schema, { id });
