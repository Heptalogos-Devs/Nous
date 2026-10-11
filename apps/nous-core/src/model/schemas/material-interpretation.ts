// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { z } from "zod";
import { structuredOutputContract } from "./provider.js";
import type { JsonObject } from "@bufbuild/protobuf";
import { channelAccessSchema, type ChannelAccess } from "../input.js";
import { modelImplementations } from "../implementation.js";

const coverage = z
  .enum(["not_available", "observed", "reported", "limited", "uncertain"])
  .describe(
    "Underlying source coverage. reported means supplied only through a description/transcript; observed requires original input.",
  );
const basis_refs = z
  .array(z.string())
  .describe(
    "Invocation-local selectors from the exact supplied support catalog.",
  );
const time = {
  start_ms: z
    .number()
    .int()
    .nullable()
    .describe(
      "Start offset in milliseconds relative to the supplied media; null when unknown or not temporal.",
    ),
  end_ms: z
    .number()
    .int()
    .nullable()
    .describe(
      "End offset in milliseconds relative to the supplied media; null when unknown or not temporal.",
    ),
};
export const materialInterpretationSchema = z
  .strictObject({
    summary: z
      .strictObject({
        content: z
          .string()
          .describe(
            "Concise synthesis of the supplied material; empty when no meaningful content is available.",
          ),
        basis_keys: basis_refs,
      })
      .describe(
        "A synthesis grounded in exact source basis_refs, with uncertainty preserved in its wording.",
      ),
    coverage: z.strictObject({
      visual: coverage,
      audio: coverage,
      embedded_text: coverage.describe(
        "Visible text embedded in image/video input, not the original text source or description transport.",
      ),
      source_text: coverage.describe(
        "Coverage of original text-like source content, excluding media descriptions and transcripts.",
      ),
    }),
    observations: z.array(
      z.strictObject({
        kind: z.enum([
          "object",
          "action",
          "state",
          "scene",
          "spatial_relation",
          "sound",
          "music",
          "text",
          "speech",
          "event",
          "other",
        ]),
        content: z.string(),
        evidence_channel: z
          .enum(["visual", "audio", "source_text"])
          .describe(
            "Underlying source channel supplying this evidence, independently of its topic/kind. A state or action narrated in speech uses audio; one stated in original text uses source_text. This does not claim the narrated event was visually observed.",
          ),
        basis: z
          .enum(["direct", "reported", "inferred"])
          .describe(
            "direct requires original source input; reported preserves a claim in a supplied description/transcript; inferred is a further inference.",
          ),
        certainty: z
          .enum(["clear", "uncertain"])
          .describe(
            "Whether the observation is clear or uncertain, independently of its derivation basis.",
          ),
        ...time,
        basis_keys: basis_refs,
      }),
    ),
    mentions: z.array(
      z.strictObject({
        surface: z.string(),
        category: z.enum([
          "person",
          "organization",
          "place",
          "object",
          "concept",
          "work",
          "other",
        ]),
        role: z.string().nullable(),
        basis_keys: basis_refs,
      }),
    ),
    embedded_text: z.array(
      z.strictObject({
        text: z.string(),
        fidelity: z.enum(["verbatim", "approximate", "uncertain"]),
        ...time,
        basis_keys: basis_refs,
      }),
    ),
    source_text: z.array(
      z.strictObject({
        text: z.string(),
        fidelity: z.enum(["verbatim", "approximate", "uncertain"]),
        ...time,
        basis_keys: basis_refs,
      }),
    ),
    speech: z.array(
      z.strictObject({
        text: z.string(),
        fidelity: z.enum(["verbatim", "semantic", "uncertain"]),
        speaker_hint: z
          .string()
          .nullable()
          .describe(
            "Evidence-grounded voice label; use a neutral speaker label or null when identity is unknown. A person named by narration is not necessarily the speaker.",
          ),
        ...time,
        basis_keys: basis_refs,
      }),
    ),
    interpretations: z.array(
      z.strictObject({
        content: z.string(),
        status: z.enum(["supported", "tentative"]),
        basis_keys: basis_refs,
      }),
    ),
    uncertainties: z.array(
      z.strictObject({
        issue: z.string(),
        alternatives: z.array(z.string()),
        basis_keys: basis_refs,
      }),
    ),
  })
  .meta({
    title: "Material interpretation",
    description:
      "Observations, linguistic content and interpretations with exact source support selectors and modality coverage.",
  });
