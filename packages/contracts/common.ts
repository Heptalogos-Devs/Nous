import { Temporal } from '@js-temporal/polyfill';
import { z } from 'zod';

export const SubjectIdSchema = z.uuidv7();
export const ObjectIdSchema = z.uuidv7();
export type SubjectId = string;
export type ObjectId = string;
function validateInstant(at: string, ctx: z.RefinementCtx) {
  try {
    const instant = Temporal.Instant.from(at),
      year = instant.toZonedDateTimeISO('UTC').year;
    if (year < 1 || year > 9999 || instant.epochNanoseconds % 1000n !== 0n)
      ctx.addIssue({
        code: 'custom',
        message: '时间必须位于0001..9999年且精确到整微秒',
      });
  } catch {
    ctx.addIssue({
      code: 'custom',
      message: '时间必须是带明确时区的合法ISO时间',
    });
  }
}
const InstantInput = z.iso
  .datetime({ offset: true })
  .superRefine(validateInstant);
export const InstantSchema = z
  .string()
  .regex(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{6}Z$/)
  .superRefine(validateInstant);
export const InstantInputSchema = InstantInput.pipe(
  z
    .string()
    .overwrite((at) =>
      Temporal.Instant.from(at).toString({ fractionalSecondDigits: 6 }),
    )
    .pipe(InstantSchema),
);
export const TimePointSchema = z.object({
  domainId: z.string().min(1),
  at: InstantSchema,
});
export const TimePointInputSchema = z.object({
  domainId: z.string().min(1),
  at: InstantInputSchema,
});
export type TimePoint = z.infer<typeof TimePointSchema>;
function validateRange(
  range: { start?: TimePoint; end?: TimePoint },
  ctx: z.RefinementCtx,
) {
  if (
    range.start &&
    range.end &&
    (range.start.domainId !== range.end.domainId ||
      range.start.at >= range.end.at)
  )
    ctx.addIssue({
      code: 'custom',
      message: '半开时间范围要求同域且start早于end',
    });
}
export const TimeRangeSchema = z
  .object({
    start: TimePointSchema.optional(),
    end: TimePointSchema.optional(),
  })
  .superRefine(validateRange);
export const TimeRangeInputSchema = z
  .object({
    start: TimePointInputSchema.optional(),
    end: TimePointInputSchema.optional(),
  })
  .superRefine(validateRange);
export type TimeRange = z.infer<typeof TimeRangeSchema>;
export type Selector =
  | { kind: 'node'; pointer: string; within?: Selector }
  | {
      kind: 'text' | 'page' | 'image' | 'media' | 'part';
      start?: number;
      end?: number;
      page?: number;
      partId?: string;
      x?: number;
      y?: number;
      width?: number;
      height?: number;
      within?: Selector;
    };
const pointer = z
  .string()
  .refine(
    (value) => value === '' || /^\/(?:[^~]|~[01])*$/.test(value),
    '节点区域必须是RFC 6901 JSON Pointer',
  );
