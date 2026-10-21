import { z } from 'zod';
import { ProblemSchema, SubjectIdSchema } from './common.js';

export const ComponentStatusSchema = z.object({
  state: z.enum(['available', 'degraded', 'unavailable']),
  code: z.string().optional(),
  problem: ProblemSchema.optional(),
});
export const HostStatusSchema = z.object({
  product: z.enum(['nous', 'hept']),
  installationId: z.string().min(1),
  version: z.string(),
  state: z.enum([
    'created',
    'starting',
    'running',
    'draining',
    'stopped',
    'failed',
  ]),
  components: z.record(z.string(), ComponentStatusSchema),
  subjectsAutoStart: z.boolean(),
});
export type HostStatus = z.infer<typeof HostStatusSchema>;
z.globalRegistry.add(HostStatusSchema, { id: 'HostStatus' });

export const OperationViewSchema = z
  .strictObject({
    operationId: z.string().min(1),
    status: z.enum([
      'queued',
      'running',
      'waiting',
      'completed',
      'failed',
      'cancelled',
    ]),
    nativeStatus: z.string(),
    createdAtEpochMs: z.number().int(),
    updatedAtEpochMs: z.number().int().optional(),
    completedAtEpochMs: z.number().int().optional(),
    traceId: z.string().optional(),
    operation: z.string(),
    subjectId: z.string().optional(),
    parentOperationId: z.string().optional(),
    queueName: z.string().optional(),
    applicationVersion: z.string().optional(),
    recoveryAttempts: z.number().int().nonnegative().optional(),
    problem: ProblemSchema.optional(),
  })
  .meta({ id: 'OperationView' });
export type OperationView = z.infer<typeof OperationViewSchema>;
export const OperationControlSchema = z
  .strictObject({
    subjectId: SubjectIdSchema.optional(),
  })
  .meta({ id: 'OperationControl' });
export const OperationStepSchema = z.strictObject({
  functionID: z.number().int().nonnegative(),
  name: z.string(),
  status: z.enum(['completed', 'failed']),
  childWorkflowID: z.string().optional(),
  startedAtEpochMs: z.number().int().optional(),
  completedAtEpochMs: z.number().int().optional(),
  problem: ProblemSchema.optional(),
});
export const OperationTraceSchema = z
  .strictObject({
    operation: OperationViewSchema,
    steps: z.array(OperationStepSchema),
  })
  .meta({ id: 'OperationTrace' });
export type OperationTrace = z.infer<typeof OperationTraceSchema>;
