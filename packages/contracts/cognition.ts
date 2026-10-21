import { z } from 'zod';
import { ConsumerSchema, PolicySchema, RefSchema, SelectorSchema, TimePointSchema, TimeRangeSchema, TimePointInputSchema, InstantInputSchema, InstantSchema } from './common.js';
import { KnowledgeCreateSchema, KnowledgeKindSchema } from './knowledge.js';
import {ConfigSnapshotSchema} from './settings.js';
import { MaterialViewSchema } from './materials.js';

export const QuerySchema = z.object({
  intent: z.string().default(''),
  subjectId: z.string().optional(),
  kinds: z.array(KnowledgeKindSchema).optional(),
  entities: z.array(z.string()).default([]),
  entityMatch: z.enum(['any', 'all']).default('any'),
  concepts: z.array(z.string()).default([]),
  tags: z.array(z.string()).default([]),
  tagMatch: z.enum(['any', 'all']).default('any'),
  time: z.object({
    field: z.enum(['occurred', 'observed', 'formed', 'valid', 'recorded']),
    domainId: z.string(),
    start: InstantInputSchema,
    end: InstantInputSchema,
  }).optional(),
  knowledgeAt: TimePointInputSchema.optional(),
  recordedAsOf: InstantInputSchema.optional(),
  refs: z.array(z.string()).default([]),
  sources: z.array(z.string()).default([]),
  preferences: z.array(z.enum(['recent', 'independent_sources', 'diverse'])).default([]),
  exploration: z.number().int().min(0).max(4).default(0),
  residual: z.boolean().default(false),
  expand: z.array(z.enum(['sources', 'relations'])).default([]),
  limit: z.number().int().positive().default(12),
  cursor: z.string().optional(),
  workContextId: z.string().optional(),
});
export type Query = z.infer<typeof QuerySchema>;
export type QueryInput = string | z.input<typeof QuerySchema>;
export const QueryInputSchema = z.union([z.string(), QuerySchema]);
export const QueryItemSchema = z.object({
  ref: RefSchema,
  title: z.string(),
  excerpt: z.string(),
  formation: z.string(),
  stance: z.string(),
  confidence: z.number().optional(),
  time: z.object({
    formed: TimePointSchema,
    valid: TimeRangeSchema.optional(),
    recorded: TimePointSchema,
  }),
  sourceRefs: z.array(RefSchema),
  matchReasons: z.array(z.string()),
  relatedRefs: z.array(RefSchema).optional(),
});
export const QueryResultSchema = z.object({
  items: z.array(QueryItemSchema),
  resolvedQuery: QuerySchema,
  nextCursor: z.string().optional(),
  resultSetId: z.string(),
  indexState: z.object({
    textReady: z.boolean(), vectorReady: z.boolean(),
    embeddingSpace: z.string().optional(), tokenizer: z.literal('icu'),
    selectedRevisions: z.number().int().nonnegative(),
    historical: z.boolean(),
  }),
});
export type QueryResult = z.infer<typeof QueryResultSchema>;
export const ExpandSchema = z.object({
  refs: z.array(RefSchema), include: z.array(z.enum(['sources', 'relations'])).default(['sources']),
  depth: z.number().int().min(1).max(4).default(1), limit: z.number().int().positive().max(60).default(60),
});
export const ExpansionResultSchema = z.object({
  items: z.array(QueryItemSchema), sources: z.array(MaterialViewSchema),
  relations: z.array(z.object({ from: RefSchema, to: RefSchema, relation: z.string(), weight: z.number() })),
});