export type MaterialInterpretation = z.infer<
  typeof materialInterpretationSchema
>;
const contract = structuredOutputContract(materialInterpretationSchema);
export const materialInterpretationSchemaDigest = contract.digest;
export const materialInterpretationIdentity = modelImplementations.material;
type InterpretationInput = {
  access: ChannelAccess;
  basisKeys: ReadonlySet<string>;
  durationMs?: number;
};
export type StructuredMaterialContext = Omit<
  InterpretationInput,
  "basisKeys"
> & {
  catalog: Readonly<Record<string, { kind: string; value: string }>>;
};
export function structuredMaterialResult(
  value: unknown,
  context: StructuredMaterialContext,
) {
  const output = validateMaterialInterpretation(value, {
    ...context,
    basisKeys: new Set(Object.keys(context.catalog)),
  });
  const mapBasis = ({
    basis_keys,
    ...item
  }: {
    basis_keys: string[];
    [field: string]: unknown;
  }) => ({
    ...item,
    basis_refs: basis_keys.map((key) => context.catalog[key]!),
  });
  const payload: Record<string, unknown> = {
    ...output,
    evidence_access: channelAccessSchema.parse(context.access),
    summary: mapBasis(output.summary),
  };
  for (const group of [
    "observations",
    "mentions",
    "embedded_text",
    "source_text",
    "speech",
    "interpretations",
    "uncertainties",
  ] as const)
    payload[group] = output[group].map(mapBasis);
  return {
    text: `Evidence access: ${Object.entries(context.access)
      .map(([channel, mode]) => `${channel}=${mode}`)
      .join("; ")}\n${projectMaterialInterpretation(output)}`,
    structuredPayload: JSON.parse(JSON.stringify(payload)) as JsonObject,
  };
}

