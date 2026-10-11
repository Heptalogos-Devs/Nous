// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { conceptActionResultSchema } from "./concept-maintenance.js";
import { consolidationResultSchema } from "./consolidation.js";

import { z } from "zod";

const outcomeSchema = z.strictObject({
  status: z.enum([
    "committed",
    "partial",
    "stale",
    "no_change",
    "obsolete",
    "rejected_invalid",
  ]),
  problemCode: z.string().optional(),
  actions: z
    .array(z.union([consolidationResultSchema, conceptActionResultSchema]))
    .optional(),
});
export function readOutcome(saved: WorkflowPayload) {
  if (saved.purged) return { status: "obsolete" as const };
  return outcomeSchema.parse(JSON.parse(saved.payloadJson));
}
export const snapshotSchema = z.strictObject({
  plan: z.unknown(),
  model: z.unknown(),
});
export const proposalSchema = z.discriminatedUnion("action", [
  z.strictObject({ action: z.literal("partition"), request: z.unknown() }),
  z.strictObject({ action: z.literal("journal"), request: z.unknown() }),
  z.strictObject({
    action: z.literal("consolidation"),
    proposal: z.unknown(),
    producer: z.unknown(),
    progress: z.array(consolidationResultSchema),
  }),
  z.strictObject({
    action: z.literal("concept"),
    proposal: z.unknown(),
    producer: z.unknown(),
    progress: z.array(conceptActionResultSchema),
  }),
  z.strictObject({
    action: z.literal("withdraw"),
    journalId: z.string().uuid(),
    epoch: z.string(),
  }),
  z.strictObject({ action: z.literal("no_change") }),
]);

export type MaintenanceSnapshot = z.infer<typeof snapshotSchema>;
export type MaintenanceProposal = z.infer<typeof proposalSchema>;

import type { WorkflowPayload } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/workflow_envelope_pb.js";
import {
  MaintenancePlanSchema,
  type MaintenancePlan,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { protocolReferences } from "@nous-wave/client/data";
import type { Dependency } from "../durable-operation.js";
export function maintenanceDependencies(plan: MaintenancePlan): Dependency[] {
  return protocolReferences(plan, MaintenancePlanSchema);
}
export function resultDependencies(
  results: z.infer<typeof outcomeSchema>["actions"],
): Dependency[] {
  const refs: Dependency[] = [];
  for (const result of results ?? []) {
    if (result.resultRef) refs.push(result.resultRef);
    if ("tagResults" in result)
      for (const tag of result.tagResults) {
        refs.push({ kind: "tag", value: tag.tagId });
      }
  }
  return refs;
}
