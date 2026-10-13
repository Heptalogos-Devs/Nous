// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { executeConceptMaintenance } from "./concept-maintenance.js";
import { executeConsolidation } from "./consolidation.js";
import {
  maintenanceOperationId,
  maintenanceActionOperationId,
} from "./identity.js";
import { authorityRejection } from "./rejection.js";
import type { ExecutionOptions } from "../execution.js";
import { fromJson } from "@bufbuild/protobuf";
import { z } from "zod";
import {
  MaintenancePlanSchema,
  ApplyEpisodePartitionRequestSchema,
  ApplyEpisodePartitionResponseSchema,
  CommitJournalRequestSchema,
  type MaintenanceNeed,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { ProducerSignatureSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { GenerationFailure } from "../model/execution/routes.js";
import { type KernelClient } from "../kernel-client.js";
import { type ModelRuntime } from "../model/runtime.js";

import { JournalResponseSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/journal_pb.js";
import { protocolReferences } from "@nous-wave/protocol/messages";
import type { Dependency } from "../operation-results.js";
import { canonicalDigest } from "../digest.js";

import { runRecordedOperation } from "../operation-results.js";
import {
  maintenanceDependencies,
  resultDependencies,
  readOutcome,
  type MaintenanceOutcome,
  type MaintenanceProposal,
  maintenanceRoles,
} from "./records.js";
import { buildMaintenanceProposal } from "./proposal.js";

export async function runModelMaintenance(
  kernel: KernelClient,
  models: ModelRuntime,
  need: MaintenanceNeed,
  options: ExecutionOptions,
  modelBudget: number,
  reserveModelCall: () => void,
) {
  const operationId = maintenanceOperationId(need);
  const identity = {
    subjectId: need.subjectId,
    owner: "memory",
    operationKey: operationId,
    semanticDigest: canonicalDigest({
      needId: need.needId,
      trigger: need.triggerAuthoritySeq.toString(),
      triggerRevision: need.triggerRevision.toString(),
      kind: need.kind,
      scopeKind: need.scopeKind,
      scopeRef: need.scopeRef,
    }),
  };
  type Result =
    | MaintenanceOutcome
    | {
        status: "deferred" | "blocked_dependency";
        problemCode: string;
        nextDue?: import("@bufbuild/protobuf/wkt").Timestamp;
      };
  return runRecordedOperation<MaintenanceProposal, Result>(
    kernel,
    identity,
    options,
    {
      context: {
        maintenanceTrigger: {
          needId: need.needId,
          triggerAuthoritySeq: need.triggerAuthoritySeq,
          triggerRevision: need.triggerRevision,
        },
      },
      replay: readOutcome,
      generate: async () => {
        const plan = await kernel.maintenance.planMaintenance(
          { selected: need },
          options,
        );
        if (plan.status === "obsolete")
          return { value: { status: "obsolete" } };
        if (plan.status === "blocked" || plan.status === "deferred")
          return {
            value: {
              status:
                plan.status === "blocked" ? "blocked_dependency" : "deferred",
              problemCode: plan.problemCode ?? "source_not_settled",
              nextDue: plan.nextDue,
            },
          };
        if (plan.status === "withdraw") {
          if (!plan.target)
            throw new Error("Journal withdrawal requires a bound target");
          await kernel.memory.withdrawJournal(
            {
              operationId,
              subjectId: need.subjectId,
              journalId: plan.target.journalId,
              expectedObjectEpoch: plan.target.expectedEpoch,
            },
            options,
          );
          return { value: { status: "committed" } };
        }
        const role =
          maintenanceRoles[need.kind as keyof typeof maintenanceRoles];
        if (!role) throw new Error(`Unknown maintenance kind: ${need.kind}`);
        if (!modelBudget || !models.invocations.profile(role))
          return {
            value: {
              status: "deferred",
              problemCode: !modelBudget
                ? "model_budget_exhausted"
                : "model_unavailable",
            },
          };
        const generated = await buildMaintenanceProposal({
          models,
          need,
          plan,
          operationId,
          options,
          reserveModelCall,
          model: models.invocations.prepare(role),
        });
        const actions =
          generated.proposal.action === "concept" ||
          generated.proposal.action === "consolidation"
            ? generated.proposal.proposal.actions
            : [];
        return {
          ...generated,
          context: {
            dependencies: maintenanceDependencies(plan),
            maintenanceTrigger: {
              needId: need.needId,
              triggerAuthoritySeq: need.triggerAuthoritySeq,
              triggerRevision: need.triggerRevision,
            },
          },
          mutationOperations: actions.map((action, index) =>
            maintenanceActionOperationId(operationId, index, action.action),
          ),
        };
      },
      commit: async (proposed) => {
        let terminalDependencies: Dependency[] = [];
        if (proposed.action === "partition") {
          const request = fromJson(
            ApplyEpisodePartitionRequestSchema,
            proposed.request,
          );
          const result = await kernel.maintenance.applyEpisodePartition(
            request,
            options,
          );
          terminalDependencies = protocolReferences(
            result,
            ApplyEpisodePartitionResponseSchema,
          );
        } else if (proposed.action === "journal") {
          const request = fromJson(
            CommitJournalRequestSchema,
            proposed.request,
          );
          const result = await kernel.maintenance.commitJournal(
            request,
            options,
          );
          terminalDependencies = protocolReferences(
            result,
            JournalResponseSchema,
          );
        } else if (proposed.action === "concept") {
          const saved = proposed;
          const plan = fromJson(MaintenancePlanSchema, saved.input);
          const outcome = await executeConceptMaintenance(
            kernel,
            plan,
            saved.proposal,
            operationId,
            fromJson(ProducerSignatureSchema, saved.producer),
            options,
          );
          return {
            value: outcome,
            outcome,
            dependencies: resultDependencies(outcome.actions),
          };
        } else if (proposed.action === "consolidation") {
          const saved = proposed;
          const plan = fromJson(MaintenancePlanSchema, saved.input);
          const outcome = await executeConsolidation(
            kernel,
            plan,
            saved.proposal,
            operationId,
            fromJson(ProducerSignatureSchema, saved.producer),
            options,
          );
          return {
            value: outcome,
            outcome,
            dependencies: resultDependencies(outcome.actions),
          };
        } else if (proposed.action === "withdraw")
          await kernel.memory.withdrawJournal(
            {
              operationId,
              subjectId: need.subjectId,
              journalId: proposed.journalId,
              expectedObjectEpoch: BigInt(proposed.epoch),
            },
            options,
          );
        if (
          (proposed.action === "partition" && need.scopeKind === "track") ||
          (proposed.action === "journal" && need.kind === "journal_review")
        )
          await kernel.maintenance.refreshMaintenance(
            { selected: need },
            options,
          );
        const outcome = {
          status:
            proposed.action === "no_change"
              ? ("no_change" as const)
              : ("committed" as const),
        };
        return { value: outcome, outcome, dependencies: terminalDependencies };
      },
      reject: async (error) => {
        if (
          authorityRejection(error) === "rejected_invalid" ||
          error instanceof z.ZodError ||
          (error instanceof GenerationFailure &&
            ["output_schema_invalid", "output_json_invalid"].includes(
              error.reason,
            ))
        ) {
          const outcome = {
            status: "rejected_invalid" as const,
            problemCode: "proposal_invalid",
          };
          return { value: outcome, outcome };
        }
        if (authorityRejection(error) === "stale") {
          await kernel.maintenance.refreshMaintenance(
            { selected: need },
            options,
          );
          const outcome = { status: "obsolete" as const };
          return { value: outcome, outcome };
        }
        return undefined;
      },
    },
  );
}
