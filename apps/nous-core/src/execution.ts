// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { Code, ConnectError, type CallOptions } from "@connectrpc/connect";
import {
  executionEnvelope,
  executionLeaseSeconds,
  type ExecutionOpportunityPolicy,
} from "@nous-wave/client/execution-policy";

export type ExecutionOptions = CallOptions & {
  opportunity?: ExecutionOpportunity;
};

/** One work clock and a separate maintenance status acknowledgement. */
export class ExecutionOpportunity {
  readonly signal: AbortSignal;
  private readonly workUntil: number;
  constructor(
    readonly policy: ExecutionOpportunityPolicy,
    caller: CallOptions = {},
    workMs = policy.work_timeout_ms,
  ) {
    executionEnvelope(policy, workMs);
    const effectiveWorkMs = Math.min(
      workMs,
      caller.timeoutMs && caller.timeoutMs > 0 ? caller.timeoutMs : workMs,
    );
    this.workUntil = performance.now() + effectiveWorkMs;
    const timeout = AbortSignal.timeout(effectiveWorkMs);
    const work = new AbortController();
    timeout.addEventListener(
      "abort",
      () =>
        work.abort(
          new ConnectError(
            "Execution work budget exhausted",
            Code.DeadlineExceeded,
          ),
        ),
      { once: true },
    );
    this.signal = caller.signal
      ? AbortSignal.any([caller.signal, work.signal])
      : work.signal;
  }
  get leaseSeconds() {
    return executionLeaseSeconds(
      this.policy,
      this.workUntil - performance.now(),
    );
  }
  acknowledge(): CallOptions {
    return { timeoutMs: this.policy.acknowledgement_timeout_ms };
  }
}

export function executionOptions(
  policy: ExecutionOpportunityPolicy,
  options: ExecutionOptions = {},
  workMs?: number,
): ExecutionOptions & {
  opportunity: ExecutionOpportunity;
  signal: AbortSignal;
} {
  const opportunity =
    options.opportunity ?? new ExecutionOpportunity(policy, options, workMs);
  return { ...options, signal: opportunity.signal, opportunity };
}
