// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { Code, ConnectError } from "@connectrpc/connect";
import { type ExecutionOptions } from "../execution.js";
import { create, toJson } from "@bufbuild/protobuf";

import {
  ApplyEpisodePartitionRequestSchema,
  CommitJournalRequestSchema,
  type MaintenanceNeed,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { ProducerSignatureSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";

import { type ModelRuntime } from "../model/runtime.js";
import { type ExecutionTelemetry } from "../model/execution/routes.js";
import {
  partitionIndices,
  journalBasisKeys,
} from "../model/schemas/longitudinal.js";

import type { ModelGenerationOutput } from "../model/schemas/contracts.js";

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
  if (plan.status === "withdraw") {
    if (!plan.target)
      throw new ConnectError(
        "Journal withdrawal requires a bound target",
        Code.InvalidArgument,
      );
    return {
      action: "withdraw",
      journalId: plan.target.journalId,
      epoch: plan.target.expectedEpoch.toString(),
    };
  }
  const model = snapshot.model!;
  if (
    need.kind === "concept_maintenance" &&
    (!plan.conceptCatalog || !plan.conceptModelInputJson)
  )
    throw new ConnectError("Missing concept catalog", Code.InvalidArgument);
  const content =
    need.kind === "concept_maintenance"
      ? plan.conceptModelInputJson!
      : JSON.stringify(snapshot.plan);
  const result = await models.invocations.generate(
    model.role,
    { content },
    {
      signal: options.signal ?? undefined,
      snapshot: model,
      beforeAttempt: reserveModelCall,
    },
  );
  await recordExecution(result.execution);
  let proposed: MaintenanceProposal;
  if (need.kind === "episode_resegment") {
    let segments;
    try {
      segments = partitionIndices(
        result.value as ModelGenerationOutput<"episode_segmentation">,
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
            $typeName: "nous.wave.kernel.v1alpha1.ApplyEpisodePartitionRequest",
            operationId,
            subjectId: need.subjectId,
            expectedAuthoritySeq: plan.authoritySeq,
            producer: modelProducer(
              result.producerMetadata,
              "episode_segmentation_text",
            ),
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
  } else if (need.kind === "concept_maintenance") {
    const proposal =
      result.value as ModelGenerationOutput<"concept_maintenance">;
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
    };
  } else if (need.kind === "memory_consolidate") {
    proposed = {
      action: "consolidation",
      proposal: result.value as ModelGenerationOutput<"memory_consolidation">,
      producer: toJson(
        ProducerSignatureSchema,
        create(
          ProducerSignatureSchema,
          modelProducer(result.producerMetadata, "memory_consolidation_text"),
        ),
      ),
    };
  } else {
    const proposal = result.value as ModelGenerationOutput<"journal_synthesis">;
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
  return proposed;
}
