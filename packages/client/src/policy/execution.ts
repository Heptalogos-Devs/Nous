// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { z } from "zod";

const confirmationTimeout = z.number().int().min(1).max(600000);
/** Core publishes this normalized policy through active Configuration. */
export const executionOpportunitySchema = z
  .strictObject({
    work_timeout_ms: z.number().int().min(1).max(900000).default(300000),
    cleanup_timeout_ms: confirmationTimeout.default(10000),
    acknowledgement_timeout_ms: confirmationTimeout.default(5000),
    response_margin_ms: confirmationTimeout.default(1000),
  })
  .prefault({});
export type ExecutionOpportunityPolicy = z.infer<
  typeof executionOpportunitySchema
>;
export const executionOpportunityPaths = Object.keys(
  executionOpportunitySchema.unwrap().shape,
).map((name) => `core_execution.opportunity.${name}`);

export function executionLeaseSeconds(
  policy: ExecutionOpportunityPolicy,
  remainingWorkMs: number,
) {
  return (
    Math.ceil(
      (Math.max(0, remainingWorkMs) +
        policy.cleanup_timeout_ms +
        policy.acknowledgement_timeout_ms) /
        1000,
    ) + 1
  );
}

export function executionEnvelope(
  policy: ExecutionOpportunityPolicy,
  workMs = policy.work_timeout_ms,
) {
  if (!Number.isInteger(workMs) || workMs < 1 || workMs > 900000)
    throw new Error("Execution work budget must be 1..900000 milliseconds");
  const confirmationMs =
    policy.cleanup_timeout_ms + policy.acknowledgement_timeout_ms;
  return {
    workMs,
    leaseSeconds: executionLeaseSeconds(policy, workMs),
    responseTimeoutMs: workMs + confirmationMs + policy.response_margin_ms,
  };
}
