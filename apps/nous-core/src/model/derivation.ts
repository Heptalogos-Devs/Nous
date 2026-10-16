// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  runRecordedOperation,
  serializeOperation,
} from "../operation-results.js";
import { Code, ConnectError } from "@connectrpc/connect";
import { executionOptions, type ExecutionOptions } from "../execution.js";
import {
  create,
  fromJson,
  toJson,
  type MessageInitShape,
} from "@bufbuild/protobuf";
import { CommitInterpretationRequestSchema } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js";
import { ProducerSignatureSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { DeriveMaterialRequest } from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
import type { KernelClient } from "../kernel-client.js";
import type { ModelRuntime } from "./runtime.js";
import { type ExecutionTelemetry } from "./execution/routes.js";
import {
  type ModelProducerMetadata,
  type PreparedModelRole,
} from "./execution/binding.js";
import type { ModelRole } from "./roles.js";
import { modelProducer } from "./producer.js";
import { z } from "zod";
import type { DerivedRepresentation } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { sampleVideo, videoSamplingIdentity } from "./video.js";
import { canonicalDigest } from "../digest.js";
import {
  materialInterpretationSchemaDigest,
  type StructuredMaterialContext,
} from "./schemas/material-interpretation.js";
import type { JsonObject } from "@bufbuild/protobuf";
import {
  channelAccessSchema,
  representedAccess,
  type ChannelAccess,
} from "./input.js";

export async function deriveMaterial(
  kernel: KernelClient,
  models: ModelRuntime,
  request: DeriveMaterialRequest,
  options: ExecutionOptions,
) {
  request.subjectId = request.subjectId.toLowerCase();
  request.sourceRegionId = request.sourceRegionId.toLowerCase();
  if (request.supersedes) request.supersedes = request.supersedes.toLowerCase();
  const calls = executionOptions(kernel.execution.opportunity, options);
  return serializeOperation(
    kernel,
    `derivation:${request.subjectId}:${request.sourceRegionId}`,
    calls.signal,
    () => deriveMaterialOnce(kernel, models, request, calls),
  );
}

async function deriveMaterialOnce(
  kernel: KernelClient,
  models: ModelRuntime,
  request: DeriveMaterialRequest,
  options: ExecutionOptions,
) {
  const strategy = request.strategy ?? models.configuration.material_strategy;
  if (
    ![
      "description_only",
      "direct_structured",
      "describe_then_structure",
    ].includes(strategy)
  )
    throw new ConnectError(
      "Unknown material strategy: use description_only, direct_structured or describe_then_structure.",
      Code.InvalidArgument,
    );
  const calls = executionOptions(kernel.execution.opportunity, options);
  options = calls;
  const sourceRead = new AbortController();
  const sourceStream = kernel.artifacts.materializeEvidence(
    {
      subjectId: request.subjectId,
      reference: { kind: "source_region", value: request.sourceRegionId },
      maxBytes: BigInt(
        Math.max(
          models.configuration.audio.max_source_bytes,
          models.configuration.video.max_source_bytes,
          models.configuration.material_inputs.derivation_source_max_bytes,
        ),
      ),
    },
    { ...options, signal: AbortSignal.any([calls.signal, sourceRead.signal]) },
  );
  const stream = sourceStream[Symbol.asyncIterator]();
  try {
    const first = await stream.next();
    if (first.done || first.value.part.case !== "header")
      throw new ConnectError(
        "Material source header is missing",
        Code.Internal,
      );
    const header = first.value.part.value;
    const isVideo =
      header.mediaType.toLowerCase().startsWith("video/") ||
      header.mediaType.toLowerCase() === "image/gif";
    const isAudio = header.mediaType.toLowerCase().startsWith("audio/");
    const maxSourceBytes = isAudio
      ? models.configuration.audio.max_source_bytes
      : isVideo
        ? models.configuration.video.max_source_bytes
        : models.configuration.material_inputs.derivation_source_max_bytes;
    let source:
      { content: Uint8Array; mediaType: string; partial: boolean } | undefined;
    let sourceFailure: string | undefined;
    const loadSource = async () => {
      if (source) return source;
      if (
        header.partial ||
        header.rangeEnd - header.rangeStart > BigInt(maxSourceBytes)
      ) {
        sourceFailure =
          isAudio || isVideo
            ? "media_source_too_large"
            : "derivation_source_too_large";
        throw new ConnectError(
          "Select a source within its configured input byte bound",
          Code.ResourceExhausted,
        );
      }
      const chunks: Uint8Array[] = [];
      let length = 0;
      for (
        let next = await stream.next();
        !next.done;
        next = await stream.next()
      ) {
        if (next.value.part.case !== "content")
          throw new ConnectError(
            "Unexpected material source header",
            Code.Internal,
          );
        const chunk = next.value.part.value;
        length += chunk.length;
        if (length > maxSourceBytes)
          throw new ConnectError(
            "Material stream exceeds configured byte bound",
            Code.ResourceExhausted,
          );
        chunks.push(chunk);
      }
      source = {
        content: Buffer.concat(chunks),
        mediaType: header.mediaType,
        partial: false,
      };
      return source;
    };
    const representations: DerivedRepresentation[] = [];
    type Interpretation = MessageInitShape<
      typeof CommitInterpretationRequestSchema
    >;
    type Inputs = NonNullable<Interpretation["inputs"]>;
    const content = (
      input: Interpretation,
      metadata?: ModelProducerMetadata,
      preprocessingDigest?: string,
    ) =>
      create(CommitInterpretationRequestSchema, {
        ...input,
        subjectId: request.subjectId,
        strategy,
        supersedes:
          representations.length === 0 ? request.supersedes : undefined,
        producer: create(ProducerSignatureSchema, {
          ...(metadata
            ? modelProducer(
                metadata,
                input.kind === "image_description" ||
                  input.kind === "scene_description"
                  ? "image_interpretation"
                  : input.kind === "transcript"
                    ? "speech_transcription"
                    : input.kind === "extracted_text"
                      ? "document_extraction"
                      : "text_interpretation",
              )
            : {
                providerClass: "deterministic",
                operation: "document_extraction",
                implementation: "verified-utf8-decoding-v1",
              }),
          preprocessingIdentity: `${metadata ? (metadata.promptId ?? "standard-audio-transcription") : "verified-utf8"}/${strategy}`,
          preprocessingRevision: metadata?.promptDigest ?? "1",
          configDigest:
            preprocessingDigest || input.structuredPayload
              ? canonicalDigest({
                  role: metadata?.configDigest,
                  preprocessing: preprocessingDigest,
                  ffmpegVersion: input.quality?.ffmpeg_version,
                })
              : (metadata?.configDigest ?? "utf8-fatal-v1"),
        }),
      });
    const commit = async (input: Interpretation) => {
      const value = await kernel.materialWorkflow.commitInterpretation(
        input,
        options,
      );
      representations.push(value);
      return value;
    };
    const paidIdentity = (
      kind: string,
      graph: Inputs,
      producer: string,
      preprocessingDigest?: string,
      supersedes?: string,
    ) => {
      const key = canonicalDigest({
        subject: request.subjectId,
        graph,
        kind,
        strategy,
        producer,
        preprocessingDigest,
        supersedes,
        outputSchemaDigest:
          kind === "structured_interpretation"
            ? materialInterpretationSchemaDigest
            : undefined,
      });
      return {
        subjectId: request.subjectId,
        owner: "material",
        operationKey: key,
        semanticDigest: key,
      };
    };
    const appendTranscripts = async (graph: Inputs) => {
      for (const input of graph) {
        if (
          input.role === "transcript" &&
          !representations.some(
            (item) => item.representationId === input.reference!.value,
          )
        )
          representations.push(
            await kernel.material.getDerivedRepresentation(
              { subjectId: request.subjectId, id: input.reference!.value },
              options,
            ),
          );
      }
    };
    const paid = async (
      kind: string,
      graph: Inputs,
      role: ModelRole,
      invoke: (snapshot: PreparedModelRole) => Promise<{
        text: string;
        producerMetadata: ModelProducerMetadata;
        execution: ExecutionTelemetry;
        inputAccess: ChannelAccess;
        structuredPayload?: JsonObject;
        inputs?: Inputs;
        quality?: Record<string, unknown>;
      }>,
      quality: Record<string, unknown> = {},
      preprocessingDigest?: string,
      promptPath?: string,
      adapter?: string,
    ) => {
      const model = await models.invocations.preparePrompt(
        role,
        promptPath,
        adapter,
      );
      const identity = paidIdentity(
        kind,
        graph,
        model.configDigest,
        preprocessingDigest,
        representations.length === 0 ? request.supersedes : undefined,
      );
      return runRecordedOperation(kernel, identity, options, {
        replay: async (outcomeRecord) => {
          if (outcomeRecord.purged)
            throw new ConnectError(
              "Derivation outcome was purged",
              Code.NotFound,
            );
          const outcome = z
            .strictObject({ representationId: z.string().uuid() })
            .parse(JSON.parse(outcomeRecord.payloadJson));
          const representation = await kernel.material.getDerivedRepresentation(
            { subjectId: request.subjectId, id: outcome.representationId },
            options,
          );
          await appendTranscripts(representation.inputs);
          representations.push(representation);
          return representation;
        },
        generate: async () => {
          try {
            const produced = await invoke(model);
            const input = content(
              {
                kind,
                text: produced.text,
                inputs: produced.inputs ?? graph,
                quality: {
                  ...quality,
                  ...produced.quality,
                  input_access: produced.inputAccess,
                } as JsonObject,
                structuredPayload: produced.structuredPayload,
                operationIdentity: identity,
              },
              produced.producerMetadata,
              preprocessingDigest,
            );
            return {
              proposal: toJson(CommitInterpretationRequestSchema, input),
              execution: produced.execution,
              dependencies: input.inputs.map((item) => item.reference!),
            };
          } catch (error) {
            models.invocations.allowDegradation(error, options.signal, role);
            throw error;
          }
        },
        commit: async (proposal) => {
          const input = fromJson(CommitInterpretationRequestSchema, proposal);
          await appendTranscripts(input.inputs);
          const representation = await commit(input);
          return {
            value: representation,
            outcome: { representationId: representation.representationId },
            dependencies: [
              {
                kind: "derived_representation",
                value: representation.representationId,
              },
            ],
          };
        },
      });
    };
    const inputs = [
      {
        ordinal: 0,
        reference: { kind: "source_region", value: request.sourceRegionId },
        role: "source",
      },
    ];
    const mime = header.mediaType.split(";")[0]?.trim().toLowerCase() ?? "";
    const textual = mime.startsWith("text/") || mime === "application/json";
    const directContext: StructuredMaterialContext = {
      access: {
        visual:
          mime.startsWith("image/") || mime.startsWith("video/")
            ? "original"
            : "unavailable",
        audio: mime.startsWith("audio/") ? "original" : "unavailable",
        source_text: textual ? "original" : "unavailable",
      },
      catalog: {
        S000: { kind: "source_region", value: request.sourceRegionId },
      },
    };
    const signal = options.signal ?? undefined;
    const structureDescription = async (description: DerivedRepresentation) => {
      const access = channelAccessSchema.parse(
        description.quality?.input_access,
      );
      const { segments } = await kernel.materialWorkflow.segmentDescription(
        { subjectId: request.subjectId, id: description.representationId },
        options,
      );
      const catalog: Record<string, { kind: string; value: string }> = {};
      for (const segment of segments) {
        if (!segment.reference)
          throw new Error("Description segment reference is missing");
        catalog[segment.key] = {
          kind: segment.reference.kind,
          value: segment.reference.value,
        };
      }
      return paid(
        "structured_interpretation",
        [
          {
            ordinal: 0,
            reference: {
              kind: "derived_representation",
              value: description.representationId,
            },
            role: "description",
          },
        ],
        "material_structuring",
        (snapshot) =>
          models.material.structure(
            segments.map((item) => `[${item.key}] ${item.text}`).join("\n"),
            signal,
            snapshot,
            {
              access: textual ? access : representedAccess(access),
              catalog,
            },
          ),
        {},
        canonicalDigest({
          segmentation: "description-utf8-lines-v1",
        }),
      );
    };
    if (
      !textual &&
      !mime.startsWith("image/") &&
      !mime.startsWith("audio/") &&
      !mime.startsWith("video/")
    )
      return {
        representations: [],
        degradation: [
          {
            code: "material_modality_unsupported",
            detail: "No standard derivation adapter for this media type",
          },
        ],
      };
    if (
      mime.startsWith("audio/") &&
      models.configuration.audio.input_mode === "transcription" &&
      strategy === "direct_structured"
    )
      return {
        representations: [],
        degradation: [
          {
            code: "audio_direct_structured_unsupported",
            detail: "Speech transcription must precede textual structuring",
          },
        ],
      };
    try {
      if (
        (mime.startsWith("video/") &&
          models.configuration.video.input_mode === "direct") ||
        (mime.startsWith("audio/") &&
          models.configuration.audio.input_mode === "direct")
      ) {
        const selected = await paid(
          strategy === "direct_structured"
            ? "structured_interpretation"
            : mime.startsWith("video/")
              ? "scene_description"
              : "audio_description",
          inputs,
          strategy === "direct_structured"
            ? "material_direct_structuring"
            : "material_description",
          async (snapshot) =>
            models.material.describeMedia(
              (await loadSource()).content,
              mime,
              strategy === "direct_structured",
              signal,
              snapshot,
              directContext,
            ),
          {
            input_mode: "direct",
            modality: mime.startsWith("video/") ? "video" : "audio",
          },
          canonicalDigest({ input_mode: "direct", media_type: mime }),
          mime.startsWith("video/") && strategy !== "direct_structured"
            ? models.configuration.video.prompt
            : undefined,
          "gateway-chat-media-v1",
        );
        if (strategy === "describe_then_structure")
          await structureDescription(selected);
        return {
          representations,
          selectedRepresentationId: representations.at(-1)!.representationId,
          degradation: [],
        };
      }
      if (mime.startsWith("video/") || mime === "image/gif") {
        const tempRoot = models.tempRoot;
        if (!tempRoot)
          throw new Error("Resolved TempRoot is required for video sampling");
        const kind =
          strategy === "direct_structured"
            ? "structured_interpretation"
            : "scene_description";
        const role =
          strategy === "direct_structured"
            ? "material_direct_structuring"
            : "material_description";
        const transcriptionConfig =
          mime !== "image/gif" &&
          models.configuration.roles.speech_transcription
            ? models.invocations.prepare("speech_transcription").configDigest
            : null;
        const preprocessingDigest = canonicalDigest({
          sampling: videoSamplingIdentity(models.configuration.video, mime),
          transcriptionConfig,
        });
        const selected = await paid(
          kind,
          inputs,
          role,
          async (prepared) => {
            const degradation: { code: string; detail: string }[] = [];
            const samples = await sampleVideo(
              (await loadSource()).content,
              models.configuration.video,
              tempRoot,
              models.credentialEnvironments,
              mime !== "image/gif" &&
                Boolean(models.configuration.roles.speech_transcription),
              signal,
              mime,
            );
            let transcript: string | undefined;
            let transcriptReference:
              { kind: string; value: string } | undefined;
            const sceneInputs = [...inputs];
            if (samples.audio) {
              try {
                const representation = await paid(
                  "transcript",
                  inputs,
                  "speech_transcription",
                  async (snapshot) => {
                    const result = await models.invocations.transcription(
                      samples.audio!,
                      signal,
                      snapshot,
                    );
                    return {
                      text: result.value,
                      producerMetadata: result.producerMetadata,
                      execution: result.execution,
                      inputAccess: {
                        visual: "unavailable",
                        audio: "original",
                        source_text: "unavailable",
                      },
                    };
                  },
                  {
                    ...samples.quality,
                    preprocessing_digest: samples.preprocessingDigest,
                  },
                  samples.preprocessingDigest,
                );
                transcript = representation.text;
                transcriptReference = {
                  kind: "derived_representation",
                  value: representation.representationId,
                };
                sceneInputs.push({
                  ordinal: 1,
                  reference: {
                    kind: "derived_representation",
                    value: representation.representationId,
                  },
                  role: "transcript",
                });
              } catch (error) {
                models.invocations.allowDegradation(
                  error,
                  signal,
                  "speech_transcription",
                );
                degradation.push({
                  code: "video_transcription_unavailable",
                  detail: "Scene description uses frames without a transcript",
                });
              }
            } else if (samples.hasAudio) {
              models.invocations.allowDegradation(
                new Error(
                  samples.audioProblem ?? "Video audio is not interpreted",
                ),
                signal,
                "speech_transcription",
              );
              degradation.push({
                code: "video_audio_not_interpreted",
                detail: "Audio is not included in the scene description",
              });
            }
            const sceneContext: StructuredMaterialContext = {
              ...directContext,
              access: {
                ...directContext.access,
                audio: transcript ? "representation" : "unavailable",
              },
              catalog: {
                ...directContext.catalog,
                ...(transcriptReference ? { T001: transcriptReference } : {}),
              },
            };
            const interpreted = await models.material.describeScene(
              samples.frames,
              transcript,
              strategy === "direct_structured",
              signal,
              prepared,
              sceneContext,
            );
            return {
              ...interpreted,
              inputs: sceneInputs,
              quality: {
                ...samples.quality,
                preprocessing_digest: samples.preprocessingDigest,
                video_degradation: degradation,
                transcription_config_digest: transcriptionConfig,
              },
            };
          },
          {},
          preprocessingDigest,
          strategy === "direct_structured"
            ? undefined
            : models.configuration.video.prompt,
        );
        const degradation = (selected.quality?.video_degradation ?? []) as {
          code: string;
          detail: string;
        }[];
        if (strategy === "describe_then_structure") {
          try {
            await structureDescription(selected);
          } catch (error) {
            models.invocations.allowDegradation(
              error,
              signal,
              "material_structuring",
            );
            degradation.push({
              code: "material_structuring_failed",
              detail: "The committed scene description is retained",
            });
          }
        }
        return {
          representations,
          selectedRepresentationId: representations.at(-1)!.representationId,
          degradation,
        };
      }
      if (strategy === "direct_structured") {
        const structured = await paid(
          "structured_interpretation",
          inputs,
          textual ? "material_structuring" : "material_direct_structuring",
          async (snapshot) =>
            models.material.structure(
              textual
                ? new TextDecoder("utf-8", { fatal: true }).decode(
                    (await loadSource()).content,
                  )
                : { bytes: (await loadSource()).content, mediaType: mime },
              signal,
              snapshot,
              directContext,
            ),
        );
        return {
          representations,
          selectedRepresentationId: structured.representationId,
          degradation: [],
        };
      }
      const description = textual
        ? await commit(
            content({
              text: new TextDecoder("utf-8", { fatal: true }).decode(
                (await loadSource()).content,
              ),
              kind: "extracted_text",
              inputs,
              quality: { input_access: directContext.access },
            }),
          )
        : mime.startsWith("audio/")
          ? await paid(
              "transcript",
              inputs,
              "speech_transcription",
              async (snapshot) => {
                const result = await models.invocations.transcription(
                  (await loadSource()).content,
                  signal,
                  snapshot,
                );
                return {
                  text: result.value,
                  producerMetadata: result.producerMetadata,
                  execution: result.execution,
                  inputAccess: {
                    visual: "unavailable",
                    audio: "original",
                    source_text: "unavailable",
                  },
                };
              },
            )
          : await paid(
              "image_description",
              inputs,
              "material_description",
              async (snapshot) =>
                models.material.interpret(
                  (await loadSource()).content,
                  mime,
                  signal,
                  snapshot,
                ),
            );
      if (strategy === "describe_then_structure") {
        await structureDescription(description);
      }
      return {
        representations,
        selectedRepresentationId: representations.at(-1)!.representationId,
        degradation: [],
      };
    } catch (error) {
      models.invocations.allowDegradation(error, signal);
      return {
        representations,
        selectedRepresentationId: representations[0]?.representationId,
        degradation: [
          {
            code:
              sourceFailure ??
              (representations.length
                ? "material_structuring_failed"
                : "material_derivation_failed"),
            detail:
              error instanceof Error ? error.message : "Derivation unavailable",
          },
        ],
      };
    }
  } finally {
    sourceRead.abort();
    // Connect exposes next only: observe cancellation to release its RPC deadline timer.
    await stream.next().catch(() => {});
  }
}
