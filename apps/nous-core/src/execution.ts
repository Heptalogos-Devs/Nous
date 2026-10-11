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

/** One work clock, followed by bounded cleanup and a separate need acknowledgement. */
export class ExecutionOpportunity {
  readonly signal: AbortSignal;
  private readonly workUntil: number;
  private cleanupUntil: number;
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
    this.cleanupUntil = this.workUntil + policy.cleanup_timeout_ms;
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
    const canceled = () => {
      this.cleanupUntil = Math.min(
        this.cleanupUntil,
        performance.now() + policy.cleanup_timeout_ms,
      );
    };
    if (this.signal.aborted) canceled();
    else this.signal.addEventListener("abort", canceled, { once: true });
  }
  get leaseSeconds() {
    return executionLeaseSeconds(
      this.policy,
      this.workUntil - performance.now(),
    );
  }
  cleanup(): CallOptions {
    const remaining = Math.min(
      this.policy.cleanup_timeout_ms,
      Math.ceil(this.cleanupUntil - performance.now()),
    );
    if (remaining <= 0)
      return {
        signal: AbortSignal.abort(
          new ConnectError(
            "Execution cleanup budget exhausted",
            Code.DeadlineExceeded,
          ),
        ),
        timeoutMs: 1,
      };
    return { signal: AbortSignal.timeout(remaining), timeoutMs: remaining };
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
