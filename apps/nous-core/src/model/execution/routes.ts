// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { ModelRole } from "../roles.js";
import {
  hydrateRole,
  type ReadyRole,
  type ModelRoleSnapshot,
  type ModelProducerMetadata,
} from "./snapshot.js";
import { InputUnavailable } from "../input.js";
import {
  safeProviderFailure,
  usageCounts,
  ProviderFailure,
} from "../protocols.js";
import type {
  ExecutionAttempt as WireAttempt,
  ExecutionTelemetry as WireTelemetry,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/workflow_envelope_pb.js";
type ExecutionAttempt = Omit<WireAttempt, "$typeName" | "status" | "usage"> & {
  status: "succeeded" | "failed" | "skipped" | "unknown";
  usage?: ReturnType<typeof usageCounts>;
};
export type ExecutionTelemetry = Omit<
  WireTelemetry,
  "$typeName" | "attempts" | "omittedAttempts"
> & {
  attempts: ExecutionAttempt[];
  omittedAttempts?: number;
};
export class GenerationFailure extends Error {
  constructor(
    role: ModelRole,
    readonly reason: string,
    readonly execution?: ExecutionTelemetry,
    readonly usage?: unknown,
  ) {
    super(`Model role ${role} invocation failed: ${reason}`);
  }
}

const interruptedExecutions = new WeakMap<object, ExecutionTelemetry>();
export function failedExecutionTelemetry(
  error: unknown,
): ExecutionTelemetry | undefined {
  return error instanceof GenerationFailure
    ? (error.execution ?? interruptedExecutions.get(error))
    : error && typeof error === "object"
      ? interruptedExecutions.get(error)
      : undefined;
}
function interrupt(
  role: ModelRole,
  reason: unknown,
  attempts: ExecutionAttempt[],
): never {
  const error =
    reason && typeof reason === "object"
      ? reason
      : new GenerationFailure(role, "caller_cancelled", {
          attempts: [...attempts],
        });
  interruptedExecutions.set(error, { attempts: [...attempts] });
  throw error;
}
export async function executeRoutes<
  T extends {
    value: unknown;
    producerMetadata: ModelProducerMetadata;
    usage?: unknown;
  },
  Input = undefined,
>(
  name: ModelRole,
  snapshot: ModelRoleSnapshot,
  credentialOrigins: ReadonlySet<string>,
  invoke: (role: ReadyRole, input: Input) => Promise<T>,
  signal?: AbortSignal,
  beforeAttempt?: () => void,
  prepare?: (role: ReadyRole) => Input,
) {
  if (snapshot.role !== name) throw new Error("Reserved model role mismatch");
  const policy = snapshot.configuration.roles[name];
  const attempts: ExecutionAttempt[] = [];
  for (const executionName of policy?.routes ?? []) {
    if (signal?.aborted) interrupt(name, signal.reason, attempts);
    let role: ReadyRole;
    let prepared: Input;
    try {
      role = hydrateRole(snapshot, executionName, credentialOrigins);
      prepared = prepare ? prepare(role) : (undefined as Input);
    } catch (error) {
      attempts.push({
        executionProfile: executionName,
        modelProfile:
          snapshot.configuration.execution_profiles[executionName]?.model ?? "",
        status: "skipped",
        failureClass:
          error instanceof InputUnavailable
            ? error.message
            : "route_unavailable",
        latencyMs: 0,
      });
      continue;
    }
    // Budget admission is outside fallback handling. Exhaustion never tries another route.
    try {
      beforeAttempt?.();
    } catch (error) {
      interrupt(name, error, attempts);
    }
    const started = performance.now();
    try {
      const result = await invoke(role, prepared);
      attempts.push({
        executionProfile: executionName,
        modelProfile: role.binding.model,
        status: "succeeded",
        latencyMs: Math.round(performance.now() - started),
        usage: "usage" in result ? usageCounts(result.usage) : undefined,
      });
      const { usage: _usage, ...record } = result;
      return {
        ...record,
        execution: {
          attempts,
          successfulExecutionProfile: executionName,
        } satisfies ExecutionTelemetry,
      };
    } catch (error) {
      const failureClass = signal?.aborted
        ? "caller_cancelled"
        : error instanceof GenerationFailure
          ? error.reason
          : safeProviderFailure(error);
      attempts.push({
        executionProfile: executionName,
        modelProfile: role.binding.model,
        status:
          signal?.aborted ||
          (error instanceof ProviderFailure && error.outcome === "unknown") ||
          [
            "validation_or_transport",
            "timeout_or_cancellation",
            "embedding_validation_or_transport",
          ].includes(failureClass)
            ? "unknown"
            : "failed",
        failureClass,
        usage:
          error instanceof GenerationFailure || error instanceof ProviderFailure
            ? usageCounts(error.usage)
            : undefined,
        latencyMs: Math.round(performance.now() - started),
      });
      if (signal?.aborted) interrupt(name, signal.reason, attempts);
    }
  }
  const failure = new GenerationFailure(
    name,
    attempts.at(-1)?.failureClass ?? "route_unavailable",
    { attempts },
  );
  failure.message = `Model role ${name} invocation failed: all_execution_routes_failed:${failure.reason}`;
  throw failure;
}
