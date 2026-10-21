import { z } from 'zod';
import {
  RefSchema,
  TimePointSchema,
  TimeRangeSchema,
  InstantSchema,
  InstantInputSchema,
  SubjectIdSchema,
  ActorSchema,
  ConsumerSchema,
  canonicalRefKey,
} from './common.js';

export const ClockRefSchema = RefSchema.omit({ selector: true })
  .extend({
    kind: z.literal('Clock'),
    revision: z.number().int().positive().safe().optional(),
  })
  .strict();
export const ClockSegmentRefSchema = RefSchema.omit({
  selector: true,
  revision: true,
})
  .extend({ kind: z.literal('ClockSegment') })
  .strict();
export const TimeCaptureRefSchema = RefSchema.omit({
  selector: true,
  revision: true,
})
  .extend({ kind: z.literal('TimeCapture') })
  .strict();
export const TimeSelectionSchema = z.strictObject({
  subjectId: SubjectIdSchema.optional(),
  domainId: z.string().min(1).optional(),
  preferClock: ClockRefSchema.optional(),
});
export const TimeCaptureRequestSchema = TimeSelectionSchema.extend({
  existingOnly: z.boolean().optional(),
});
z.globalRegistry.add(TimeCaptureRequestSchema, { id: 'TimeCaptureBody' });
const realPoint = TimePointSchema.extend({ domainId: z.literal('real') });
const mapped = z.strictObject({
  point: TimePointSchema,
  realAt: InstantSchema,
  clockRef: ClockRefSchema.extend({
    revision: z.number().int().positive().safe(),
  }),
  clockRevision: z.number().int().positive().safe(),
  segmentRef: ClockSegmentRefSchema,
});
const real = z.strictObject({ point: realPoint, realAt: InstantSchema });
function consistent(value: z.infer<typeof mapped>, ctx: z.RefinementCtx) {
  if (
    value.point.domainId === 'real' ||
    value.clockRef.revision !== value.clockRevision ||
    value.clockRef.product !== value.segmentRef.product ||
    value.clockRef.installationId !== value.segmentRef.installationId
  )
    ctx.addIssue({
      code: 'custom',
      message: '映射时间的时域、修订与来源必须一致',
    });
}
export const CapturedTimeSchema = z.xor([real, mapped.superRefine(consistent)]);
export const TimeCaptureReceiptSchema = z.xor([
  real.extend({ captureRef: TimeCaptureRefSchema }),
  mapped
    .extend({ captureRef: TimeCaptureRefSchema })
    .superRefine((value, ctx) => {
      consistent(value, ctx);
      if (
        value.captureRef.product !== value.clockRef.product ||
        value.captureRef.installationId !== value.clockRef.installationId
      )
        ctx.addIssue({
          code: 'custom',
          message: '时间回执必须来自实际时钟所有者',
        });
    }),
]);
export type TimeSelection = z.infer<typeof TimeSelectionSchema>;
export type CapturedTime = z.infer<typeof CapturedTimeSchema>;
export type TimeCaptureReceipt = z.infer<typeof TimeCaptureReceiptSchema>;
export const TemporalExtentSchema = z.discriminatedUnion('kind', [
  z.strictObject({ kind: z.literal('unknown') }),
  z.strictObject({ kind: z.literal('instant'), point: TimePointSchema }),
  z.strictObject({ kind: z.literal('interval'), range: TimeRangeSchema }),
]);
for (const [id, schema] of Object.entries({
  ClockRefSchema,
  ClockSegmentRefSchema,
  TimeCaptureRefSchema,
  TimeSelectionSchema,
  CapturedTimeSchema,
  TimeCaptureReceiptSchema,
  TemporalExtentSchema,
}))
  z.globalRegistry.add(schema, { id: id.replace(/Schema$/, '') });

export const ClockRateSchema = z
  .string()
  .regex(/^(?:0|[1-9]\d{0,17})(?:\.\d{0,11}[1-9])?$/);
