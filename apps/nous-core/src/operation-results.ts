// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, type MessageInitShape } from "@bufbuild/protobuf";
import {
  ResultPayloadSchema,
  ExecutionTelemetrySchema,
  ModelResultContextSchema,
  type ModelResultContext,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_results_pb.js";
import type { OperationIdentity } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js";
import type { CognitiveRef } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { KernelClient } from "./kernel-client.js";
import { executionOptions, type ExecutionOptions } from "./execution.js";
import { failedExecutionTelemetry } from "./model/execution/routes.js";

export type Dependency = Pick<CognitiveRef, "kind" | "value">;
type Identity = Omit<OperationIdentity, "$typeName">;
type Telemetry = MessageInitShape<typeof ExecutionTelemetrySchema>;

export function resultPayload(
  payload: unknown,
  dependencies: Dependency[] = [],
) {
  return create(ResultPayloadSchema, {
    payloadJson: JSON.stringify(payload),
    dependencies,
  });
}

type ResultContext = MessageInitShape<typeof ModelResultContextSchema>;
type ModelProposal<P> = {
  proposal: P;
  context?: ResultContext;
  dependencies?: Dependency[];
  execution?: Telemetry;
  mutationOperations?: string[];
};
type Committed<T> = { value: T; outcome: unknown; dependencies?: Dependency[] };

/** Owns result replay, saving paid proposals with telemetry, and recording owner outcomes. */
export async function runRecordedOperation<P, T>(
  kernel: KernelClient,
  identity: Identity,
  options: ExecutionOptions,
  work: {
    context?: ResultContext;
    replay: (
      outcome: import("@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_results_pb.js").ResultPayload,
    ) => Promise<T> | T;
    generate: () => Promise<ModelProposal<P> | { value: T }>;
    commit: (
      proposal: P,
      context: ModelResultContext | undefined,
      reused: boolean,
    ) => Promise<Committed<T>>;
    reject?: (error: unknown) => Promise<Committed<T> | undefined>;
  },
): Promise<T> {
  const calls = executionOptions(kernel.execution.opportunity, options);
  const saved = await kernel.modelResults.readOperation(identity, calls);
  if (saved.outcome) return work.replay(saved.outcome);
  try {
    let proposal: P;
    let context = saved.context;
    if (saved.proposal) proposal = JSON.parse(saved.proposal.payloadJson) as P;
    else {
      const generated = await work.generate();
      if ("value" in generated) return generated.value;
      proposal = generated.proposal;
      context = generated.context
        ? create(ModelResultContextSchema, generated.context)
        : undefined;
      await kernel.modelResults.saveOperation(
        {
          identity,
          context,
          proposal: resultPayload(proposal, generated.dependencies),
          executionTelemetry: generated.execution,
          mutationOperations: generated.mutationOperations,
        },
        calls.opportunity.cleanup(),
      );
    }
    const result = await work.commit(
      proposal,
      context,
      saved.proposal !== undefined,
    );
    await kernel.modelResults.saveOperation(
      {
        identity,
        outcome: resultPayload(result.outcome, result.dependencies),
      },
      calls.opportunity.cleanup(),
    );
    return result.value;
  } catch (error) {
    const result = await work.reject?.(error);
    const attempts = failedExecutionTelemetry(error);
    const execution = attempts?.attempts.some(
      (attempt) => attempt.status !== "skipped",
    )
      ? attempts
      : undefined;
    if (execution || result)
      await kernel.modelResults.saveOperation(
        {
          identity,
          executionTelemetry: execution,
          context: work.context,
          outcome: result
            ? resultPayload(result.outcome, result.dependencies)
            : undefined,
        },
        calls.opportunity.cleanup(),
      );
    if (result) return result.value;
    throw error;
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
