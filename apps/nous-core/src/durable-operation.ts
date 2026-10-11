// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, type MessageInitShape } from "@bufbuild/protobuf";
import { Code } from "@connectrpc/connect";
import {
  domainError,
  DomainErrorCode,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import {
  WorkflowPayloadSchema,
  ExecutionTelemetrySchema,
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
type Identity = Omit<FindWorkflowRequest, "$typeName">;
type Telemetry = MessageInitShape<typeof ExecutionTelemetrySchema>;

export function workflowPayload(
  payload: unknown,
  dependencies: Dependency[] = [],
) {
  return create(WorkflowPayloadSchema, {
    payloadJson: JSON.stringify(payload),
    dependencies,
  });
}

/** Lease mechanics are shared. Domain code owns replay, commit and rejection meaning. */
export async function reserveOperation(
  kernel: KernelClient,
  identity: Identity,
  snapshot: Omit<WorkflowSnapshot, "$typeName">,
  options: ExecutionOptions,
) {
  const calls = executionOptions(kernel.execution.opportunity, options);
  const record = await kernel.modelWorkflow.reserveWorkflow(
    {
      ...identity,
      snapshot,
      leaseSeconds: calls.opportunity.leaseSeconds,
    },
    calls,
  );
  return new DurableOperation(kernel, record, calls);
}

class DurableOperation {
  constructor(
    private readonly kernel: KernelClient,
    readonly record: WorkflowReservation,
    private readonly calls: ReturnType<typeof executionOptions>,
  ) {}
  private get lease() {
    return { lease: this.record.lease! };
  }
  async mutate<T>(operationId: string, commit: () => Promise<T>): Promise<T> {
    await this.kernel.modelWorkflow.saveWorkflow(
      {
        ...this.lease,
        mutationOperations: [operationId],
      },
      this.calls,
    );
    return commit();
  }
  async recordExecution(execution: Telemetry) {
    await this.kernel.modelWorkflow.saveWorkflow(
      {
        ...this.lease,
        executionTelemetry: execution,
      },
      this.calls.opportunity.cleanup(),
    );
  }
  async saveProposal(
    value: unknown,
    dependencies: Dependency[] = [],
    execution?: Telemetry,
  ) {
    await this.kernel.modelWorkflow.saveWorkflow(
      {
        ...this.lease,
        proposal: workflowPayload(value, dependencies),
        executionTelemetry: execution,
      },
      this.calls,
    );
  }
  async complete(value: unknown, dependencies: Dependency[] = []) {
    await this.kernel.modelWorkflow.saveWorkflow(
      {
        ...this.lease,
        outcome: workflowPayload(value, dependencies),
      },
      this.calls.opportunity.cleanup(),
    );
  }
  async run<T>(work: () => Promise<T>): Promise<T> {
    if (this.record.busy || !this.record.lease)
      throw domainError(
        "Workflow is busy; retry the same operation",
        Code.Aborted,
        {
          code: DomainErrorCode.OPERATION_IN_PROGRESS,
          recovery: ErrorRecovery.RETRY_OPERATION,
        },
      );
    try {
      return await work();
    } catch (error) {
      const execution = failedExecutionTelemetry(error);
      if (execution) await this.recordExecution(execution);
      throw error;
    } finally {
      await this.kernel.modelWorkflow
        .releaseWorkflow(this.lease, this.calls.opportunity.cleanup())
        .catch(() => {});
    }
  }
}
