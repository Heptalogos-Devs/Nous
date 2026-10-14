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
  runRecordedOperation,
  serializeOperation,
} from "../operation-results.js";
import type { ResultPayload } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_results_pb.js";
import { canonicalDigest } from "../digest.js";
import { z } from "zod";
import { attachExecutionTelemetry } from "./execution/routes.js";
import { modelProducer } from "./producer.js";

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
  const replay = async (saved: ResultPayload) => {
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
  return runRecordedOperation<
    { request: JsonValue },
    {
      memory: Awaited<ReturnType<KernelClient["memory"]["formMemory"]>>;
      degradation: never[];
    }
  >(kernel, identity, options, {
    replay,
    generate: async () => {
      const model = models.invocations.prepare("memory_formation");
      const prepared = await kernel.materialWorkflow.prepareFormation(
        {
          subjectId: r.subjectId,
          occurrenceId: r.occurrenceId,
          representationId: r.representationId,
          audioDescription: models.configuration.audio.input_mode === "direct",
          resolvedMentions: mode === "select_from_resolved_mentions",
          maxBytes: BigInt(
            models.configuration.material_inputs.formation_source_max_bytes,
          ),
        },
        options,
      );
      const source = prepared.source;
      if (!source?.reference)
        throw new ConnectError("Formation source unavailable", Code.Internal);
      const representationId =
        source.reference.kind === "derived_representation"
          ? source.reference.value
          : undefined;
      const candidates = prepared.candidates;
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
            resolvedEntityCandidates: candidates,
            aboutnessMode: mode,
          }),
        },
        {
          signal: options.signal ?? undefined,
          prepared: model,
        },
      );
      const result = {
        ...generated.value,
        producerMetadata: generated.producerMetadata,
        execution: generated.execution,
      };
      try {
        if (
          new Set(result.selectedEntityKeys).size !==
            result.selectedEntityKeys.length ||
          result.selectedEntityKeys.some(
            (key) => !candidates.some((candidate) => candidate.key === key),
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
                    candidates
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
                    locator: representationId
                      ? {
                          case: "derivedRepresentationId",
                          value: representationId,
                        }
                      : { case: "wholeOccurrence", value: true },
                  },
                },
              },
            ],
          },
        });
        return {
          proposal: { request: toJson(FormMemoryRequestSchema, request) },
          dependencies: [
            { kind: "occurrence", value: r.occurrenceId },
            ...(representationId
              ? [{ kind: "derived_representation", value: representationId }]
              : []),
            ...r.explicitTags.map((value) => ({ kind: "tag", value })),
          ],
          execution: result.execution,
        };
      } catch (error) {
        throw attachExecutionTelemetry(error, result.execution);
      }
    },
    commit: async (proposed) => {
      const request = fromJson(
        FormMemoryRequestSchema,
        proposed.request as JsonValue,
      );
      const memory = await kernel.memory.formMemory(request, options);
      return {
        value: { memory, degradation: [] },
        outcome: { revisionId: memory.revisionId },
        dependencies: [
          { kind: "memory", value: memory.memoryId },
          { kind: "memory_revision", value: memory.revisionId },
        ],
      };
    },
  });
}
