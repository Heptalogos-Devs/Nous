// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { executeConceptMaintenance } from "./concept-maintenance.js";
import { executeConsolidation } from "./consolidation.js";
import { maintenanceOperationId } from "./identity.js";
import { authorityRejection } from "./rejection.js";
import { executionOptions, type ExecutionOptions } from "../execution.js";
import { z } from "zod";
import {
  ApplyEpisodePartitionResponseSchema,
  type MaintenanceNeed,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import {
  GenerationFailure,
  failedExecutionTelemetry,
} from "../model/execution/routes.js";
import { type KernelClient } from "../kernel-client.js";
import { type ModelRuntime } from "../model/runtime.js";

import { JournalResponseSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/journal_pb.js";
import { protocolReferences } from "@nous-wave/client/data";
import type { Dependency } from "../durable-operation.js";
import { canonicalDigest } from "../digest.js";

import { reserveOperation, workflowPayload } from "../durable-operation.js";
import {
  maintenanceDependencies,
  resultDependencies,
  readOutcome,
  snapshotSchema,
  proposalSchema,
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
  options = calls;
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
  const previous = await kernel.modelWorkflow.findWorkflow(identity, options);
  if (previous.outcome) return readOutcome(previous.outcome);
  let storedSnapshot: Parameters<typeof reserveOperation>[2] =
    previous.snapshot ?? {};
  if (!storedSnapshot.content?.payloadJson) {
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
    const role =
      need.kind === "episode_resegment"
        ? "episode_segmentation"
        : need.kind === "concept_maintenance"
          ? "concept_maintenance"
          : need.kind === "memory_consolidate"
            ? "memory_consolidation"
            : "journal_synthesis";
    const snapshot = {
      plan,
      model:
        plan.status === "withdraw" ? null : models.invocations.snapshot(role),
    };
    storedSnapshot = {
      content: workflowPayload(
        z.encode(snapshotSchema, snapshot),
        maintenanceDependencies(plan),
      ),
    };
  }
  const operation = await reserveOperation(
    kernel,
    identity,
    {
      ...storedSnapshot,
      maintenanceClaim: {
        $typeName: "nous.wave.kernel.v1alpha1.MaintenanceClaim",
        needId: need.needId,
        leaseToken: need.leaseToken!,
        triggerAuthoritySeq: need.triggerAuthoritySeq,
        triggerRevision: need.triggerRevision,
      },
    },
    options,
  );
  const reservation = operation.record;
  if (reservation.outcome) return readOutcome(reservation.outcome);
  if (reservation.busy || !reservation.lease)
    return { status: "retry" as const, problemCode: "workflow_busy" };
  return operation.run(async () => {
    try {
      const snapshot = snapshotSchema.parse(
        JSON.parse(reservation.snapshot!.content!.payloadJson),
      );
      const { plan } = snapshot;
      let proposed = reservation.proposal?.payloadJson
        ? proposalSchema.parse(JSON.parse(reservation.proposal?.payloadJson))
        : undefined;
      if (!proposed) {
        proposed = await buildMaintenanceProposal({
          models,
          need,
          snapshot,
          operationId,
          options,
          reserveModelCall,
          recordExecution: (value) => operation.recordExecution(value),
        });
        await operation.saveProposal(z.encode(proposalSchema, proposed));
      }
      let terminalDependencies: Dependency[] = [];
      if (proposed.action === "partition") {
        const request = proposed.request;
        const result = await operation.mutate(request.operationId, () =>
          kernel.maintenance.applyEpisodePartition(request, options),
        );
        terminalDependencies = protocolReferences(
          result,
          ApplyEpisodePartitionResponseSchema,
        );
      } else if (proposed.action === "journal") {
        const request = proposed.request;
        const result = await operation.mutate(request.operationId, () =>
          kernel.maintenance.commitJournal(request, options),
        );
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
          saved.producer,
          options,
          saved.progress,
          async (progress) => {
            saved.progress = [...progress];
            await operation.saveProposal(
              z.encode(proposalSchema, saved),
              resultDependencies(saved.progress),
            );
          },

          operation.mutate.bind(operation),
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
          saved.producer,
          options,
          saved.progress,
          async (progress) => {
            saved.progress = [...progress];
            await operation.saveProposal(
              z.encode(proposalSchema, saved),
              resultDependencies(saved.progress),
            );
          },

          operation.mutate.bind(operation),
        );
        await operation.complete(outcome, resultDependencies(outcome.actions));
        return outcome;
      } else if (proposed.action === "withdraw")
        await operation.mutate(operationId, () =>
          kernel.memory.withdrawJournal(
            {
              operationId,
              subjectId: need.subjectId,
              journalId: proposed.journalId,
              expectedObjectEpoch: proposed.epoch,
            },
            options,
          ),
        );
      if (
        (proposed.action === "partition" && need.scopeKind === "track") ||
        (proposed.action === "journal" && need.kind === "journal_review")
      )
        await kernel.maintenance.refreshMaintenance({ claimed: need }, options);
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
        await kernel.maintenance.refreshMaintenance({ claimed: need }, options);
        return outcome;
      }
      throw error;
    }
  });
}
