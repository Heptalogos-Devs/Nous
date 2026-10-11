// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, type MessageInitShape } from "@bufbuild/protobuf";
import { Code, ConnectError } from "@connectrpc/connect";
import {
  domainErrorDetail,
  domainError,
  ErrorCode,
  ErrorOwner,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import {
  ExecutionTelemetrySchema,
  WorkflowPayloadSchema,
  WorkflowSnapshotSchema,
  type ExecutionTelemetry,
  type MaintenanceClaim,
  type WorkflowPayload,
  type WorkflowSnapshot,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/workflow_envelope_pb.js";
import type {
  FindWorkflowRequest,
  WorkflowReservation,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js";
import type { CognitiveRef } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { KernelClient } from "./kernel-client.js";
import { executionOptions, type ExecutionOptions } from "./execution.js";
import { failedExecutionTelemetry } from "./model/execution/routes.js";

export type Dependency = Pick<CognitiveRef, "kind" | "value">;
export type MutationExecutor = <T>(
  operationId: string,
  commit: () => Promise<T>,
) => Promise<T>;
export type ExecutionTelemetryInput = MessageInitShape<
  typeof ExecutionTelemetrySchema
>;
export type DurableIdentity = Pick<
  FindWorkflowRequest,
  "subjectId" | "owner" | "operationKey" | "semanticDigest"
>;
export type OperationSnapshot = MessageInitShape<typeof WorkflowSnapshotSchema>;
export type SnapshotDecision<T = never> =
  { snapshot: OperationSnapshot } | ([T] extends [never] ? never : { skip: T });

export type OpenDurableOperation<T = never> =
  | ([T] extends [never] ? never : { state: "not_started"; result: T })
  | {
      state: "completed";
      outcome: WorkflowPayload;
      executionTelemetry?: ExecutionTelemetry;
    }
  | {
      state: "busy";
      snapshot: WorkflowSnapshot;
      proposal?: WorkflowPayload;
      executionTelemetry?: ExecutionTelemetry;
    }
  | {
      state: "acquired";
      operation: DurableOperation;
      snapshot: WorkflowSnapshot;
      proposal?: WorkflowPayload;
      executionTelemetry?: ExecutionTelemetry;
    };

export type OpenDurableOperationInput<T = never> = {
  snapshotFactory: () => Promise<SnapshotDecision<T>>;
  execution: ExecutionOptions;
  maintenanceClaim?: Omit<MaintenanceClaim, "$typeName">;
};

type WorkflowCheckpoint = {
  proposal?: WorkflowPayload;
  outcome?: WorkflowPayload;
  executionTelemetry?: ExecutionTelemetryInput;
  mutationOperations?: string[];
};

export interface DurableOperation {
  checkpoint(update: WorkflowCheckpoint): Promise<void>;
  mutate<T>(operationId: string, commit: () => Promise<T>): Promise<T>;
  run<T>(work: () => Promise<T>): Promise<T>;
}

export function workflowPayload(
  payload: unknown,
  dependencies: Dependency[] = [],
): WorkflowPayload {
  return create(WorkflowPayloadSchema, {
    payloadJson: JSON.stringify(payload),
    dependencies,
  });
}

/** Opens a frozen operation through the only Core Find/Reserve path. */
export async function openDurableOperation<T = never>(
  kernel: KernelClient,
  identity: DurableIdentity,
  input: OpenDurableOperationInput<T>,
): Promise<OpenDurableOperation<T>> {
  const calls = executionOptions(kernel.execution.opportunity, input.execution);
  const found = await kernel.modelWorkflow.findWorkflow(identity, calls);
  if (found.found && found.outcome) {
    return {
      state: "completed",
      outcome: found.outcome,
      executionTelemetry: found.executionTelemetry,
    };
  }

  let snapshot: OperationSnapshot | WorkflowSnapshot;
  if (found.found) {
    if (!found.snapshot)
      throw new ConnectError(
        "Found workflow has no frozen snapshot",
        Code.DataLoss,
      );
    snapshot = found.snapshot;
  } else {
    const decision = await input.snapshotFactory();
    if ("skip" in decision)
      return {
        state: "not_started",
        result: decision.skip,
      } as OpenDurableOperation<T>;
    snapshot = decision.snapshot;
  }

  if (input.maintenanceClaim) {
    snapshot = {
      ...snapshot,
      maintenanceClaim: {
        $typeName: "nous.wave.kernel.v1alpha1.MaintenanceClaim",
        ...input.maintenanceClaim,
      },
    };
  }

  const reserved = await kernel.modelWorkflow.reserveWorkflow(
    {
      ...identity,
      snapshot,
      leaseSeconds: calls.opportunity.leaseSeconds,
    },
    calls,
  );
  if (!reserved.snapshot)
    throw new ConnectError(
      "Workflow reservation has no persisted snapshot",
      Code.DataLoss,
    );
  if (reserved.outcome) {
    if (reserved.lease || reserved.busy)
      throw new ConnectError(
        "Terminal workflow reservation has an active attempt",
        Code.DataLoss,
      );
    return {
      state: "completed",
      outcome: reserved.outcome,
      executionTelemetry: reserved.executionTelemetry,
    };
  }
  if (reserved.busy) {
    if (reserved.lease)
      throw new ConnectError(
        "Busy workflow reservation unexpectedly has a lease",
        Code.DataLoss,
      );
    return {
      state: "busy",
      snapshot: reserved.snapshot,
      proposal: reserved.proposal,
      executionTelemetry: reserved.executionTelemetry,
    };
  }
  if (!reserved.lease)
    throw new ConnectError(
      "Acquired workflow reservation has no lease",
      Code.DataLoss,
    );
  return {
    state: "acquired",
    operation: new HeldDurableOperation(
      kernel,
      { ...reserved, lease: reserved.lease },
      calls,
    ),
    snapshot: reserved.snapshot,
    proposal: reserved.proposal,
    executionTelemetry: reserved.executionTelemetry,
  };
}

/** A held workflow attempt. Domain code owns replay, commit and rejection meaning. */
class HeldDurableOperation implements DurableOperation {
  constructor(
    private readonly kernel: KernelClient,
    private readonly record: WorkflowReservation & {
      lease: NonNullable<WorkflowReservation["lease"]>;
    },
    private readonly calls: ReturnType<typeof executionOptions>,
  ) {}

  private get lease() {
    return { lease: this.record.lease };
  }

  private async save(update: WorkflowCheckpoint, options: ExecutionOptions) {
    if (
      (update.proposal !== undefined && update.outcome !== undefined) ||
      (update.proposal === undefined &&
        update.outcome === undefined &&
        update.executionTelemetry === undefined &&
        update.mutationOperations === undefined)
    )
      throw new ConnectError(
        "Invalid empty or mixed workflow checkpoint",
        Code.InvalidArgument,
      );
    await this.kernel.modelWorkflow.saveWorkflow(
      { ...this.lease, ...update },
      options,
    );
  }

  checkpoint(update: WorkflowCheckpoint): Promise<void> {
    return this.save(update, this.calls);
  }

  async mutate<T>(operationId: string, commit: () => Promise<T>): Promise<T> {
    await this.checkpoint({ mutationOperations: [operationId] });
    return commit();
  }

  async run<T>(work: () => Promise<T>): Promise<T> {
    try {
      return await work();
    } catch (error) {
      const execution = failedExecutionTelemetry(error);
      if (execution) {
        await this.save(
          { executionTelemetry: create(ExecutionTelemetrySchema, execution) },
          this.calls.opportunity.cleanup(),
        ).catch(() => {});
      }
      throw error;
    } finally {
      await this.kernel.modelWorkflow
        .releaseWorkflow(this.lease, this.calls.opportunity.cleanup())
        .catch(() => {});
    }
  }
}

export function operationBusyError(): ConnectError {
  return domainError(
    "Workflow is busy; retry the same operation",
    Code.Aborted,
    {
      code: ErrorCode.OPERATION_IN_PROGRESS,
      owner: ErrorOwner.MODEL_WORKFLOW,
      recovery: ErrorRecovery.RETRY_OPERATION,
    },
  );
}

export function isOperationBusyError(error: unknown): boolean {
  return (
    error instanceof ConnectError &&
    domainErrorDetail(error)?.code === ErrorCode.OPERATION_IN_PROGRESS
  );
}
