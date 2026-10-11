// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  openDurableOperation,
  isOperationBusyError,
  operationBusyError,
  workflowPayload,
} from "../durable-operation.js";
import { Code, ConnectError } from "@connectrpc/connect";
import { executionOptions, type ExecutionOptions } from "../execution.js";
import type { DeriveMaterialRequest } from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
import type { KernelClient } from "../kernel-client.js";
import type { ModelRuntime } from "./runtime.js";
import { type ExecutionTelemetry } from "./execution/routes.js";
import {
  type ModelProducerMetadata,
  type ModelRoleSnapshot,
  snapshotSchema as modelSnapshotSchema,
} from "./execution/snapshot.js";
import type { ModelRole } from "./roles.js";
import { modelProducer } from "./producer.js";
import { z } from "zod";
import type { DerivedRepresentation } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import {
  inspectVideoPreprocessing,
  sampleVideo,
  type SampledVideo,
  type VideoPreprocessingIdentity,
} from "./video.js";
import {
  DerivationSourceTooLarge,
  lazyDerivationSource,
} from "./derivation-source.js";
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
  }
  const source = lazyDerivationSource(
    kernel,
    request,
    region,
    artifact,
    maxSourceBytes,
    models.materialInputs.derivation_source_max_bytes,
    options,
  );
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
  let paidStageSkipReason: string | undefined;
  const paid = async (
    kind: string,
    graph: Parameters<typeof commit>[2],
    role: ModelRole,
    invoke: (snapshot: ModelRoleSnapshot) => Promise<
      | {
          text: string;
          producerMetadata: ModelProducerMetadata;
          execution: ExecutionTelemetry;
          inputAccess: ChannelAccess;
          structuredPayload?: JsonObject;
          quality?: Record<string, unknown>;
        }
      | { skipReason: string }
    >,
    quality: Record<string, unknown> = {},
    preprocessingDigest?: string | (() => Promise<string>),
    promptPath?: string,
    adapter?: string,
  ): Promise<DerivedRepresentation | undefined> => {
    paidStageSkipReason = undefined;
    const model = await models.invocations.snapshotPrompt(
      role,
      promptPath,
      adapter,
    );
    const stagePreprocessingDigest =
      typeof preprocessingDigest === "function"
        ? await preprocessingDigest()
        : preprocessingDigest;
    const key = canonicalDigest({
      subject: request.subjectId,
      graph,
      kind,
      strategy,
      producer: model.configDigest,
      preprocessingDigest: stagePreprocessingDigest,
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
    const opened = await openDurableOperation(kernel, identity, {
      execution: options,
      snapshotFactory: async () => ({
        snapshot: {
          content: workflowPayload(
            { model, preprocessingDigest: stagePreprocessingDigest },
            graph.map((input) => input.reference!),
          ),
        },
      }),
    });
    if (opened.state === "completed") {
      if (opened.outcome.purged)
        throw new ConnectError("Derivation outcome was purged", Code.NotFound);
      const outcome = z
        .union([
          z.strictObject({ representationId: z.string().uuid() }),
          z.strictObject({ skipped: z.string() }),
        ])
        .parse(JSON.parse(opened.outcome.payloadJson));
      if ("skipped" in outcome) {
        paidStageSkipReason = outcome.skipped;
        return undefined;
      }
      const representation = await kernel.material.getDerivedRepresentation(
        { subjectId: request.subjectId, id: outcome.representationId },
        options,
      );
      representations.push(representation);
      return representation;
    }
    if (opened.state === "busy") throw operationBusyError();
    const operation = opened.operation;
    return operation.run(async () => {
      let proposal = opened.proposal?.payloadJson
        ? (JSON.parse(opened.proposal.payloadJson) as {
            text: string;
            producerMetadata: ModelProducerMetadata;
            inputAccess: ChannelAccess;
            structuredPayload?: JsonObject;
            quality: Record<string, unknown>;
          })
        : undefined;
      if (!proposal) {
        const frozen = z
          .strictObject({
            model: modelSnapshotSchema,
            preprocessingDigest: z.string().optional(),
          })
          .parse(JSON.parse(opened.snapshot.content!.payloadJson));
        const result = await invoke(frozen.model);
        if ("skipReason" in result) {
          paidStageSkipReason = result.skipReason;
          await operation.checkpoint({
            outcome: workflowPayload({ skipped: result.skipReason }),
          });
          return undefined;
        }
        proposal = {
          text: result.text,
          producerMetadata: result.producerMetadata,
          inputAccess: result.inputAccess,
          structuredPayload: result.structuredPayload,
          quality: { ...quality, ...result.quality },
        };
        await operation.checkpoint({
          proposal: workflowPayload(proposal),
          executionTelemetry: result.execution,
        });
      }
      const representation = await commit(
        proposal.text,
        kind,
        graph,
        proposal.producerMetadata,
        {
          ...proposal.quality,
          input_access: channelAccessSchema.parse(proposal.inputAccess),
        },
        stagePreprocessingDigest,
        proposal.structuredPayload,
      );
      await operation.checkpoint({
        outcome: workflowPayload(
          { representationId: representation.representationId },
          [
            {
              kind: "derived_representation",
              value: representation.representationId,
            },
          ],
        ),
      });
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
  const mime = artifact.mediaType.split(";")[0]?.trim().toLowerCase() ?? "";
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
    const result = await paid(
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
      async (snapshot) => {
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
        return models.material.structure(
          segments.map((item) => `[${item.key}] ${item.text}`).join("\n"),
          signal,
          snapshot,
          {
            access: textual ? access : representedAccess(access),
            catalog,
          },
        );
      },
      {},
      canonicalDigest({
        segmentation: "description-utf8-lines-v1",
        interpretation: materialInterpretationIdentity,
      }),
    );
    if (!result) throw new Error("Description structure stage was skipped");
    return result;
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
        async (snapshot) => {
          const loaded = await source();
          return {
            ...(await models.material.describeMedia(
              loaded.content,
              mime,
              strategy === "direct_structured",
              signal,
              snapshot,
              directContext,
            )),
            quality: { source_bytes: loaded.content.length },
          };
        },
        {
          input_mode: "direct",
          modality: mime.startsWith("video/") ? "video" : "audio",
        },
        canonicalDigest({ input_mode: "direct", media_type: mime }),
        mime.startsWith("video/") && strategy !== "direct_structured"
          ? models.video.prompt
          : undefined,
        "gateway-chat-media-v1",
      );
      if (!selected) throw new Error("Direct media stage was skipped");
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
      const includeAudio = Boolean(
        models.invocations.profile("speech_transcription"),
      );
      let identityPromise: Promise<VideoPreprocessingIdentity> | undefined;
      const preprocessingIdentity = () =>
        (identityPromise ??= inspectVideoPreprocessing(
          models.video,
          models.tempRoot!,
          models.credentialEnvironments,
        ));
      let samplesPromise: Promise<SampledVideo> | undefined;
      const samples = () =>
        (samplesPromise ??= (async () => {
          const [loaded, identity] = await Promise.all([
            source(),
            preprocessingIdentity(),
          ]);
          return sampleVideo(
            loaded.content,
            models.video,
            models.tempRoot!,
            models.credentialEnvironments,
            includeAudio,
            identity,
            signal,
          );
        })());
      let transcript: string | undefined;
      let transcriptReference: { kind: string; value: string } | undefined;
      const sceneInputs = [...inputs];
      const degradation: { code: string; detail: string }[] = [];
      if (includeAudio) {
        try {
          const representation = await paid(
            "transcript",
            inputs,
            "speech_transcription",
            async (snapshot) => {
              const sampled = await samples();
              if (!sampled.audio)
                return {
                  skipReason: sampled.hasAudio
                    ? "audio_extraction_unavailable"
                    : "no_audio_track",
                };
              const result = await models.invocations.transcription(
                sampled.audio,
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
                quality: sampled.quality,
              };
            },
            {},
            () =>
              preprocessingIdentity().then(
                (value) => value.preprocessingDigest,
              ),
          );
          if (representation) {
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
          } else if (paidStageSkipReason === "audio_extraction_unavailable") {
            degradation.push({
              code: "video_audio_not_interpreted",
              detail: "Audio is not included in the scene description",
            });
          }
        } catch (error) {
          if (isOperationBusyError(error)) throw error;
          if (signal?.aborted) throw signal.reason;
          degradation.push({
            code: "video_transcription_unavailable",
            detail: "Scene description uses frames without a transcript",
          });
        }
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
      const selected = await paid(
        strategy === "direct_structured"
          ? "structured_interpretation"
          : "scene_description",
        sceneInputs,
        strategy === "direct_structured"
          ? "material_direct_structuring"
          : "material_description",
        async (snapshot) => {
          const sampled = await samples();
          return {
            ...(await models.material.describeScene(
              sampled.frames,
              transcript,
              strategy === "direct_structured",
              signal,
              snapshot,
              sceneContext,
            )),
            quality: { ...sampled.quality, has_audio: sampled.hasAudio },
          };
        },
        {},
        () =>
          preprocessingIdentity().then((value) => value.preprocessingDigest),
        strategy === "direct_structured" ? undefined : models.video.prompt,
      );
      if (!selected) throw new Error("Video scene stage was skipped");
      if (!includeAudio && selected.quality?.has_audio === true)
        degradation.push({
          code: "video_audio_not_interpreted",
          detail: "Audio is not included in the scene description",
        });
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
        async (snapshot) => {
          const loaded = await source();
          return models.material.structure(
            textual
              ? new TextDecoder("utf-8", { fatal: true }).decode(loaded.content)
              : { bytes: loaded.content, mediaType: mime },
            signal,
            snapshot,
            directContext,
          );
        },
      );
      if (!structured)
        throw new Error("Structured derivation stage was skipped");
      return {
        representations,
        selectedRepresentationId: structured.representationId,
        degradation: [],
      };
    }
    let description: DerivedRepresentation | undefined;
    if (textual) {
      const loaded = await source();
      description = await commit(
        new TextDecoder("utf-8", { fatal: true }).decode(loaded.content),
        "extracted_text",
        inputs,
        undefined,
        { input_access: directContext.access },
      );
    } else if (mime.startsWith("audio/")) {
      description = await paid(
        "transcript",
        inputs,
        "speech_transcription",
        async (snapshot) => {
          const loaded = await source();
          const result = await models.invocations.transcription(
            loaded.content,
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
      );
    } else {
      description = await paid(
        "image_description",
        inputs,
        "material_description",
        async (snapshot) => {
          const loaded = await source();
          return models.material.interpret(
            loaded.content,
            mime,
            signal,
            snapshot,
          );
        },
      );
    }
    if (!description) throw new Error("Material description stage was skipped");
    if (strategy === "describe_then_structure") {
      await structureDescription(description);
    }
    return {
      representations,
      selectedRepresentationId: representations.at(-1)!.representationId,
      degradation: [],
    };
  } catch (error) {
    if (isOperationBusyError(error)) throw error;
    if (signal?.aborted) throw error;
    if (error instanceof DerivationSourceTooLarge)
      return {
        representations,
        selectedRepresentationId: representations[0]?.representationId,
        degradation: [
          {
            code: error.degradationCode,
            detail:
              error.degradationCode === "media_source_too_large"
                ? "Media exceeds configured input byte bound"
                : "Select a bounded source region",
          },
        ],
      };
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
