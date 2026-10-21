import { DBOS } from '@dbos-inc/dbos-sdk';
import {
  OperationStepSchema,
  OperationViewSchema,
} from '@nous-wave/contracts/runtime';
import { problem, safeProblem } from '../errors.js';

/** DBOS 5.3 recognizes libpq startup options and skips its concurrent pool connect SET query. */
export function dbosSystemDatabaseURL(databaseURL: string): string {
  const url = new URL(databaseURL);
  const options = url.searchParams.get('options') ?? '';
  if (!options.includes('idle_in_transaction_session_timeout')) {
    url.searchParams.set(
      'options',
      `${options} -c idle_in_transaction_session_timeout=60000`.trim(),
    );
  }
  return url.toString();
}
/** The product checks current operation authority before explicitly resuming. */
export async function resumeDurableWorkflow(workflowId: string) {
  const state = await DBOS.getWorkflowStatus(workflowId);
  if (!state)
    throw problem('operations.not_found', 'not_found', '持久操作不存在');
  if (state.status === 'ERROR') {
    const steps = await DBOS.listWorkflowSteps(workflowId);
    const failed = steps?.find((step) => step.error);
    return DBOS.rewindWorkflow(workflowId, {
      startStep: failed?.functionID ?? 0,
    });
  }
  return DBOS.resumeWorkflow(workflowId);
}

/** Select engine metadata. Domain inputs and results are read through their owners. */
export function describeDurableWorkflow(
  state: NonNullable<Awaited<ReturnType<typeof DBOS.getWorkflowStatus>>>,
) {
  const statuses: Record<
    string,
    'queued' | 'running' | 'waiting' | 'completed' | 'failed' | 'cancelled'
  > = {
    ENQUEUED: 'queued',
    DELAYED: 'waiting',
    PENDING: 'running',
    SUCCESS: 'completed',
    ERROR: 'failed',
    CANCELLED: 'cancelled',
    MAX_RECOVERY_ATTEMPTS_EXCEEDED: 'failed',
  };
  return OperationViewSchema.parse({
    operationId: state.workflowID,
    status: statuses[state.status] ?? 'waiting',
    nativeStatus: state.status,
    createdAtEpochMs: state.createdAt,
    updatedAtEpochMs: state.updatedAt,
    completedAtEpochMs: state.completedAt,
    traceId:
      typeof state.attributes?.traceId === 'string'
        ? state.attributes.traceId
        : undefined,
    operation:
      typeof state.attributes?.operation === 'string'
        ? state.attributes.operation
        : state.workflowName,
    subjectId:
      typeof state.attributes?.subjectId === 'string'
        ? state.attributes.subjectId
        : undefined,
    parentOperationId: state.parentWorkflowID,
    queueName: state.queueName,
    applicationVersion: state.applicationVersion,
    recoveryAttempts: state.recoveryAttempts,
    problem: state.error ? safeProblem(state.error) : undefined,
  });
}

export function describeDurableSteps(
  steps: Awaited<ReturnType<typeof DBOS.listWorkflowSteps>>,
) {
  return (steps ?? []).map((step) =>
    OperationStepSchema.parse({
      functionID: step.functionID,
      name: step.name,
      status: step.error ? 'failed' : 'completed',
      childWorkflowID: step.childWorkflowID ?? undefined,
      startedAtEpochMs: step.startedAtEpochMs,
      completedAtEpochMs: step.completedAtEpochMs,
      problem: step.error ? safeProblem(step.error) : undefined,
    }),
  );
}
