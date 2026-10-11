// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { conceptMaintenanceSchema } from "../model/schemas/concept-maintenance.js";

import { consolidationSchema } from "../model/schemas/consolidation.js";

import { Code, ConnectError } from "@connectrpc/connect";
import { type ExecutionOptions } from "../execution.js";
import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";

import {
  ApplyEpisodePartitionRequestSchema,
  CommitJournalRequestSchema,
  type MaintenanceNeed,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { ProducerSignatureSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";

import { type ModelRuntime } from "../model/runtime.js";
import { type ModelRoleSnapshot } from "../model/execution/snapshot.js";
import { type ExecutionTelemetry } from "../model/execution/routes.js";
import {
  partitionIndices,
  journalBasisKeys,
  episodePartitionSchema,
  journalSynthesisSchema,
} from "../model/schemas/longitudinal.js";

import { modelProducer } from "../model/producer.js";

import { type MaintenancePlan } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import {
  type MaintenanceSnapshot,
  type MaintenanceProposal,
} from "./records.js";

/** Builds and validates a proposal from one frozen owner plan; commits remain in workflow. */
export async function buildMaintenanceProposal({
  models,
  need,
  plan,
  snapshot,
  operationId,
  options,
  reserveModelCall,
  recordExecution,
}: {
  models: ModelRuntime;
  need: MaintenanceNeed;
  plan: MaintenancePlan;
  snapshot: MaintenanceSnapshot;
  operationId: string;
  options: ExecutionOptions;
  reserveModelCall: () => void;
  recordExecution: (value: ExecutionTelemetry) => Promise<void>;
}): Promise<MaintenanceProposal> {
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
      epoch: plan.target.expectedEpoch.toString(),
    };
  } else {
    if (need.kind === "episode_resegment") {
      const result = await models.segmentEpisode(
        JSON.stringify(snapshot.plan),
        options.signal ?? undefined,
        snapshot.model as ModelRoleSnapshot,
        reserveModelCall,
      );
      await recordExecution(result.execution);
      let segments;
      try {
        segments = partitionIndices(
          episodePartitionSchema.parse(result.value),
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
            request: toJson(ApplyEpisodePartitionRequestSchema, {
              $typeName:
                "nous.wave.kernel.v1alpha1.ApplyEpisodePartitionRequest",
              operationId,
              subjectId: need.subjectId,
              expectedAuthoritySeq: plan.authoritySeq,
              sources: plan.sources,
              orderedOccurrences: plan.members.map(
                (member) => member.occurrenceId,
              ),
              segments: segments.map((segment) => ({
                $typeName:
                  "nous.wave.kernel.v1alpha1.EpisodePartitionSegment" as const,
                ...segment,
              })),
            }),
          }
        : { action: "no_change" };
      if (proposed.action === "partition") {
        const request = fromJson(
          ApplyEpisodePartitionRequestSchema,
          proposed.request as JsonValue,
        );
        request.producer = create(
          ProducerSignatureSchema,
          modelProducer(result.producerMetadata, "episode_segmentation_text"),
        );
        proposed.request = toJson(ApplyEpisodePartitionRequestSchema, request);
      }
    } else if (need.kind === "concept_maintenance") {
      if (!plan.conceptCatalog || !plan.conceptModelInputJson)
        throw new ConnectError("Missing concept catalog", Code.InvalidArgument);
      const result = await models.maintainConcepts(
        plan.conceptModelInputJson,
        options.signal ?? undefined,
        snapshot.model as ModelRoleSnapshot,
        reserveModelCall,
      );
      await recordExecution(result.execution);
      const proposal = conceptMaintenanceSchema.parse(result.value);
      proposed = {
        action: "concept",
        proposal,
        producer: toJson(
          ProducerSignatureSchema,
          create(
            ProducerSignatureSchema,
            modelProducer(result.producerMetadata, "concept_maintenance_text"),
          ),
        ),
        progress: [],
      };
    } else if (need.kind === "memory_consolidate") {
      const result = await models.consolidate(
        JSON.stringify(snapshot.plan),
        options.signal ?? undefined,
        snapshot.model as ModelRoleSnapshot,
        reserveModelCall,
      );
      await recordExecution(result.execution);
      proposed = {
        action: "consolidation",
        proposal: consolidationSchema.parse(result.value),
        producer: toJson(
          ProducerSignatureSchema,
          create(
            ProducerSignatureSchema,
            modelProducer(result.producerMetadata, "memory_consolidation_text"),
          ),
        ),
        progress: [],
      };
    } else {
      const result = await models.synthesizeJournal(
        JSON.stringify(snapshot.plan),
        options.signal ?? undefined,
        snapshot.model as ModelRoleSnapshot,
        reserveModelCall,
      );
      await recordExecution(result.execution);
      const proposal = journalSynthesisSchema.parse(result.value);
      try {
        journalBasisKeys(
          proposal,
          new Set(plan.basis.map((entry) => entry.key)),
        );
      } catch {
        throw new ConnectError(
          "Journal proposal violates the support catalog",
          Code.InvalidArgument,
        );
      }
      if (proposal.action === "no_change" && plan.target)
        throw new ConnectError(
          "Journal revalidation requires a supported commit",
          Code.InvalidArgument,
        );
      if (proposal.action === "no_change") proposed = { action: "no_change" };
      else if (proposal.action === "withdraw") {
        if (!plan.target)
          throw new ConnectError(
            "Journal withdrawal requires an existing scope",
            Code.InvalidArgument,
          );
        proposed = {
          action: "withdraw",
          journalId: plan.target.journalId,
          epoch: plan.target.expectedEpoch.toString(),
        };
      } else {
        const basis = new Map(
          plan.basis.map((entry) => [entry.key, entry.basis!]),
        );
        proposed = {
          action: "journal",
          request: toJson(CommitJournalRequestSchema, {
            $typeName: "nous.wave.kernel.v1alpha1.CommitJournalRequest",
            operationId,
            subjectId: need.subjectId,
            expectedAuthoritySeq: plan.authoritySeq,
            target: plan.target,
            sources: plan.sources,
            title: proposal.title ?? undefined,
            narrative: proposal.narrative,
            points: proposal.points.map((point, ordinal) => ({
              $typeName: "nous.wave.v1alpha1.JournalPoint" as const,
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
