// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { Code, ConnectError } from "@connectrpc/connect";
import { type ExecutionOptions } from "../execution.js";
import { create, toJson } from "@bufbuild/protobuf";

import {
  ApplyEpisodePartitionRequestSchema,
  CommitJournalRequestSchema,
  MaintenancePlanSchema,
  type MaintenanceNeed,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";

import { type ModelRuntime } from "../model/runtime.js";
import { type ExecutionTelemetry } from "../model/execution/routes.js";
import {
  partitionIndices,
  journalBasisKeys,
} from "../model/schemas/longitudinal.js";

import { modelProducer } from "../model/producer.js";

import {
  type MaintenanceSnapshot,
  type MaintenanceProposal,
} from "./records.js";

/** Builds and validates a proposal from one frozen owner plan; commits remain in workflow. */
export async function buildMaintenanceProposal({
  models,
  need,
  snapshot,
  operationId,
  options,
  reserveModelCall,
  recordExecution,
}: {
  models: ModelRuntime;
  need: MaintenanceNeed;
  snapshot: MaintenanceSnapshot;
  operationId: string;
  options: ExecutionOptions;
  reserveModelCall: () => void;
  recordExecution: (value: ExecutionTelemetry) => Promise<void>;
}): Promise<MaintenanceProposal> {
  const { plan } = snapshot;
  let proposed: MaintenanceProposal;
  if (plan.status === "withdraw") {
    if (!plan.target)
      throw new ConnectError(
        "Journal withdrawal requires a bound target",
        Code.InvalidArgument,
      );
    proposed = {
      action: "withdraw",
      journalId: plan.target.journalId,
      epoch: plan.target.expectedEpoch,
    };
  } else {
    const model = snapshot.model!;
    if (need.kind === "episode_resegment") {
      const result = await models.segmentEpisode(
        JSON.stringify(toJson(MaintenancePlanSchema, snapshot.plan)),
        options.signal ?? undefined,
        model,
        reserveModelCall,
      );
      await recordExecution(result.execution);
      let segments;
      try {
        segments = partitionIndices(
          result.value,
          plan.members.map((member) => member.key),
        );
      } catch {
        throw new ConnectError(
          "Episode partition proposal violates the member catalog",
          Code.InvalidArgument,
        );
      }
      if (!segments) {
        const existing = new Set(
          plan.episodes.flatMap(
            (episode) =>
              episode.currentRevision?.members.map(
                (member) => member.reference?.value,
              ) ?? [],
          ),
        );
        if (plan.members.some((member) => !existing.has(member.occurrenceId)))
          throw new ConnectError(
            "No-change proposal omitted unorganized experience",
            Code.InvalidArgument,
          );
      }
      proposed = segments
        ? {
            action: "partition",
            request: create(ApplyEpisodePartitionRequestSchema, {
              operationId,
              subjectId: need.subjectId,
              expectedAuthoritySeq: plan.authoritySeq,
              sources: plan.sources,
              orderedOccurrences: plan.members.map(
                (member) => member.occurrenceId,
              ),
              segments,
              producer: modelProducer(
                result.producerMetadata,
                "episode_segmentation_text",
              ),
            }),
          }
        : { action: "no_change" };
    } else if (need.kind === "concept_maintenance") {
      if (!plan.conceptCatalog || !plan.conceptModelInputJson)
        throw new ConnectError("Missing concept catalog", Code.InvalidArgument);
      const result = await models.maintainConcepts(
        plan.conceptModelInputJson,
        options.signal ?? undefined,
        model,
        reserveModelCall,
      );
      await recordExecution(result.execution);
      proposed = {
        action: "concept",
        proposal: result.value,
        producer: modelProducer(
          result.producerMetadata,
          "concept_maintenance_text",
        ),
        progress: [],
      };
    } else if (need.kind === "memory_consolidate") {
      const result = await models.consolidate(
        JSON.stringify(toJson(MaintenancePlanSchema, snapshot.plan)),
        options.signal ?? undefined,
        model,
        reserveModelCall,
      );
      await recordExecution(result.execution);
      proposed = {
        action: "consolidation",
        proposal: result.value,
        producer: modelProducer(
          result.producerMetadata,
          "memory_consolidation_text",
        ),
        progress: [],
      };
    } else {
      const result = await models.synthesizeJournal(
        JSON.stringify(toJson(MaintenancePlanSchema, snapshot.plan)),
        options.signal ?? undefined,
        model,
        reserveModelCall,
      );
      await recordExecution(result.execution);
      try {
        journalBasisKeys(
          result.value,
          new Set(plan.basis.map((entry) => entry.key)),
        );
      } catch {
        throw new ConnectError(
          "Journal proposal violates the support catalog",
          Code.InvalidArgument,
        );
      }
      if (result.value.action === "no_change" && plan.target)
        throw new ConnectError(
          "Journal revalidation requires a supported commit",
          Code.InvalidArgument,
        );
      if (result.value.action === "no_change")
        proposed = { action: "no_change" };
      else if (result.value.action === "withdraw") {
        if (!plan.target)
          throw new ConnectError(
            "Journal withdrawal requires an existing scope",
            Code.InvalidArgument,
          );
        proposed = {
          action: "withdraw",
          journalId: plan.target.journalId,
          epoch: plan.target.expectedEpoch,
        };
      } else {
        const basis = new Map(
          plan.basis.map((entry) => [entry.key, entry.basis!]),
        );
        proposed = {
          action: "journal",
          request: create(CommitJournalRequestSchema, {
            operationId,
            subjectId: need.subjectId,
            expectedAuthoritySeq: plan.authoritySeq,
            target: plan.target,
            sources: plan.sources,
            title: result.value.title ?? undefined,
            narrative: result.value.narrative,
            points: result.value.points.map((point, ordinal) => ({
              ordinal,
              role: point.role,
              text: point.text,
              basis: point.basisKeys.map((key) => basis.get(key)!),
            })),
            producer: modelProducer(
              result.producerMetadata,
              "journal_synthesis_text",
            ),
          }),
        };
      }
    }
  }
  return proposed;
}
