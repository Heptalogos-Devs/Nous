// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { UserContent } from "ai";
import { ModelInvocations } from "./invocations.js";
import type { ModelRole } from "./roles.js";
import { type PreparedModelRole } from "./execution/binding.js";
import {
  structuredMaterialResult,
  type StructuredMaterialContext,
} from "./schemas/material-interpretation.js";
import { descriptionSchema as interpretationSchema } from "./schemas/description.js";
import { originalMediaAccess, type InputModalities } from "./input.js";

/** Owns model input construction and Material output interpretation. */
export class MaterialInterpretation {
  constructor(
    private readonly invocations: ModelInvocations,
    private readonly videoPrompt: string,
  ) {}
  private async interpretInput(
    role: ModelRole,
    request: Parameters<ModelInvocations["generate"]>[1],
    structured: boolean,
    available: (input: InputModalities) => StructuredMaterialContext,
    signal?: AbortSignal,
    prepared?: PreparedModelRole,
    prompt?: { role: ModelRole; path: string },
  ) {
    const result = await this.invocations.generate(role, request, {
      signal,
      prepared,
      prompt,
      interpretOutput: (value, input) => ({
        ...(structured
          ? structuredMaterialResult(value, available(input))
          : interpretationSchema.parse({ text: value })),
        inputAccess: available(input).access,
      }),
    });
    return {
      ...result.value,
      producerMetadata: result.producerMetadata,
      execution: result.execution,
    };
  }
  async interpret(
    bytes: Uint8Array,
    mediaType: string,
    signal?: AbortSignal,
    fixed?: PreparedModelRole,
  ) {
    if (!mediaType.startsWith("image/"))
      throw new Error("Image description requires image material");
    return this.interpretInput(
      "material_description",
      {
        content: [{ type: "file", data: bytes, mediaType }],
        context: (actual) => ({ evidence_access: originalMediaAccess(actual) }),
      },
      false,
      (actual) => ({ access: originalMediaAccess(actual), catalog: {} }),
      signal,
      fixed,
    );
  }
  async structure(
    input: string | { bytes: Uint8Array; mediaType: string },
    signal?: AbortSignal,
    fixed?: PreparedModelRole,
    context?: StructuredMaterialContext,
  ) {
    if (!context)
      throw new Error(
        "Structured material requires a stable formation basis catalog",
      );
    if (typeof input !== "string" && !input.mediaType.startsWith("image/"))
      throw new Error(
        "Use direct media interpretation for audio or video; file structuring accepts images",
      );
    const role =
      typeof input === "string"
        ? "material_structuring"
        : "material_direct_structuring";
    const available = (actual: InputModalities): StructuredMaterialContext =>
      typeof input === "string"
        ? context
        : { ...context, access: originalMediaAccess(actual) };
    const content: UserContent =
      typeof input === "string"
        ? JSON.stringify({
            evidence_text: input,
            evidence_kind:
              context.access.source_text === "original"
                ? "original_text"
                : "committed_representation",
            evidence_access: context.access,
            basis_catalog: Object.keys(context.catalog),
            modalities: {
              visual: context.access.visual !== "unavailable",
              audio: context.access.audio !== "unavailable",
              source_text: context.access.source_text !== "unavailable",
            },
          })
        : [
            {
              type: "file" as const,
              data: input.bytes,
              mediaType: input.mediaType,
            },
          ];
    if (Array.isArray(content))
      content.unshift({
        type: "text",
        text: JSON.stringify({ basis_catalog: Object.keys(context.catalog) }),
      });
    return this.interpretInput(
      role,
      {
        content,
        context:
          typeof input === "string"
            ? undefined
            : (actual) => ({
                evidence_access: available(actual).access,
                basis_catalog: Object.keys(context.catalog),
              }),
      },
      true,
      available,
      signal,
      fixed,
    );
  }
  async describeMedia(
    bytes: Uint8Array,
    mediaType: string,
    structured: boolean,
    signal?: AbortSignal,
    fixed?: PreparedModelRole,
    context?: StructuredMaterialContext,
  ) {
    if (structured && !context)
      throw new Error(
        "Structured material requires a stable formation basis catalog",
      );
    const available = (actual: InputModalities): StructuredMaterialContext => ({
      ...context!,
      access: originalMediaAccess(actual),
    });
    return this.interpretInput(
      structured ? "material_direct_structuring" : "material_description",
      {
        content: `Input media type: ${mediaType}. The attached material is the input evidence. Formation basis catalog: ${JSON.stringify(Object.keys(context?.catalog ?? {}))}.`,
        media: { bytes, mediaType },
        context: (actual) => ({
          evidence_access: originalMediaAccess(actual),
          basis_catalog: Object.keys(context?.catalog ?? {}),
        }),
      },
      structured,
      available,
      signal,
      fixed,
    );
  }
  async describeScene(
    frames: { bytes: Uint8Array; timestamp: number }[],
    transcript: string | undefined,
    structured: boolean,
    signal?: AbortSignal,
    fixed?: PreparedModelRole,
    context?: StructuredMaterialContext,
  ) {
    if (structured && !context)
      throw new Error(
        "Structured material requires a stable formation basis catalog",
      );
    const available = (actual: InputModalities): StructuredMaterialContext => ({
      ...context!,
      access: {
        visual: actual.visual ? "original" : "unavailable",
        audio: transcript ? "representation" : "unavailable",
        source_text: "unavailable",
      },
    });
    const content = [
      {
        type: "text" as const,
        text: JSON.stringify({
          sampled_timestamps: frames.map((f) => f.timestamp),
          transcript,
          basis_catalog: Object.keys(context?.catalog ?? {}),
        }),
      },
      ...frames.flatMap((frame, index) => [
        {
          type: "text" as const,
          text: JSON.stringify({
            frame_index: index,
            timestamp_seconds: frame.timestamp,
            applies_to: "the immediately following image",
          }),
        },
        {
          type: "file" as const,
          data: frame.bytes,
          mediaType: "image/jpeg",
        },
      ]),
    ];
    return this.interpretInput(
      structured ? "material_direct_structuring" : "material_description",
      {
        content,
        context: (actual) => ({
          evidence_access: available(actual).access,
          basis_catalog: Object.keys(context?.catalog ?? {}),
        }),
      },
      structured,
      available,
      signal,
      fixed,
      fixed || structured
        ? undefined
        : { role: "material_description", path: this.videoPrompt },
    );
  }
}
