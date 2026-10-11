// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, toJson } from "@bufbuild/protobuf";
import { Code, ConnectError } from "@connectrpc/connect";
import type {
  ApplyEpisodePartitionRequest,
  CommitJournalRequest,
  MaintenanceNeed,
  MaintenancePlan,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import {
  ApplyEpisodePartitionRequestSchema,
  CommitJournalRequestSchema,
  MaintenancePlanSchema,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import {
  ProducerSignatureSchema,
  type ProducerSignature,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { type ExecutionOptions } from "../execution.js";
import { modelProducer } from "../model/producer.js";
import { type ModelRoleSnapshot } from "../model/execution/snapshot.js";
import { type ModelRuntime } from "../model/runtime.js";
import { conceptMaintenanceSchema } from "../model/schemas/concept-maintenance.js";
import { consolidationSchema } from "../model/schemas/consolidation.js";
import {
  partitionIndices,
  journalBasisKeys,
  episodePartitionSchema,
  journalSynthesisSchema,
} from "../model/schemas/longitudinal.js";

export type MaintenanceProposal =
  | { action: "partition"; request: ApplyEpisodePartitionRequest }
  | { action: "journal"; request: CommitJournalRequest }
  | {
      action: "consolidation";
      proposal: ReturnType<typeof consolidationSchema.parse>;
      producer: ProducerSignature;
    }
  | {
      action: "concept";
      proposal: ReturnType<typeof conceptMaintenanceSchema.parse>;
      producer: ProducerSignature;
    }
  | { action: "withdraw"; journalId: string; epoch: string }
  | { action: "no_change" };

/** Builds and validates one in-memory proposal from the current owner plan. */
export async function buildMaintenanceProposal({
  models,
  need,
  plan,
  modelSnapshot,
  operationId,
  options,
  reserveModelCall,
}: {
  models: ModelRuntime;
  need: MaintenanceNeed;
  plan: MaintenancePlan;
  modelSnapshot?: ModelRoleSnapshot;
  operationId: string;
  options: ExecutionOptions;
  reserveModelCall: () => void;
}): Promise<{ proposal: MaintenanceProposal }> {
  if (plan.status === "withdraw") {
    if (!plan.target)
      throw new ConnectError(
        "Journal withdrawal requires a bound target",
        Code.InvalidArgument,
      );
    return {
      proposal: {
        action: "withdraw",
        journalId: plan.target.journalId,
        epoch: plan.target.expectedEpoch.toString(),
      },
    };
  }

  if (!modelSnapshot)
    throw new ConnectError(
      "Maintenance model snapshot is unavailable",
      Code.Internal,
    );
  const planInput = JSON.stringify(toJson(MaintenancePlanSchema, plan));
  if (need.kind === "episode_resegment") {
    const result = await models.segmentEpisode(
      planInput,
      options.signal ?? undefined,
      modelSnapshot,
      reserveModelCall,
    );
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
            episode.revision?.members.map(
              (member) => member.reference?.value,
            ) ?? [],
        ),
      );
      if (plan.members.some((member) => !existing.has(member.occurrenceId)))
        throw new ConnectError(
          "No-change proposal omitted unorganized experience",
          Code.InvalidArgument,
        );
      return { proposal: { action: "no_change" } };
    }
    return {
      proposal: {
        action: "partition",
        request: create(ApplyEpisodePartitionRequestSchema, {
          operationId,
          subjectId: need.subjectId,
          expectedAuthoritySeq: plan.authoritySeq,
          sources: plan.sources,
          orderedOccurrences: plan.members.map((member) => member.occurrenceId),
          segments: segments.map((segment) => ({
            $typeName:
              "nous.wave.kernel.v1alpha1.EpisodePartitionSegment" as const,
            ...segment,
          })),
          producer: create(
            ProducerSignatureSchema,
            modelProducer(result.producerMetadata, "episode_segmentation_text"),
          ),
        }),
      },
    };
  }

  if (need.kind === "concept_maintenance") {
    if (!plan.conceptCatalog || !plan.conceptModelInputJson)
      throw new ConnectError("Missing concept catalog", Code.InvalidArgument);
    const result = await models.maintainConcepts(
      plan.conceptModelInputJson,
      options.signal ?? undefined,
      modelSnapshot,
      reserveModelCall,
    );
    return {
      proposal: {
        action: "concept",
        proposal: conceptMaintenanceSchema.parse(result.value),
        producer: create(
          ProducerSignatureSchema,
          modelProducer(result.producerMetadata, "concept_maintenance_text"),
        ),
      },
    };
  }

  if (need.kind === "memory_consolidate") {
    const result = await models.consolidate(
      planInput,
      options.signal ?? undefined,
      modelSnapshot,
      reserveModelCall,
    );
    return {
      proposal: {
        action: "consolidation",
        proposal: consolidationSchema.parse(result.value),
        producer: create(
          ProducerSignatureSchema,
          modelProducer(result.producerMetadata, "memory_consolidation_text"),
        ),
      },
    };
  }

  const result = await models.synthesizeJournal(
    planInput,
    options.signal ?? undefined,
    modelSnapshot,
    reserveModelCall,
  );
  const proposal = journalSynthesisSchema.parse(result.value);
  try {
    journalBasisKeys(proposal, new Set(plan.basis.map((entry) => entry.key)));
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
  if (proposal.action === "no_change")
    return { proposal: { action: "no_change" } };
  if (proposal.action === "withdraw") {
    if (!plan.target)
      throw new ConnectError(
        "Journal withdrawal requires an existing scope",
        Code.InvalidArgument,
      );
    return {
      proposal: {
        action: "withdraw",
        journalId: plan.target.journalId,
        epoch: plan.target.expectedEpoch.toString(),
      },
    };
  }

  const basis = new Map(plan.basis.map((entry) => [entry.key, entry.basis!]));
  return {
    proposal: {
      action: "journal",
      request: create(CommitJournalRequestSchema, {
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
        producer: create(
          ProducerSignatureSchema,
          modelProducer(result.producerMetadata, "journal_synthesis_text"),
        ),
      }),
    },
  };
}
