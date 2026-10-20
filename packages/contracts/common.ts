import { z } from 'zod';

export const SubjectIdSchema = z.uuidv7();
export const ObjectIdSchema = z.uuidv7();
export type SubjectId = string;
export type ObjectId = string;
export const TimePointSchema = z.object({ clockId: z.string().min(1), at: z.iso.datetime({ offset: true }).overwrite(at => new Date(at).toISOString()) });
export type TimePoint = z.infer<typeof TimePointSchema>;
export const TimeRangeSchema = z.object({ start: TimePointSchema.optional(), end: TimePointSchema.optional() });
export type TimeRange = z.infer<typeof TimeRangeSchema>;
export const SelectorSchema: z.ZodType<Selector> = z.lazy(() => z.object({
  kind: z.enum(['text', 'page', 'image', 'media', 'part', 'node']),
  start: z.number().nonnegative().optional(), end: z.number().nonnegative().optional(),
  page: z.number().int().positive().optional(), partId: z.string().optional(), nodeId: z.string().optional(),
  x: z.number().min(0).max(1).optional(), y: z.number().min(0).max(1).optional(),
  width: z.number().min(0).max(1).optional(), height: z.number().min(0).max(1).optional(),
  within: SelectorSchema.optional(),
}));
export interface Selector {
  kind: 'text' | 'page' | 'image' | 'media' | 'part' | 'node'; start?: number; end?: number;
  page?: number; partId?: string; nodeId?: string; x?: number; y?: number; width?: number; height?: number; within?: Selector;
}
export const RefSchema = z.object({
  product: z.enum(['nous', 'hept']), installationId: z.string().min(1), subjectId: z.string().optional(),
  kind: z.string().min(1), id: z.string().min(1), revision: z.number().int().positive().optional(), selector: SelectorSchema.optional(),
});
export type Ref = z.infer<typeof RefSchema>;
export const ConsumerSchema = z.object({ kind: z.enum(['subject', 'worker', 'author', 'maintenance']), host: z.string(), id: z.string() });
export type Consumer = z.infer<typeof ConsumerSchema>;
function selectorIdentity(selector?: Selector): unknown {
  return selector ? [selector.kind, selector.start ?? null, selector.end ?? null, selector.page ?? null,
    selector.partId ?? null, selector.nodeId ?? null, selector.x ?? null, selector.y ?? null,
    selector.width ?? null, selector.height ?? null, selectorIdentity(selector.within)] : null;
}
export function canonicalRefKey(ref: Ref): string {
  return JSON.stringify([ref.product, ref.installationId, ref.subjectId ?? null, ref.kind, ref.id, ref.revision ?? null, selectorIdentity(ref.selector)]);
}
export function sameRef(left: Ref, right: Ref): boolean { return canonicalRefKey(left) === canonicalRefKey(right); }
export const ActorSchema = z.object({
  kind: z.enum(['human', 'subject', 'service', 'worker', 'author']), id: z.string().min(1),
  grants: z.array(z.string()), subjectIds: z.array(z.string()).optional(), expiresAt: z.iso.datetime().optional(),
  consumer: ConsumerSchema.optional(),
});
export type Actor = z.infer<typeof ActorSchema>;
export const CallContextSchema = z.object({
  actor: ActorSchema, subjectId: z.string(), purpose: z.string(), audience: z.array(z.string()), consumer: ConsumerSchema.optional(),
  traceContext: z.object({ traceId: z.string().regex(/^[a-f0-9]{32}$/), operationId: z.string().min(1), traceparent: z.string().optional() }),
});
export type CallContext = z.infer<typeof CallContextSchema>;
export const PolicySchema = z.object({ readers: z.array(z.string()), purposes: z.array(z.string()), audiences: z.array(z.string()) });
export type Policy = z.infer<typeof PolicySchema>;
export const ProblemSchema = z.object({
  type: z.string(), title: z.string(), status: z.number().int(), detail: z.string().optional(),
  code: z.enum(['invalid_input','forbidden','not_found','conflict','capability_unavailable','budget_exhausted','dependency_unavailable','effect_unknown']),
  operationId: z.string().optional(), currentRevision: z.number().optional(), retryAfter: z.number().optional(),
});
export type Problem = z.infer<typeof ProblemSchema>;
export const OperationRefSchema = z.object({ operationId: z.string(), statusUrl: z.string(), resourceRef: RefSchema.optional() });
export type OperationRef = z.infer<typeof OperationRefSchema>;

// Named public schemas are shared by OpenAPI references.
for (const [id, schema] of Object.entries({SubjectIdSchema,ObjectIdSchema,TimePointSchema,TimeRangeSchema,SelectorSchema,RefSchema,ActorSchema,CallContextSchema,PolicySchema,ProblemSchema,OperationRefSchema,ConsumerSchema})) z.globalRegistry.add(schema, { id: id.replace(/Schema$/, '') });
