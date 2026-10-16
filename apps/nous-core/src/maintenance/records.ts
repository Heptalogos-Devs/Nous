// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { JsonValue } from "@bufbuild/protobuf";
import type { ResultPayload } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_results_pb.js";
import {
  MaintenancePlanSchema,
  type MaintenancePlan,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { protocolReferences } from "@nous-wave/protocol/messages";
import type { Dependency } from "../operation-results.js";
import type { ModelGenerationOutput } from "../model/schemas/contracts.js";

export const maintenanceRoles = {
  subject_reflection: "subject_reflection",
  episode_resegment: "episode_segmentation",
  journal_review: "journal_synthesis",
  journal_revalidate: "journal_synthesis",
  memory_consolidate: "memory_consolidation",
  concept_maintenance: "concept_maintenance",
} as const;

export type MaintenanceActionResult = {
  index: number;
  status:
    | "committed"
    | "no_change"
    | "rejected_invalid"
    | "skipped_dependency"
    | "stale";
  resultRef: Dependency | null;
};
export type MaintenanceOutcome = {
  status:
    | "committed"
    | "partial"
    | "stale"
    | "no_change"
    | "obsolete"
    | "rejected_invalid";
  problemCode?: string;
  actions?: MaintenanceActionResult[];
};
export type MaintenanceProposal =
  | { action: "partition" | "journal"; request: JsonValue }
  | {
      action: "consolidation";
      input: JsonValue;
      proposal: ModelGenerationOutput<"memory_consolidation">;
      producer: JsonValue;
    }
  | {
      action: "concept";
      input: JsonValue;
      proposal: ModelGenerationOutput<"concept_maintenance">;
      producer: JsonValue;
    }
  | { action: "withdraw"; journalId: string; epoch: string }
  | { action: "no_change" };

/** These payloads are immutable Core-owned records, not provider or caller inputs. */
export function readOutcome(saved: ResultPayload): MaintenanceOutcome {
  return saved.purged
    ? { status: "obsolete" }
    : (JSON.parse(saved.payloadJson) as MaintenanceOutcome);
}
export function maintenanceDependencies(plan: MaintenancePlan): Dependency[] {
  return protocolReferences(plan, MaintenancePlanSchema);
}
export function resultDependencies(
  results: MaintenanceActionResult[] | undefined,
): Dependency[] {
  return (results ?? []).flatMap((result) =>
    result.resultRef ? [result.resultRef] : [],
  );
}