export const ClockOfflinePolicySchema = z.discriminatedUnion('mode', [
  z.strictObject({ mode: z.literal('freeze') }),
  z.strictObject({ mode: z.literal('elapsed'), offlineRate: ClockRateSchema }),
]);
const revision = z.number().int().positive().safe();
export const TimeDomainSchema = z.strictObject({
  domainId: z.string().min(1).max(256),
  label: z.string().min(1),
  coordinateSystem: z.literal('iso8601'),
  precision: z.literal('microsecond'),
  authorityRef: RefSchema,
  status: z.enum(['active', 'retired']),
  subjectId: SubjectIdSchema.optional(),
  createdRealAt: InstantSchema,
});
export const TimeDomainCreateSchema = z.strictObject({
  domainId: z.string().min(1).max(256),
  label: z.string().min(1).max(1024),
  subjectId: SubjectIdSchema.optional(),
});
export const ClockSegmentSchema = z.strictObject({
  ref: ClockSegmentRefSchema,
  clockRef: ClockRefSchema.extend({ revision }),
  domainId: z.string().min(1),
  revision,
  domainAnchor: InstantSchema,
  realAnchor: InstantSchema,
  rate: ClockRateSchema,
  startedByOperationId: z.string().min(1),
  reason: z.enum([
    'configure',
    'change_rate',
    'pause',
    'resume',
    'advance',
    'host_start',
    'host_stop',
  ]),
});
export const ClockRecordSchema = z.strictObject({
  ref: ClockRefSchema.extend({ revision }),
  domainId: z.string().min(1),
  revision,
  currentSegmentRef: ClockSegmentRefSchema,
  onlineRate: ClockRateSchema,
  offlinePolicy: ClockOfflinePolicySchema,
  status: z.enum(['running', 'paused', 'retired']),
});
export const ClockSnapshotSchema = z.union([
  z.strictObject({
    domainId: z.literal('real'),
    rate: z.literal('1'),
    now: real,
  }),
  z
    .strictObject({
      domainId: z.string().min(1),
      rate: ClockRateSchema,
      clockRef: ClockRefSchema.extend({ revision }),
      revision,
      segment: ClockSegmentSchema,
      now: mapped,
    })
    .superRefine((value, ctx) => {
      consistent(value.now, ctx);
      if (
        value.domainId !== value.now.point.domainId ||
        value.domainId !== value.segment.domainId ||
        value.revision !== value.clockRef.revision ||
        value.revision !== value.segment.revision ||
        value.revision !== value.now.clockRevision ||
        value.rate !== value.segment.rate ||
        value.clockRef.id !== value.segment.clockRef.id ||
        value.clockRef.id !== value.now.clockRef.id ||
        value.segment.ref.id !== value.now.segmentRef.id
      )
        ctx.addIssue({
          code: 'custom',
          message: '时钟快照必须采用同一实际映射与修订',
        });
    }),
]);
export const ClockConfigureSchema = z.union([
  z.strictObject({
    subjectId: SubjectIdSchema.optional(),
    domainId: z.string().min(1),
    expectedRevision: z.literal(0),
    initialAt: InstantInputSchema,
    rate: ClockRateSchema,
    offlinePolicy: ClockOfflinePolicySchema,
  }),
  z
    .strictObject({
      subjectId: SubjectIdSchema.optional(),
      domainId: z.string().min(1),
      clockRef: ClockRefSchema.optional(),
      expectedRevision: revision,
      rate: ClockRateSchema.optional(),
      offlinePolicy: ClockOfflinePolicySchema.optional(),
    })
    .refine(
      (value) => value.rate !== undefined || value.offlinePolicy !== undefined,
      '至少修改倍率或离线政策',
    ),
]);
export const ClockPauseSchema = z.strictObject({
  clockRef: ClockRefSchema,
  expectedRevision: revision,
});
export const ClockResumeSchema = ClockPauseSchema.extend({
  rate: ClockRateSchema.refine((value) => value !== '0', '恢复倍率必须为正'),
});
export const ClockAdvanceSchema = ClockPauseSchema.extend({
  deltaMicros: z.string().regex(/^(?:0|[1-9]\d{0,20})$/),
});
export const ClockDeadlineSchema = z.discriminatedUnion('kind', [
  z.strictObject({ kind: z.literal('due'), at: InstantSchema }),
  z.strictObject({ kind: z.literal('paused') }),
  z.strictObject({ kind: z.literal('unmapped') }),
]);
export const ClockSegmentClosureSchema = z.strictObject({
  segmentRef: ClockSegmentRefSchema,
  closedByClockRevision: revision,
  domainEnd: InstantSchema,
  endedRealAt: InstantSchema,
  endedByOperationId: z.string(),
});
export const ClockChangeReceiptSchema = z.strictObject({
  operationId: z.string(),
  record: ClockRecordSchema,
  snapshot: ClockSnapshotSchema,
  closure: ClockSegmentClosureSchema.optional(),
  lifecycle: z
    .strictObject({
      status: z.enum(['offline', 'resumed', 'superseded', 'recovered']),
      resumeOperationId: z.string().optional(),
      gap: z.string().nullable(),
    })
    .optional(),
});
export type ClockRef = z.infer<typeof ClockRefSchema>;
export type ClockSegmentRef = z.infer<typeof ClockSegmentRefSchema>;
export type ClockRate = z.infer<typeof ClockRateSchema>;
export type ClockOfflinePolicy = z.infer<typeof ClockOfflinePolicySchema>;
export type TimeDomain = z.infer<typeof TimeDomainSchema>;
export type ClockSegment = z.infer<typeof ClockSegmentSchema>;
export type ClockRecord = z.infer<typeof ClockRecordSchema>;
export type ClockSnapshot = z.infer<typeof ClockSnapshotSchema>;
export type ClockConfigure = z.infer<typeof ClockConfigureSchema>;
export type ClockChangeReceipt = z.infer<typeof ClockChangeReceiptSchema>;
export type ClockDeadline = z.infer<typeof ClockDeadlineSchema>;
for (const [id, schema] of Object.entries({
  ClockRate: ClockRateSchema,
  ClockOfflinePolicy: ClockOfflinePolicySchema,
  TimeDomain: TimeDomainSchema,
  TimeDomainCreateRequest: TimeDomainCreateSchema,
  ClockSegment: ClockSegmentSchema,
  ClockRecord: ClockRecordSchema,
  ClockSnapshot: ClockSnapshotSchema,
  ClockConfigureRequest: ClockConfigureSchema,
  ClockPauseRequest: ClockPauseSchema,
  ClockResumeRequest: ClockResumeSchema,
  ClockAdvanceRequest: ClockAdvanceSchema,
  ClockDeadline: ClockDeadlineSchema,
  ClockSegmentClosure: ClockSegmentClosureSchema,
  ClockChangeReceipt: ClockChangeReceiptSchema,
}))
  z.globalRegistry.add(schema, { id });