export const WorkContextBodySchema = z.object({
  goal: z.string(), openQuestions: z.array(z.string()).default([]),
  progress: z.string().default(''), currentRefs: z.array(RefSchema).default([]),
  temporaryNotes: z.array(z.string()).default([]), resumeCue: z.string().default(''),
  participantRefs: z.array(RefSchema).default([]),
  lifecycle: z.enum(['active', 'paused', 'completed']).default('active'),
  relatedRefs: z.array(RefSchema).default([]),
});
export const WorkContextSchema = WorkContextBodySchema.extend({
  scope:ConsumerSchema,
  id: z.string(), subjectId: z.string(), revision: z.number().int().positive(),
  ref: RefSchema, recorded: InstantSchema,
});
export type WorkContext = z.infer<typeof WorkContextSchema>;
export const InputPortionSchema = z.object({
  ref: RefSchema, text: z.string(),
  selector: SelectorSchema.optional(),
  required: z.boolean().default(false),
});
export const ContextPrepareSchema = z.object({
  consumer: ConsumerSchema,
  role: z.string().default('deliberation'),
  workContextId: z.string().optional(),
  roleTask: z.string().default(''),
  situation: z.string().default(''),
  query: z.string().optional(),
  latestMaterials: z.array(InputPortionSchema).default([]),
  requiredSources:z.array(RefSchema).default([]),
  model: z.object({ resourceId: z.string(), modelId: z.string(), contextLimit: z.number().int().positive() }),
  budget: z.object({
    outputTokens: z.number().int().nonnegative(),
    toolTokens: z.number().int().nonnegative().default(0),
    requiredTokens: z.number().int().nonnegative().default(0),
    mediaTokens: z.number().int().nonnegative().default(0),
    cognitionTokens: z.number().int().positive().optional(),
  }),
  observed: TimePointInputSchema,
});
export type ContextPrepareInput = z.input<typeof ContextPrepareSchema>;
export const ContextBlockSchema = z.object({
  kind: z.enum(['task', 'identity', 'situation', 'knowledge', 'sources', 'work']),
  text: z.string(), refs: z.array(RefSchema), tokens: z.number().int().nonnegative(),
});
export const ContextViewSchema = z.object({
  id: z.string(), ref: RefSchema, subjectId: z.string(),
  consumer: ConsumerSchema, workContextId: z.string().optional(),
  role: z.string(),
  blocks: z.array(ContextBlockSchema), selectedRefs: z.array(RefSchema),
  inputPortions: z.array(InputPortionSchema),
  trimmed: z.array(z.object({ ref: RefSchema.optional(), reason: z.string(), originalCodePoints: z.number(), includedCodePoints: z.number() })),
  model: ContextPrepareSchema.shape.model,
  budget: z.object({ availableTokens: z.number(), usedTokens: z.number(), reservedTokens: z.number(), estimate: z.literal('unicode_code_points') }),
  effectivePolicy: z.object({ purpose: z.string(), audience: z.array(z.string()) }),
  settingsSnapshots:z.record(z.string(),ConfigSnapshotSchema),
  observed: TimePointSchema,
  recorded: InstantSchema,
});
export type ContextView = z.infer<typeof ContextViewSchema>;
export const FeedbackSchema = z.object({
  stepKey: z.string(), kind: z.enum(['presented', 'cited', 'adopted', 'outcome', 'corrected']),
  refs: z.array(RefSchema), basisRefs: z.array(RefSchema).default([]),
});
export const KnowledgeChangeSchema = KnowledgeCreateSchema.extend({
  target: z.object({ id: z.string(), expectedRevision: z.number().int().positive() }).optional(),
  reason: z.string().min(1),
});
export const ChangeSetSchema = z.object({
  groups: z.array(z.object({
    id: z.string(), reason: z.string(), changes: z.array(KnowledgeChangeSchema),
  })),
});
export type ChangeSet = z.infer<typeof ChangeSetSchema>;
export const FormationArtifactSchema = z.object({
  id: z.string(), ref: RefSchema, subjectId: z.string(),
  purpose: z.enum(['growth', 'seed']).default('growth'), previousArtifactRef: RefSchema.optional(), appliedArtifactRef: RefSchema.optional(),
  experienceRefs: z.array(RefSchema), sourceRefs: z.array(RefSchema),
  changeSet: ChangeSetSchema,
  groups: z.array(z.object({
    id: z.string(), status: z.enum(['pending', 'applied', 'conflict', 'invalid', 'discarded']),
    issues: z.array(z.string()).default([]),
    createdRefs: z.array(RefSchema), revisedRefs: z.array(RefSchema),
    conflicts: z.array(z.object({ id: z.string(), expectedRevision: z.number(), currentRevision: z.number().optional() })),
  })),
  modelOperationId:z.string().optional(),
  resolutions:z.array(z.object({groupIds:z.array(z.string()),previous:ChangeSetSchema,action:z.enum(['edit','rejudge','discard']),reason:z.string(),at:TimePointSchema,modelOperationId:z.string().optional()})).default([]),
  modelOutputRef: RefSchema.optional(), recorded: InstantSchema,
});
export type FormationArtifact = z.infer<typeof FormationArtifactSchema>;
export const GrowthResolveSchema=z.object({id:z.string(),groupIds:z.array(z.string()).min(1),action:z.enum(['edit','rejudge','discard']),groups:ChangeSetSchema.shape.groups.optional(),formed:TimePointInputSchema,reason:z.string().min(1)});
export const SeedApplySchema = z.object({
  id: z.string(),formed:TimePointInputSchema, selectedIndexes: z.array(z.number().int().nonnegative()).optional(),
  changes: z.array(KnowledgeChangeSchema).optional(),
});

export const ResourceBodySchema = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('directory'), title: z.string(), root: z.string(), extensions: z.array(z.string()).default([]), maxBytes: z.number().int().positive().default(10_485_760), policy: PolicySchema }),
  z.object({ kind: z.literal('http'), title: z.string(), baseUrl: z.string().url(), allowedOrigins: z.array(z.string().url()).default([]), maxBytes: z.number().int().positive().default(10_485_760), policy: PolicySchema }),
  z.object({ kind: z.literal('mcp'), title: z.string(), connectorId: z.string(), allowedUris: z.array(z.string()).default([]), maxBytes: z.number().int().positive().default(10_485_760), policy: PolicySchema }),
]);
export type ResourceBody = z.infer<typeof ResourceBodySchema>;

// Named public schemas are shared by OpenAPI references.
for (const [id, schema] of Object.entries({QuerySchema,QueryInputSchema,QueryItemSchema,QueryResultSchema,ExpandSchema,ExpansionResultSchema,WorkContextBodySchema,WorkContextSchema,InputPortionSchema,ContextPrepareSchema,ContextBlockSchema,ContextViewSchema,FeedbackSchema,KnowledgeChangeSchema,ChangeSetSchema,FormationArtifactSchema,GrowthResolveSchema,SeedApplySchema,ResourceBodySchema})) z.globalRegistry.add(schema, { id: id === 'QueryInputSchema' ? 'QueryRequest' : id.replace(/Schema$/, '') });
