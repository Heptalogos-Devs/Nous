// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { conceptActionResultSchema } from "./concept-maintenance.js";
import { consolidationResultSchema } from "./consolidation.js";

import {
  fromJson,
  toJson,
  type DescMessage,
  type JsonValue,
  type MessageShape,
} from "@bufbuild/protobuf";
import { z } from "zod";
import {
  ApplyEpisodePartitionRequestSchema,
  CommitJournalRequestSchema,
  MaintenancePlanSchema,
  type MaintenancePlan,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import type { ModelRoleSnapshot } from "../model/execution/snapshot.js";
import type { WorkflowPayload } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/workflow_envelope_pb.js";
import { ProducerSignatureSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { conceptMaintenanceSchema } from "../model/schemas/concept-maintenance.js";
import { consolidationSchema } from "../model/schemas/consolidation.js";

function protoJson<D extends DescMessage>(schema: D) {
  type T = MessageShape<D>;
  return z.codec(z.unknown(), z.custom<T>(), {
    decode: (value) => fromJson(schema, value as JsonValue),
    encode: (value) => toJson(schema, value),
  });
}

const episodePartitionRequestSchema = protoJson(
  ApplyEpisodePartitionRequestSchema,
);
const journalRequestSchema = protoJson(CommitJournalRequestSchema);
const producerSchema = protoJson(ProducerSignatureSchema);
const epochSchema = z.codec(z.string(), z.bigint(), {
  decode: BigInt,
  encode: (value) => value.toString(),
});

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
  plan: protoJson(MaintenancePlanSchema),
  model: z.custom<ModelRoleSnapshot | null>(),
});
export const proposalSchema = z.discriminatedUnion("action", [
  z.strictObject({
    action: z.literal("partition"),
    request: episodePartitionRequestSchema,
  }),
  z.strictObject({
    action: z.literal("journal"),
    request: journalRequestSchema,
  }),
  z.strictObject({
    action: z.literal("consolidation"),
    proposal: consolidationSchema,
    producer: producerSchema,
    progress: z.array(consolidationResultSchema),
  }),
  z.strictObject({
    action: z.literal("concept"),
    proposal: conceptMaintenanceSchema,
    producer: producerSchema,
    progress: z.array(conceptActionResultSchema),
  }),
  z.strictObject({
    action: z.literal("withdraw"),
    journalId: z.string().uuid(),
    epoch: epochSchema,
  }),
  z.strictObject({ action: z.literal("no_change") }),
]);

export type MaintenanceSnapshot = z.infer<typeof snapshotSchema>;
export type MaintenanceProposal = z.infer<typeof proposalSchema>;

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
