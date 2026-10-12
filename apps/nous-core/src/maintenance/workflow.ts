// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { executeConceptMaintenance } from "./concept-maintenance.js";
import { executeConsolidation } from "./consolidation.js";
import {
  maintenanceOperationId,
  maintenanceActionOperationId,
} from "./identity.js";
import { authorityRejection } from "./rejection.js";
import { executionOptions, type ExecutionOptions } from "../execution.js";
import { fromJson, toJson } from "@bufbuild/protobuf";
import { z } from "zod";
import {
  MaintenancePlanSchema,
  ApplyEpisodePartitionRequestSchema,
  ApplyEpisodePartitionResponseSchema,
  CommitJournalRequestSchema,
  type MaintenanceNeed,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { ProducerSignatureSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  GenerationFailure,
  failedExecutionTelemetry,
} from "../model/execution/routes.js";
import { type KernelClient } from "../kernel-client.js";
import { type ModelRuntime } from "../model/runtime.js";

import { JournalResponseSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/journal_pb.js";
import { protocolReferences } from "@nous-wave/protocol/messages";
import type { Dependency } from "../durable-operation.js";
import { canonicalDigest } from "../digest.js";

import {
  startOperation,
  serializeOperation,
  workflowPayload,
} from "../durable-operation.js";
import {
  maintenanceDependencies,
  resultDependencies,
  readOutcome,
  type MaintenanceSnapshot,
  type MaintenanceProposal,
} from "./records.js";
import { buildMaintenanceProposal } from "./proposal.js";

export async function runModelMaintenance(
  kernel: KernelClient,
  models: ModelRuntime,
  need: MaintenanceNeed,
  options: ExecutionOptions,
  reserveModelCall: () => void,
) {
  const calls = executionOptions(kernel.execution.opportunity, options);
  return serializeOperation(
    kernel,
    `memory:${need.subjectId}:${maintenanceOperationId(need)}`,
    calls.signal,
    () =>
      runModelMaintenanceOnce(kernel, models, need, calls, reserveModelCall),
  );
}

async function runModelMaintenanceOnce(
  kernel: KernelClient,
  models: ModelRuntime,
  need: MaintenanceNeed,
  options: ExecutionOptions,
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
  const previous = await kernel.modelWorkflow.readOperation(identity, options);
  if (previous.outcome) return readOutcome(previous.outcome);
  let snapshot: Parameters<typeof startOperation>[2] | undefined =
    previous.snapshot;
  if (!snapshot) {
    const plan = await kernel.maintenance.planMaintenance(
      { selected: need },
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
    const role =
      need.kind === "episode_resegment"
        ? "episode_segmentation"
        : need.kind === "concept_maintenance"
          ? "concept_maintenance"
          : need.kind === "memory_consolidate"
            ? "memory_consolidation"
            : "journal_synthesis";
    const model =
      plan.status === "withdraw" ? null : models.invocations.snapshot(role);
    snapshot = {
      content: workflowPayload(
        { plan: toJson(MaintenancePlanSchema, plan), model },
        maintenanceDependencies(plan),
      ),
    };
  }
  const operation = await startOperation(
    kernel,
    identity,
    {
      ...snapshot,
      maintenanceTrigger: {
        $typeName: "nous.wave.kernel.v1alpha1.MaintenanceTrigger",
        needId: need.needId,
        triggerAuthoritySeq: need.triggerAuthoritySeq,
        triggerRevision: need.triggerRevision,
      },
    },
    options,
  );
  const record = operation.record;
  if (record.outcome) return readOutcome(record.outcome);
  return operation.run(async () => {
    try {
      const frozenSnapshot = JSON.parse(
        record.snapshot!.content!.payloadJson,
      ) as MaintenanceSnapshot;
      const plan = fromJson(MaintenancePlanSchema, frozenSnapshot.plan);
      let proposed = record.proposal?.payloadJson
        ? (JSON.parse(record.proposal.payloadJson) as MaintenanceProposal)
        : undefined;
      if (!proposed) {
        proposed = await buildMaintenanceProposal({
          models,
          need,
          plan,
          snapshot: frozenSnapshot,
          operationId,
          options,
          reserveModelCall,
          recordExecution: (value) => operation.recordExecution(value),
        });
        const actions =
          proposed.action === "concept"
            ? proposed.proposal.actions
            : proposed.action === "consolidation"
              ? proposed.proposal.actions
              : [];
        await operation.saveProposal(
          proposed,
          [],
          undefined,
          actions.map((action, index) =>
            maintenanceActionOperationId(operationId, index, action.action),
          ),
        );
      }
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
        const request = fromJson(CommitJournalRequestSchema, proposed.request);
        const result = await kernel.maintenance.commitJournal(request, options);
        terminalDependencies = protocolReferences(
          result,
          JournalResponseSchema,
        );
      } else if (proposed.action === "concept") {
        const saved = proposed;
        const outcome = await executeConceptMaintenance(
          kernel,
          plan,
          saved.proposal,
          operationId,
          fromJson(ProducerSignatureSchema, saved.producer),
          options,
        );
        await operation.complete(outcome, resultDependencies(outcome.actions));
        return outcome;
      } else if (proposed.action === "consolidation") {
        const saved = proposed;
        const outcome = await executeConsolidation(
          kernel,
          plan,
          saved.proposal,
          operationId,
          fromJson(ProducerSignatureSchema, saved.producer),
          options,
        );
        await operation.complete(outcome, resultDependencies(outcome.actions));
        return outcome;
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
      await operation.complete(outcome, terminalDependencies);
      return outcome;
    } catch (error) {
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
        const execution = failedExecutionTelemetry(error);
        if (execution) await operation.recordExecution(execution);
        await operation.complete(outcome);
        return outcome;
      }
      if (authorityRejection(error) === "stale") {
        const outcome = { status: "obsolete" as const };
        const execution = failedExecutionTelemetry(error);
        if (execution) await operation.recordExecution(execution);
        await operation.complete(outcome);
        await kernel.maintenance.refreshMaintenance(
          { selected: need },
          options,
        );
        return outcome;
      }
      throw error;
    }
  });
}
