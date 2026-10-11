// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { ModelRole } from "../roles.js";
import { type ReadyRole } from "./role.js";
import { InputUnavailable } from "../input.js";
import { safeProviderFailure } from "../protocols.js";
export class GenerationFailure extends Error {
  constructor(
    role: ModelRole,
    readonly reason: string,
  ) {
    super(`Model role ${role} invocation failed: ${reason}`);
  }
}

export async function executeRoutes<T, Input = undefined>(
  name: ModelRole,
  routes: readonly ReadyRole[],
  invoke: (role: ReadyRole, input: Input) => Promise<T>,
  signal?: AbortSignal,
  beforeAttempt?: () => void,
  prepare?: (role: ReadyRole) => Input,
) {
  let lastFailure = "route_unavailable";
  for (const role of routes) {
    signal?.throwIfAborted();
    let prepared: Input;
    try {
      prepared = prepare ? prepare(role) : (undefined as Input);
    } catch (error) {
      if (!(error instanceof InputUnavailable)) throw error;
      lastFailure = error.message;
      continue;
    }
    // Budget admission is outside fallback handling. Exhaustion never tries another route.
    beforeAttempt?.();
    try {
      return await invoke(role, prepared);
    } catch (error) {
      lastFailure = signal?.aborted
        ? "caller_cancelled"
        : error instanceof GenerationFailure
          ? error.reason
          : safeProviderFailure(error);
      signal?.throwIfAborted();
    }
  }
  const failure = new GenerationFailure(name, lastFailure);
  failure.message = `Model role ${name} invocation failed: all_execution_routes_failed:${failure.reason}`;
  throw failure;
}
