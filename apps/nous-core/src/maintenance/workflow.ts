// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { Code, ConnectError } from "@connectrpc/connect";
import { z } from "zod";
import { type MaintenanceNeed } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { CognitionMutationRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/cognition_pb.js";
import { CognitiveRefSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { executionOptions, type ExecutionOptions } from "../execution.js";
import { type KernelClient } from "../kernel-client.js";
import { type ModelRuntime } from "../model/runtime.js";
import { GenerationFailure } from "../model/execution/routes.js";
import { executeConceptMaintenance } from "./concept-maintenance.js";
import { executeConsolidation } from "./consolidation.js";
import { authorityRejection } from "./rejection.js";
import { maintenanceOperationId } from "./identity.js";
import { buildMaintenanceProposal } from "./proposal.js";

/** Run one claimed need within the caller's maintenance grant. */
export async function runModelMaintenance(
  kernel: KernelClient,
  models: ModelRuntime,
  need: MaintenanceNeed,
  options: ExecutionOptions,
  reserveModelCall: () => void,
) {
  options = executionOptions(kernel.execution.opportunity, options);
  const plan = await kernel.maintenance.planMaintenance(
    { claimed: need },
    options,
  );
  if (plan.status === "obsolete") return { status: "obsolete" as const };
  if (plan.status === "blocked" || plan.status === "deferred")
    return {
      status:
        plan.status === "blocked"
          ? ("blocked_dependency" as const)
          : ("deferred" as const),
      problemCode: plan.problemCode ?? "source_not_settled",
      nextDue: plan.nextDue,
    };

  const operationId = maintenanceOperationId(need);
  try {
    const { proposal } = await buildMaintenanceProposal({
      models,
      need,
      plan,
      operationId,
      options,
      reserveModelCall,
    });

    if (proposal.action === "partition") {
      await kernel.maintenance.applyEpisodePartition(proposal.request, options);
    } else if (proposal.action === "journal") {
      await kernel.maintenance.commitJournal(proposal.request, options);
    } else if (proposal.action === "concept") {
      return await executeConceptMaintenance(
        kernel,
        plan,
        proposal.proposal,
        operationId,
        proposal.producer,
        options,
      );
    } else if (proposal.action === "consolidation") {
      return await executeConsolidation(
        kernel,
        plan,
        proposal.proposal,
        operationId,
        proposal.producer,
        options,
      );
    } else if (proposal.action === "withdraw") {
      await kernel.memory.mutateCognition(
        create(CognitionMutationRequestSchema, {
          operationId,
          subjectId: need.subjectId,
          objectRef: create(CognitiveRefSchema, {
            kind: "journal",
            value: proposal.journalId,
          }),
          expectedObjectEpoch: BigInt(proposal.epoch),
          action: { case: "withdraw", value: {} },
        }),
        options,
      );
    }

    if (
      (proposal.action === "partition" && need.scopeKind === "track") ||
      (proposal.action === "journal" && need.kind === "journal_review")
    )
      await kernel.maintenance.refreshMaintenance({ claimed: need }, options);

    return {
      status:
        proposal.action === "no_change"
          ? ("no_change" as const)
          : ("committed" as const),
    };
  } catch (error) {
    if (
      authorityRejection(error) === "rejected_invalid" ||
      error instanceof z.ZodError ||
      (error instanceof GenerationFailure &&
        ["output_schema_invalid", "output_json_invalid"].includes(error.reason))
    )
      return {
        status: "rejected_invalid" as const,
        problemCode: "proposal_invalid",
      };
    if (authorityRejection(error) === "stale") {
      await kernel.maintenance.refreshMaintenance({ claimed: need }, options);
      return { status: "obsolete" as const };
    }
    if (error instanceof ConnectError && error.code === Code.InvalidArgument)
      return {
        status: "rejected_invalid" as const,
        problemCode: "proposal_invalid",
      };
    throw error;
  }
}
