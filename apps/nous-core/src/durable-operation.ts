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
  MaintenanceClaimSchema,
  WorkflowPayloadSchema,
  WorkflowSnapshotSchema,
  type ExecutionTelemetry,
  type MaintenanceClaim,
  type WorkflowPayload,
  type WorkflowSnapshot,
  type WorkflowLease,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/workflow_envelope_pb.js";
import type {
  OpenWorkflowRequest,
  CompletedWorkflow,
  PendingWorkflow,
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
type ExecutionTelemetryInput = MessageInitShape<
  typeof ExecutionTelemetrySchema
>;
export type DurableIdentity = Pick<
  OpenWorkflowRequest,
  "subjectId" | "owner" | "operationKey" | "semanticDigest"
>;
type OperationSnapshot = MessageInitShape<typeof WorkflowSnapshotSchema>;
type SnapshotDecision<T = never> =
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
  mutationOperationId?: string;
};

interface DurableOperation {
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

/** Opens a frozen operation through one atomic existing-or-acquire path. */
export async function openDurableOperation<T = never>(
  kernel: KernelClient,
  identity: DurableIdentity,
  input: OpenDurableOperationInput<T>,
): Promise<OpenDurableOperation<T>> {
  const calls = executionOptions(kernel.execution.opportunity, input.execution);
  const claim = input.maintenanceClaim
    ? create(MaintenanceClaimSchema, input.maintenanceClaim)
    : undefined;
  const open = (snapshot?: OperationSnapshot) =>
    kernel.modelWorkflow.openWorkflow(
      {
        ...identity,
        snapshot: snapshot
          ? create(WorkflowSnapshotSchema, snapshot)
          : undefined,
        maintenanceClaim: claim,
        leaseSeconds: calls.opportunity.leaseSeconds,
      },
      calls,
    );

  let reserved = await open();
  if (reserved.state.case === "missing") {
    const decision = await input.snapshotFactory();
    if ("skip" in decision)
      return {
        state: "not_started",
        result: decision.skip,
      } as OpenDurableOperation<T>;
    reserved = await open(decision.snapshot);
  }
  switch (reserved.state.case) {
    case "completed":
      return completed(reserved.state.value);
    case "busy": {
      const pending = reserved.state.value;
      return {
        state: "busy",
        snapshot: pendingSnapshot(pending),
        proposal: pending.proposal,
        executionTelemetry: pending.executionTelemetry,
      };
    }
    case "acquired": {
      const { pending, lease } = reserved.state.value;
      if (!pending || !lease)
        throw new ConnectError(
          "Incomplete workflow acquisition",
          Code.DataLoss,
        );
      return {
        state: "acquired",
        operation: new HeldDurableOperation(kernel, lease, calls),
        snapshot: pendingSnapshot(pending),
        proposal: pending.proposal,
        executionTelemetry: pending.executionTelemetry,
      };
    }
    case "missing":
      throw new ConnectError(
        "Workflow snapshot was not reserved",
        Code.DataLoss,
      );
    default:
      throw new ConnectError(
        "Missing workflow reservation state",
        Code.DataLoss,
      );
  }
}

function pendingSnapshot(pending: PendingWorkflow): WorkflowSnapshot {
  if (!pending.snapshot)
    throw new ConnectError("Pending workflow has no snapshot", Code.DataLoss);
  return pending.snapshot;
}
function completed(
  value: CompletedWorkflow,
): Extract<OpenDurableOperation, { state: "completed" }> {
  if (!value.outcome)
    throw new ConnectError("Completed workflow has no outcome", Code.DataLoss);
  return {
    state: "completed",
    outcome: value.outcome,
    executionTelemetry: value.executionTelemetry,
  };
}

/** A held workflow attempt. Domain code owns replay, commit and rejection meaning. */
class HeldDurableOperation implements DurableOperation {
  constructor(
    private readonly kernel: KernelClient,
    private readonly heldLease: WorkflowLease,
    private readonly calls: ReturnType<typeof executionOptions>,
  ) {}

  private get lease() {
    return { lease: this.heldLease };
  }

  private async save(update: WorkflowCheckpoint, options: ExecutionOptions) {
    if (
      (update.proposal !== undefined && update.outcome !== undefined) ||
      (update.proposal === undefined &&
        update.outcome === undefined &&
        update.executionTelemetry === undefined &&
        update.mutationOperationId === undefined)
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
    await this.checkpoint({ mutationOperationId: operationId });
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