/** Private capability injected only into owners recording an actual physical completion. */
export interface TimeRecordings {
  captureTerminal(
    ctx: import('./common.js').CallContext,
    input: {
      operationId: string;
      originRef: import('./common.js').Ref;
      consumer: import('./common.js').Consumer;
      observed: TimeCaptureReceipt;
      realAt?: string;
    },
  ): Promise<TimeCaptureReceipt>;
}

/** Versioned archive row owned by Nous Time; Clock state remains in Hept. */
export const TimeCaptureDataRecordSchema = z
  .strictObject({
    id: z.uuidv7(),
    operation_id: z.string().min(1),
    request_digest: z.string().regex(/^[a-f0-9]{64}$/),
    capture_key: z.string().min(1),
    connection_ref: RefSchema.nullable(),
    subject_id: SubjectIdSchema.nullable(),
    consumer: ConsumerSchema,
    actor: ActorSchema,
    origin_ref: RefSchema.nullable(),
    receipt: TimeCaptureReceiptSchema,
    recorded_at: InstantInputSchema,
  })
  .superRefine((row, ctx) => {
    if (row.capture_key !== canonicalRefKey(row.receipt.captureRef))
      ctx.addIssue({
        code: 'custom',
        message: '时间回执的固定身份与存储键不一致',
      });
    if (row.origin_ref && (row.origin_ref.subjectId ?? null) !== row.subject_id)
      ctx.addIssue({ code: 'custom', message: '时间记录的来源与主体不一致' });
    if (
      row.receipt.captureRef.subjectId !== undefined &&
      row.receipt.captureRef.subjectId !== row.subject_id
    )
      ctx.addIssue({ code: 'custom', message: '时间回执不属于该主体' });
  });
export type TimeCaptureDataRecord = z.infer<typeof TimeCaptureDataRecordSchema>;