// Provider schema stays conservative; the Material owner enforces these local bounds.
function validateMaterialInterpretation(
  value: unknown,
  input: InterpretationInput,
): MaterialInterpretation {
  const output = materialInterpretationSchema.parse(value);
  const access = channelAccessSchema.parse(input.access);
  if (
    Object.entries(output.coverage).some(
      ([channel, coverageStatus]) =>
        coverageStatus === "observed" &&
        access[
          channel === "embedded_text"
            ? "visual"
            : (channel as keyof ChannelAccess)
        ] === "representation",
    ) ||
    output.observations.some(
      (item) =>
        item.basis === "direct" &&
        access[item.evidence_channel] === "representation",
    )
  )
    throw new Error(
      "Committed representation cannot establish direct source observation",
    );
  if (
    Buffer.byteLength(JSON.stringify(output)) > 262144 ||
    Buffer.byteLength(output.summary.content) > 8192
  )
    throw new Error("Structured material exceeds payload bounds");
  if (output.summary.content.trim() && !output.summary.basis_keys.length)
    throw new Error("Summary requires source support");
  const groups = [
    [output.summary],
    output.observations,
    output.mentions,
    output.embedded_text,
    output.source_text,
    output.speech,
    output.interpretations,
    output.uncertainties,
  ];
  for (const group of groups) {
    if (group.length > 64)
      throw new Error("Structured material exceeds item bound");
    for (const item of group) {
      for (const field of Object.values(item)) {
        if (
          typeof field === "string" &&
          Buffer.byteLength(field) > (item === output.summary ? 8192 : 4096)
        )
          throw new Error("Structured material exceeds field byte bound");
      }
      if (
        item.basis_keys.length > 16 ||
        new Set(item.basis_keys).size !== item.basis_keys.length
      )
        throw new Error("Structured material support count invalid");
      for (const key of item.basis_keys)
        if (!input.basisKeys.has(key))
          throw new Error("Unknown structured material support key");
      if (
        "basis" in item &&
        (item.basis === "direct" || item.basis === "reported") &&
        !item.basis_keys.length
      )
        throw new Error("Direct observation requires source support");
      if ("start_ms" in item) {
        const { start_ms: start, end_ms: end } = item;
        for (const timestamp of [start, end])
          if (
            timestamp !== null &&
            (!Number.isSafeInteger(timestamp) ||
              timestamp < 0 ||
              (input.durationMs !== undefined && timestamp > input.durationMs))
          )
            throw new Error(
              "Structured material timestamp out of source bounds",
            );
        if (start !== null && end !== null && start > end)
          throw new Error("Structured material timestamp order invalid");
      }
      if (
        "alternatives" in item &&
        (item.alternatives.length > 8 ||
          item.alternatives.some((entry) => Buffer.byteLength(entry) > 4096))
      )
        throw new Error("Structured material alternatives exceed bounds");
    }
  }
  const channels = {
    visual: access.visual !== "unavailable",
    audio: access.audio !== "unavailable",
    source_text: access.source_text !== "unavailable",
  };
  for (const observation of output.observations)
    if (
      !channels[observation.evidence_channel] ||
      output.coverage[observation.evidence_channel] === "not_available"
    )
      throw new Error(
        `Structured material invents unavailable ${observation.evidence_channel} evidence`,
      );
  if (
    !channels.source_text &&
    (output.coverage.source_text !== "not_available" ||
      output.source_text.length)
  )
    throw new Error(
      "Structured material invents unavailable source text input",
    );
  if (!channels.visual && output.coverage.embedded_text !== "not_available")
    throw new Error(
      "Structured material invents unavailable embedded visual text",
    );
  if (
    !channels.visual &&
    (output.coverage.visual !== "not_available" || output.embedded_text.length)
  )
    throw new Error("Structured material invents unavailable visual input");
  if (
    !channels.audio &&
    (output.coverage.audio !== "not_available" || output.speech.length)
  )
    throw new Error("Structured material invents unavailable audio input");
  return output;
}

function projectMaterialInterpretation(output: MaterialInterpretation): string {
  const section = (title: string, lines: string[]) =>
    lines.length ? [`${title}:`, ...lines.map((line) => `- ${line}`)] : [];
  return [
    `Summary: ${output.summary.content}`,
    `Coverage: visual=${output.coverage.visual}; audio=${output.coverage.audio}; embedded_text=${output.coverage.embedded_text}; source_text=${output.coverage.source_text}`,
    ...section(
      "Observations",
      output.observations.map(
        (item) =>
          `[${item.basis}/${item.certainty}/${item.kind}; evidence=${item.evidence_channel}] ${item.content}`,
      ),
    ),
    ...section(
      "Mentions",
      output.mentions.map(
        (item) =>
          `${item.surface} [${item.category}]${item.role === null ? "" : ` (${item.role})`}`,
      ),
    ),
    ...section(
      "Embedded text",
      output.embedded_text.map((item) => `[${item.fidelity}] ${item.text}`),
    ),
    ...section(
      "Source text",
      output.source_text.map((item) => `[${item.fidelity}] ${item.text}`),
    ),
    ...section(
      "Speech",
      output.speech.map(
        (item) =>
          `[${item.fidelity}] ${item.speaker_hint ?? "unknown speaker"}: ${item.text}`,
      ),
    ),
    ...section(
      "Interpretations",
      output.interpretations.map((item) => `[${item.status}] ${item.content}`),
    ),
    ...section(
      "Uncertainties",
      output.uncertainties.map(
        (item) =>
          `${item.issue}${item.alternatives.length ? `; alternatives: ${item.alternatives.join(" | ")}` : ""}`,
      ),
    ),
  ].join("\n");
}