function selectorSchema(query = false): z.ZodType<Selector> {
  const number = () => (query ? z.coerce.number() : z.number());
  const schema: z.ZodType<Selector> = z.lazy(() =>
    z.discriminatedUnion('kind', [
      z.strictObject({
        kind: z.literal('node'),
        pointer,
        within: schema.optional(),
      }),
      z.strictObject({
        kind: z.enum(['text', 'page', 'image', 'media', 'part']),
        start: number().nonnegative().optional(),
        end: number().nonnegative().optional(),
        page: number().int().positive().optional(),
        partId: z.string().optional(),
        x: number().min(0).max(1).optional(),
        y: number().min(0).max(1).optional(),
        width: number().min(0).max(1).optional(),
        height: number().min(0).max(1).optional(),
        within: schema.optional(),
      }),
    ]),
  );
  return schema;
}
export const SelectorSchema = selectorSchema();
/** HTTP query scalars are strings; the resulting value is the same formal Selector. */
export const SelectorQuerySchema = selectorSchema(true);
z.globalRegistry.add(SelectorQuerySchema, { id: 'SelectorQuery' });
export const RefSchema = z.object({
  product: z.enum(['nous', 'hept']),
  installationId: z.string().min(1),
  subjectId: z.string().optional(),
  kind: z.string().min(1),
  id: z.string().min(1),
  revision: z.number().int().positive().optional(),
  selector: SelectorSchema.optional(),
});
export type Ref = z.infer<typeof RefSchema>;
/** Identity supplied by the external owner, without inventing a product object. */
export const OriginIdentitySchema = z.discriminatedUnion('type', [
  z.strictObject({
    type: z.literal('platform'),
    adapterId: z.string().min(1),
    accountId: z.string().min(1),
    conversationId: z.string().min(1),
    messageId: z.string().min(1),
    nodePath: z.array(z.string()).optional(),
  }),
  z.strictObject({
    type: z.literal('resource'),
    connectorId: z.string().min(1),
    documentId: z.string().min(1),
    version: z.string().min(1).optional(),
  }),
  z.strictObject({
    type: z.literal('native'),
    runtimeId: z.string().min(1),
    sessionId: z.string().min(1),
    sessionKey: z.string().min(1).optional(),
    runId: z.string().min(1).optional(),
    entryId: z.string().min(1).optional(),
    callId: z.string().min(1).optional(),
  }),
]).meta({ id: 'OriginIdentity' });
export type OriginIdentity = z.infer<typeof OriginIdentitySchema>;
export const ConsumerSchema = z.strictObject({
  kind: z.enum(['subject', 'worker', 'author', 'maintenance']),
  host: z.string().min(1),
  id: z.string().min(1),
});
export type Consumer = z.infer<typeof ConsumerSchema>;
function selectorIdentity(selector?: Selector): unknown {
  if (!selector) return null;
  if (selector.kind === 'node')
    return ['node', selector.pointer, selectorIdentity(selector.within)];
  return [
    selector.kind,
    selector.start ?? null,
    selector.end ?? null,
    selector.page ?? null,
    selector.partId ?? null,
    selector.x ?? null,
    selector.y ?? null,
    selector.width ?? null,
    selector.height ?? null,
    selectorIdentity(selector.within),
  ];
}
export function canonicalRefKey(ref: Ref): string {
  return JSON.stringify([
    ref.product,
    ref.installationId,
    ref.subjectId ?? null,
    ref.kind,
    ref.id,
    ref.revision ?? null,
    selectorIdentity(ref.selector),
  ]);
}
export function sameRef(left: Ref, right: Ref): boolean {
  return canonicalRefKey(left) === canonicalRefKey(right);
}
export const ActorSchema = z.strictObject({
  id: z.string().min(1),
  kind: z.enum(['human', 'service', 'subject', 'worker', 'author']),
  authorityRef: z.string().min(1),
});
export type Actor = z.infer<typeof ActorSchema>;
export const TraceContextSchema = z.strictObject({
  traceId: z.string().regex(/^[a-f0-9]{32}$/),
  operationId: z.string().min(1),
  traceparent: z.string().optional(),
  invocationId: z.string().min(1).optional(),
  taskId: z.string().min(1).optional(),
  effectId: z.string().min(1).optional(),
});
export type TraceContext = z.infer<typeof TraceContextSchema>;
export const CallContextSchema = z.strictObject({
  actor: ActorSchema,
  consumer: ConsumerSchema,
  subjectId: SubjectIdSchema.optional(),
  purpose: z.string(),
  audience: z.array(z.string()),
  trace: TraceContextSchema,
});
export type CallContext = z.infer<typeof CallContextSchema>;
export const PolicySchema = z.object({
  readers: z.array(z.string()),
  purposes: z.array(z.string()),
  audiences: z.array(z.string()),
});
export type Policy = z.infer<typeof PolicySchema>;
export const JsonValueSchema = z.json();
z.globalRegistry.add(JsonValueSchema, { id: 'JsonValue' });
export const MoneySchema = z.strictObject({
  amount: z.string().regex(/^-?(?:0|[1-9]\d*)(?:\.\d+)?$/),
  currency: z.string().regex(/^[A-Z]{3}$/),
});
export type Money = z.infer<typeof MoneySchema>;
export const ProblemSchema = z.object({
  code: z.string().min(1),
  category: z.enum([
    'validation',
    'forbidden',
    'not_found',
    'conflict',
    'unavailable',
    'resource_pressure',
    'budget',
    'effect_unknown',
    'failed',
  ]),
  status: z.number().int().min(400).max(599),
  messageKey: z.string().min(1),
  args: z.record(z.string(), z.union([z.string(), z.number()])).optional(),
  message: z.string(),
  operationId: z.string().optional(),
  traceId: z.string().optional(),
  currentRevision: z.number().int().nonnegative().safe().optional(),
  retry: z
    .object({
      mode: z.enum(['after_change', 'after_delay', 'reconcile', 'none']),
      at: z.string().optional(),
    })
    .optional(),
  details: z.record(z.string(), JsonValueSchema).optional(),
});
export type Problem = z.infer<typeof ProblemSchema>;
export class ProblemError extends Error {
  readonly problem: Problem;
  constructor(value: Problem) {
    const parsed = ProblemSchema.parse(value);
    super(parsed.message);
    this.name = 'ProblemError';
    this.problem = parsed;
  }
  get code() {
    return this.problem.code;
  }
  get status() {
    return this.problem.status;
  }
}

/** Read a validated business error, including its durable serialized representation. */
export function readProblem(value: unknown): Problem | undefined {
  if (value instanceof ProblemError) return value.problem;
  if (!value || typeof value !== 'object' || !('name' in value) || value.name !== 'ProblemError' || !('problem' in value)) return;
  const parsed = ProblemSchema.safeParse(value.problem);
  return parsed.success ? parsed.data : undefined;
}
export const OperationRefSchema = z.object({
  operationId: z.string(),
  statusUrl: z.string(),
  resourceRef: RefSchema.optional(),
});
export type OperationRef = z.infer<typeof OperationRefSchema>;

// Named public schemas are shared by OpenAPI references.
for (const [id, schema] of Object.entries({
  SubjectIdSchema,
  ObjectIdSchema,
  TimePointSchema,
  TimePointInputSchema,
  TimeRangeSchema,
  TimeRangeInputSchema,
  InstantSchema,
  InstantInputSchema,
  SelectorSchema,
  RefSchema,
  ActorSchema,
  TraceContextSchema,
  CallContextSchema,
  PolicySchema,
  MoneySchema,
  ProblemSchema,
  OperationRefSchema,
  ConsumerSchema,
}))
  z.globalRegistry.add(schema, {
    id: id.replace(/Schema$/, '').replace(/Input$/, 'Request'),
  });
