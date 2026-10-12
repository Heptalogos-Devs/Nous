// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { Code, ConnectError } from "@connectrpc/connect";
import { executionOptions, type ExecutionOptions } from "../execution.js";
import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { FormMemoryRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { type FormationRequest } from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
import type { KernelClient } from "../kernel-client.js";
import type { ModelRuntime } from "./runtime.js";
import {
  startOperation,
  serializeOperation,
  workflowPayload,
} from "../durable-operation.js";
import type { WorkflowPayload } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/workflow_envelope_pb.js";
import { type ModelRoleSnapshot } from "./execution/snapshot.js";
import { canonicalDigest } from "../digest.js";
import { z } from "zod";
import { modelProducer } from "./producer.js";

type FormationSnapshot = {
  model: ModelRoleSnapshot;
  sourceMaxBytes: number;
  representationId?: string;
  candidates: { key: string; surface: string; entityRef: string }[];
};
export async function formObservation(
  kernel: KernelClient,
  models: ModelRuntime,
  r: FormationRequest,
  options: ExecutionOptions,
) {
  const calls = executionOptions(kernel.execution.opportunity, options);
  return serializeOperation(
    kernel,
    `memory:${r.subjectId.toLowerCase()}:${r.operationId.toLowerCase()}`,
    calls.signal,
    () => formObservationOnce(kernel, models, r, calls),
  );
}

async function formObservationOnce(
  kernel: KernelClient,
  models: ModelRuntime,
  r: FormationRequest,
  options: ExecutionOptions,
) {
  const calls = executionOptions(kernel.execution.opportunity, options);
  options = calls;
  if (
    !z.string().uuid().safeParse(r.operationId).success ||
    r.operationId === "00000000-0000-0000-0000-000000000000"
  )
    throw new ConnectError(
      "Caller-stable operation_id is required",
      Code.InvalidArgument,
    );
  r.operationId = r.operationId.toLowerCase();
  r.subjectId = r.subjectId.toLowerCase();
  r.occurrenceId = r.occurrenceId.toLowerCase();
  if (r.representationId) r.representationId = r.representationId.toLowerCase();
  const mode = r.aboutnessMode || "select_from_resolved_mentions";
  if (
    !["explicit", "select_from_resolved_mentions", "none"].includes(mode) ||
    (mode !== "explicit" && r.explicitAboutness.length)
  )
    throw new ConnectError("Invalid aboutness policy", Code.InvalidArgument);
  if (
    new Set(r.explicitAboutness).size !== r.explicitAboutness.length ||
    r.explicitAboutness.length > 128
  )
    throw new ConnectError(
      "Duplicate/oversized aboutness",
      Code.InvalidArgument,
    );
  if (
    r.explicitTags.length > 128 ||
    r.explicitTags.some((tag) => !z.string().uuid().safeParse(tag).success)
  )
    throw new ConnectError(
      "Invalid explicit Tag identities",
      Code.InvalidArgument,
    );
  r.explicitTags = [
    ...new Set(r.explicitTags.map((tag) => tag.toLowerCase())),
  ].sort();
  const identity = {
    subjectId: r.subjectId,
    owner: "memory",
    operationKey: r.operationId,
    semanticDigest: canonicalDigest({
      subject: r.subjectId,
      occurrence: r.occurrenceId,
      representation: r.representationId,
      mode,
      aboutness: [...r.explicitAboutness].sort(),
      explicitTags: r.explicitTags,
    }),
  };
  const replay = async (saved: WorkflowPayload) => {
    if (saved.purged)
      throw new ConnectError("Formation outcome was purged", Code.NotFound);
    const outcome = JSON.parse(saved.payloadJson) as { revisionId: string };
    const memory = await kernel.memory.getMemoryRevision(
      { subjectId: r.subjectId, id: outcome.revisionId },
      options,
    );
    return {
      memory,
      degradation: [],
    };
  };
  const previous = await kernel.modelWorkflow.readOperation(identity, options);
  if (previous.outcome) return replay(previous.outcome);
  let snapshot: Parameters<typeof startOperation>[2] | undefined =
    previous.snapshot;
  if (!snapshot) {
    const model = models.invocations.snapshot("memory_formation");
    const occurrence = await kernel.material.getOccurrence(
      { subjectId: r.subjectId, id: r.occurrenceId },
      options,
    );
    let representationId = r.representationId;
    if (!representationId && occurrence.artifactId) {
      const artifact = await kernel.material.getArtifact(
        { subjectId: r.subjectId, id: occurrence.artifactId },
        options,
      );
      const mime = artifact.mediaType.split(";")[0]!.trim().toLowerCase();
      const kind = mime.startsWith("image/")
        ? "image_description"
        : mime.startsWith("audio/")
          ? models.audio.input_mode === "direct"
            ? "audio_description"
            : "transcript"
          : mime.startsWith("video/")
            ? "scene_description"
            : "extracted_text";
      const available = await kernel.material.listDerivedRepresentations(
        {
          subjectId: r.subjectId,
          artifactId: occurrence.artifactId,
          kind,
          limit: 1,
        },
        options,
      );
      representationId = available.items[0]?.representationId;
    }
    const candidates =
      mode === "select_from_resolved_mentions"
        ? (
            await kernel.materialWorkflow.getResolvedMentions(
              { subjectId: r.subjectId, occurrenceId: r.occurrenceId },
              options,
            )
          ).candidates.map((candidate) => ({
            key: candidate.key,
            surface: candidate.surface,
            entityRef: candidate.entityRef,
          }))
        : [];
    snapshot = {
      content: workflowPayload(
        {
          model,
          representationId,
          candidates,
          sourceMaxBytes: models.materialInputs.formation_source_max_bytes,
        },
        [
          { kind: "occurrence", value: r.occurrenceId },
          ...(representationId
            ? [{ kind: "derived_representation", value: representationId }]
            : []),
          ...r.explicitTags.map((value) => ({ kind: "tag", value })),
        ],
      ),
    };
  }
  const operation = await startOperation(kernel, identity, snapshot, options);
  const record = operation.record;
  if (record.outcome) return replay(record.outcome);
  return operation.run(async () => {
    let proposed = record.proposal?.payloadJson
      ? (JSON.parse(record.proposal.payloadJson) as { request: JsonValue })
      : undefined;
    if (!proposed) {
      const frozenSnapshot = JSON.parse(
        record.snapshot!.content!.payloadJson,
      ) as FormationSnapshot;
      const source = await kernel.material.materializeEvidence(
        {
          subjectId: r.subjectId,
          reference: frozenSnapshot.representationId
            ? {
                kind: "derived_representation",
                value: frozenSnapshot.representationId,
              }
            : { kind: "occurrence", value: r.occurrenceId },
          maxBytes: BigInt(frozenSnapshot.sourceMaxBytes),
        },
        options,
      );
      if (source.partial)
        throw new ConnectError(
          "Formation source is too large; select a bounded representation",
          Code.ResourceExhausted,
        );
      const mime = source.mediaType.split(";")[0]!.toLowerCase();
      if (!mime.startsWith("text/") && mime !== "application/json")
        throw new ConnectError(
          "material_representation_required",
          Code.FailedPrecondition,
        );
      const text = new TextDecoder("utf-8", { fatal: true }).decode(
        source.content,
      );
      const generated = await models.invocations.generate(
        "memory_formation",
        {
          content: JSON.stringify({
            evidenceText: text,
            resolvedEntityCandidates: frozenSnapshot.candidates,
            aboutnessMode: mode,
          }),
        },
        {
          signal: options.signal ?? undefined,
          snapshot: frozenSnapshot.model,
        },
      );
      const result = {
        ...generated.value,
        producerMetadata: generated.producerMetadata,
        execution: generated.execution,
      };
      await operation.recordExecution(result.execution);
      if (
        new Set(result.selectedEntityKeys).size !==
          result.selectedEntityKeys.length ||
        result.selectedEntityKeys.some(
          (key) =>
            !frozenSnapshot.candidates.some(
              (candidate) => candidate.key === key,
            ),
        )
      )
        throw new ConnectError(
          "Model selected an invalid entity candidate",
          Code.FailedPrecondition,
        );
      const aboutness =
        mode === "explicit"
          ? r.explicitAboutness
          : mode === "none"
            ? []
            : [
                ...new Set(
                  frozenSnapshot.candidates
                    .filter((candidate) =>
                      result.selectedEntityKeys.includes(candidate.key),
                    )
                    .map((candidate) => candidate.entityRef),
                ),
              ];
      const request = create(FormMemoryRequestSchema, {
        operationId: r.operationId,
        subjectId: r.subjectId,
        cognitiveRole: "declarative",
        input: {
          formationMode: "grounded",
          groundingOccurrenceId: r.occurrenceId,
          semanticRole: result.semanticRole,
          text: result.text,
          title: result.title ?? undefined,
          epistemicClass: "derived",
          aboutness,
          tags: r.explicitTags,
          producer: modelProducer(
            result.producerMetadata,
            "memory_formation_text",
          ),
          basis: [
            {
              basis: {
                case: "evidence",
                value: {
                  occurrenceId: r.occurrenceId,
                  basisRole: "interpretation",
                  locator: frozenSnapshot.representationId
                    ? {
                        case: "derivedRepresentationId",
                        value: frozenSnapshot.representationId,
                      }
                    : { case: "wholeOccurrence", value: true },
                },
              },
            },
          ],
        },
      });
      proposed = {
        request: toJson(FormMemoryRequestSchema, request),
      };
      await operation.saveProposal(proposed);
    }
    const request = fromJson(
      FormMemoryRequestSchema,
      proposed.request as JsonValue,
    );
    const memory = await kernel.memory.formMemory(request, options);
    await operation.complete({ revisionId: memory.revisionId }, [
      { kind: "memory", value: memory.memoryId },
      { kind: "memory_revision", value: memory.revisionId },
    ]);
    return {
      memory,
      degradation: [],
    };
  });
}
