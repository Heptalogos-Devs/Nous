// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { reserveOperation, workflowPayload } from "../durable-operation.js";
import { Code, ConnectError } from "@connectrpc/connect";
import { executionOptions, type ExecutionOptions } from "../execution.js";
import type { DeriveMaterialRequest } from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
import type { KernelClient } from "../kernel-client.js";
import type { ModelRuntime } from "./runtime.js";
import { type ExecutionTelemetry } from "./execution/routes.js";
import {
  type ModelProducerMetadata,
  type ModelRoleSnapshot,
} from "./execution/snapshot.js";
import type { ModelRole } from "./roles.js";
import { modelProducer } from "./producer.js";
import { z } from "zod";
import type { DerivedRepresentation } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { sampleVideo } from "./video.js";
import { canonicalDigest } from "../digest.js";
import {
  materialInterpretationSchemaDigest,
  materialInterpretationIdentity,
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
  const strategy = request.strategy ?? models.materialStrategy;
  if (
    ![
      "description_only",
      "direct_structured",
      "describe_then_structure",
    ].includes(strategy)
  )
    throw new ConnectError(
      "Unknown material strategy. Use description_only, direct_structured or describe_then_structure; video frame sampling uses input_mode in the video configuration object (config describe video).",
      Code.InvalidArgument,
    );
  if (
    request.target &&
    !["description", "structured", "automatic"].includes(request.target)
  )
    throw new ConnectError("Unknown derivation target", Code.InvalidArgument);
  if (
    (request.target === "description" && strategy !== "description_only") ||
    (request.target === "structured" && strategy === "description_only")
  )
    throw new ConnectError(
      "Derivation target conflicts with strategy",
      Code.InvalidArgument,
    );
  const calls = executionOptions(kernel.execution.opportunity, options);
  options = calls;
  const region = await kernel.material.getSourceRegion(
    { subjectId: request.subjectId, id: request.sourceRegionId },
    options,
  );
  const artifact = await kernel.material.getArtifact(
    { subjectId: request.subjectId, id: region.artifactId },
    options,
  );
  const isVideo = artifact.mediaType.toLowerCase().startsWith("video/");
  const isAudio = artifact.mediaType.toLowerCase().startsWith("audio/");
  const maxSourceBytes = isAudio
    ? models.audio.max_source_bytes
    : models.video.max_source_bytes;
  let source;
  if ((isVideo || isAudio) && region.coordinateKind === "whole_artifact") {
    if (artifact.byteLength > BigInt(maxSourceBytes))
      return {
        representations: [],
        degradation: [
          {
            code: "media_source_too_large",
            detail: "Media exceeds configured input byte bound",
          },
        ],
      };
    const chunks: Uint8Array[] = [];
    let length = 0;
    for await (const chunk of kernel.artifacts.downloadArtifact(
      { subjectId: request.subjectId, artifactId: region.artifactId },
      options,
    )) {
      length += chunk.content.length;
      if (length > maxSourceBytes)
        throw new Error("Media stream exceeds configured byte bound");
      chunks.push(chunk.content);
    }
    source = {
      content: Buffer.concat(chunks),
      mediaType: artifact.mediaType,
      partial: false,
    };
  } else
    source = await kernel.material.materializeEvidence(
      {
        subjectId: request.subjectId,
        reference: { kind: "source_region", value: request.sourceRegionId },
        maxBytes: BigInt(models.materialInputs.derivation_source_max_bytes),
      },
      options,
    );
  if (source.partial)
    return {
      representations: [],
      degradation: [
        {
          code: "derivation_source_too_large",
          detail: "Select a bounded source region",
        },
      ],
    };
  const representations: DerivedRepresentation[] = [];
  const commit = async (
    text: string,
    kind: string,
    inputs: {
      ordinal: number;
      reference: { kind: string; value: string };
      role: string;
    }[],
    producerMetadata?: ModelProducerMetadata,
    quality: Record<string, unknown> = {},
    preprocessingDigest?: string,
    structuredPayload?: JsonObject,
  ) => {
    const representation = await kernel.materialWorkflow.commitInterpretation(
      {
        subjectId: request.subjectId,
        text,
        kind,
        inputs,
        strategy,
        structuredPayload,
        quality: JSON.parse(
          JSON.stringify({
            ...quality,
          }),
        ) as import("@bufbuild/protobuf").JsonObject,
        supersedes:
          representations.length === 0 ? request.supersedes : undefined,
        producer: {
          ...(producerMetadata
            ? modelProducer(
                producerMetadata,
                kind === "image_description" || kind === "scene_description"
                  ? "image_interpretation"
                  : kind === "transcript"
                    ? "speech_transcription"
                    : kind === "extracted_text"
                      ? "document_extraction"
                      : "text_interpretation",
              )
            : {
                providerClass: "deterministic",
                operation: "document_extraction",
                implementation: "verified-utf8-decoding-v1",
              }),
          preprocessingIdentity: `${producerMetadata ? (producerMetadata.promptId ?? "standard-audio-transcription") : "verified-utf8"}/${strategy}`,
          preprocessingRevision: producerMetadata?.promptDigest ?? "1",
          configDigest:
            preprocessingDigest || structuredPayload
              ? canonicalDigest({
                  role: producerMetadata?.configDigest,
                  preprocessing: preprocessingDigest,
                  interpretation: structuredPayload
                    ? materialInterpretationIdentity
                    : undefined,
                })
              : (producerMetadata?.configDigest ?? "utf8-fatal-v1"),
        },
      },
      options,
    );
    representations.push(representation);
    return representation;
  };
  const paid = async (
    kind: string,
    graph: Parameters<typeof commit>[2],
    role: ModelRole,
    invoke: (snapshot: ModelRoleSnapshot) => Promise<{
      text: string;
      producerMetadata: ModelProducerMetadata;
      execution: ExecutionTelemetry;
      inputAccess: ChannelAccess;
      structuredPayload?: JsonObject;
    }>,
    quality: Record<string, unknown> = {},
    preprocessingDigest?: string,
    promptPath?: string,
    adapter?: string,
  ) => {
    const model = await models.invocations.snapshotPrompt(
      role,
      promptPath,
      adapter,
    );
    const key = canonicalDigest({
      subject: request.subjectId,
      graph,
      kind,
      strategy,
      producer: model.configDigest,
      preprocessingDigest,
      supersedes: representations.length === 0 ? request.supersedes : undefined,
      outputSchemaDigest:
        kind === "structured_interpretation"
          ? materialInterpretationSchemaDigest
          : undefined,
      interpretation: materialInterpretationIdentity,
    });
    const identity = {
      subjectId: request.subjectId,
      owner: "material",
      operationKey: key,
      semanticDigest: key,
    };
    const operation = await reserveOperation(
      kernel,
      identity,
      {
        content: workflowPayload(
          model,
          graph.map((input) => input.reference!),
        ),
      },
      options,
    );
    const reservation = operation.record;
    if (reservation.outcome) {
      if (reservation.outcome.purged)
        throw new ConnectError("Derivation outcome was purged", Code.NotFound);
      const outcome = z
        .strictObject({ representationId: z.string().uuid() })
        .parse(JSON.parse(reservation.outcome.payloadJson));
      const representation = await kernel.material.getDerivedRepresentation(
        { subjectId: request.subjectId, id: outcome.representationId },
        options,
      );
      representations.push(representation);
      return representation;
    }
    return operation.run(async () => {
      let proposal = reservation.proposal?.payloadJson
        ? (JSON.parse(reservation.proposal?.payloadJson) as {
            text: string;
            producerMetadata: ModelProducerMetadata;
            execution: ExecutionTelemetry;
            inputAccess: ChannelAccess;
            structuredPayload?: JsonObject;
          })
        : undefined;
      if (!proposal) {
        proposal = await invoke(
          JSON.parse(
            reservation.snapshot!.content!.payloadJson,
          ) as ModelRoleSnapshot,
        );
        const { execution, ...saved } = proposal;
        await operation.saveProposal(saved, [], execution);
      }
      const representation = await commit(
        proposal.text,
        kind,
        graph,
        proposal.producerMetadata,
        {
          ...quality,
          input_access: channelAccessSchema.parse(proposal.inputAccess),
        },
        preprocessingDigest,
        proposal.structuredPayload,
      );
      await operation.complete(
        { representationId: representation.representationId },
        [
          {
            kind: "derived_representation",
            value: representation.representationId,
          },
        ],
      );
      return representation;
    });
  };
  const inputs = [
    {
      ordinal: 0,
      reference: { kind: "source_region", value: request.sourceRegionId },
      role: "source",
    },
  ];
  const mime = source.mediaType.split(";")[0]?.trim().toLowerCase() ?? "";
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
    catalog: { S000: { kind: "source_region", value: request.sourceRegionId } },
  };
  const signal = options.signal ?? undefined;
  const structureDescription = async (description: DerivedRepresentation) => {
    const access = channelAccessSchema.parse(description.quality?.input_access);
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
        interpretation: materialInterpretationIdentity,
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
    models.audio.input_mode === "transcription" &&
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
      (mime.startsWith("video/") && models.video.input_mode === "direct") ||
      (mime.startsWith("audio/") && models.audio.input_mode === "direct")
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
        (snapshot) =>
          models.material.describeMedia(
            source.content,
            mime,
            strategy === "direct_structured",
            signal,
            snapshot,
            directContext,
          ),
        {
          input_mode: "direct",
          source_bytes: source.content.length,
          modality: mime.startsWith("video/") ? "video" : "audio",
        },
        canonicalDigest({ input_mode: "direct", media_type: mime }),
        mime.startsWith("video/") && strategy !== "direct_structured"
          ? models.video.prompt
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
    if (mime.startsWith("video/")) {
      if (!models.tempRoot)
        throw new Error("Resolved TempRoot is required for video sampling");
      const samples = await sampleVideo(
        source.content,
        models.video,
        models.tempRoot,
        models.credentialEnvironments,
        Boolean(models.invocations.profile("speech_transcription")),
        signal,
      );
      let transcript: string | undefined;
      let transcriptReference: { kind: string; value: string } | undefined;
      const sceneInputs = [...inputs];
      const degradation: { code: string; detail: string }[] = [];
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
            samples.quality,
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
        } catch {
          if (signal?.aborted) throw signal.reason;
          degradation.push({
            code: "video_transcription_unavailable",
            detail: "Scene description uses frames without a transcript",
          });
        }
      } else if (samples.hasAudio)
        degradation.push({
          code: "video_audio_not_interpreted",
          detail: "Audio is not included in the scene description",
        });
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
      const selected = await paid(
        strategy === "direct_structured"
          ? "structured_interpretation"
          : "scene_description",
        sceneInputs,
        strategy === "direct_structured"
          ? "material_direct_structuring"
          : "material_description",
        (snapshot) =>
          models.material.describeScene(
            samples.frames,
            transcript,
            strategy === "direct_structured",
            signal,
            snapshot,
            sceneContext,
          ),
        samples.quality,
        samples.preprocessingDigest,
        strategy === "direct_structured" ? undefined : models.video.prompt,
      );
      if (strategy === "describe_then_structure") {
        try {
          await structureDescription(selected);
        } catch {
          if (signal?.aborted) throw signal.reason;
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
        (snapshot) =>
          models.material.structure(
            textual
              ? new TextDecoder("utf-8", { fatal: true }).decode(source.content)
              : { bytes: source.content, mediaType: mime },
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
          new TextDecoder("utf-8", { fatal: true }).decode(source.content),
          "extracted_text",
          inputs,
          undefined,
          { input_access: directContext.access },
        )
      : mime.startsWith("audio/")
        ? await paid(
            "transcript",
            inputs,
            "speech_transcription",
            async (snapshot) => {
              const result = await models.invocations.transcription(
                source.content,
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
            (snapshot) =>
              models.material.interpret(source.content, mime, signal, snapshot),
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
    if (signal?.aborted) throw error;
    return {
      representations,
      selectedRepresentationId: representations[0]?.representationId,
      degradation: [
        {
          code: representations.length
            ? "material_structuring_failed"
            : "material_derivation_failed",
          detail:
            error instanceof Error ? error.message : "Derivation unavailable",
        },
      ],
    };
  }
}
