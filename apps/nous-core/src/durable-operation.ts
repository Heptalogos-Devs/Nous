// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, type MessageInitShape } from "@bufbuild/protobuf";
import {
  WorkflowPayloadSchema,
  ExecutionTelemetrySchema,
  type WorkflowSnapshot,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/workflow_envelope_pb.js";
import type {
  OperationIdentity,
  OperationRecord,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js";
import type { CognitiveRef } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { KernelClient } from "./kernel-client.js";
import { executionOptions, type ExecutionOptions } from "./execution.js";
import { failedExecutionTelemetry } from "./model/execution/routes.js";

export type Dependency = Pick<CognitiveRef, "kind" | "value">;
type Identity = Omit<OperationIdentity, "$typeName">;
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

/** Records retain paid proposals and exact outcomes; Core serializes each operation. */
export async function startOperation(
  kernel: KernelClient,
  identity: Identity,
  snapshot: Omit<WorkflowSnapshot, "$typeName">,
  options: ExecutionOptions,
) {
  const calls = executionOptions(kernel.execution.opportunity, options);
  const record = await kernel.modelWorkflow.startOperation(
    {
      identity,
      snapshot,
    },
    calls,
  );
  return new DurableOperation(kernel, identity, record, calls);
}

class DurableOperation {
  constructor(
    private readonly kernel: KernelClient,
    readonly identity: Identity,
    readonly record: OperationRecord,
    private readonly calls: ReturnType<typeof executionOptions>,
  ) {}
  async recordExecution(execution: Telemetry) {
    await this.kernel.modelWorkflow.saveOperation(
      {
        identity: this.identity,
        executionTelemetry: execution,
      },
      this.calls.opportunity.cleanup(),
    );
  }
  async saveProposal(
    value: unknown,
    dependencies: Dependency[] = [],
    execution?: Telemetry,
    mutationOperations: string[] = [],
  ) {
    await this.kernel.modelWorkflow.saveOperation(
      {
        identity: this.identity,
        proposal: workflowPayload(value, dependencies),
        executionTelemetry: execution,
        mutationOperations,
      },
      this.calls.opportunity.cleanup(),
    );
  }
  async complete(value: unknown, dependencies: Dependency[] = []) {
    await this.kernel.modelWorkflow.saveOperation(
      {
        identity: this.identity,
        outcome: workflowPayload(value, dependencies),
      },
      this.calls.opportunity.cleanup(),
    );
  }
  async run<T>(work: () => Promise<T>): Promise<T> {
    try {
      return await work();
    } catch (error) {
      const execution = failedExecutionTelemetry(error);
      if (execution) await this.recordExecution(execution);
      throw error;
    }
  }
}

const operationQueues = new WeakMap<KernelClient, Map<string, Promise<void>>>();

/** One host owns the Kernel; concurrent callers wait and then replay durable results. */
export async function serializeOperation<T>(
  kernel: KernelClient,
  key: string,
  signal: AbortSignal | undefined,
  work: () => Promise<T>,
): Promise<T> {
  let queue = operationQueues.get(kernel);
  if (!queue)
    operationQueues.set(kernel, (queue = new Map<string, Promise<void>>()));
  const previous = queue.get(key) ?? Promise.resolve();
  let release!: () => void;
  const current = new Promise<void>((resolve) => {
    release = resolve;
  });
  const tail = previous.then(() => current);
  queue.set(key, tail);
  let abort: (() => void) | undefined;
  try {
    if (signal?.aborted) throw signal.reason;
    if (signal) {
      await Promise.race([
        previous,
        new Promise<never>((_, reject) => {
          abort = () => reject(signal.reason);
          signal.addEventListener("abort", abort, { once: true });
        }),
      ]);
    } else await previous;
    if (signal?.aborted) throw signal.reason;
    return await work();
  } finally {
    if (abort) signal?.removeEventListener("abort", abort);
    release();
    void tail.then(() => {
      if (queue.get(key) === tail) queue.delete(key);
    });
  }
}
