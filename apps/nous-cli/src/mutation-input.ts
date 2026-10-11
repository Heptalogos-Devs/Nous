// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { z } from "zod";
import type { NousClient } from "@nous-wave/client";
import type { CliEnvironment } from "./runtime.js";
import { CliError } from "./agent.js";
const referenceText = z.string().min(1).max(2048);
export const revisionBasisRole = z.enum([
  "direct",
  "interpretation",
  "contextual",
]);
export const epistemicRelation = z.enum([
  "supports",
  "contradicts",
  "corroborates",
  "weakens",
  "corrects",
  "counterexample",
  "inferred_from",
]);
export const revisionBasisInput = z.strictObject({
  ref: referenceText,
  role: revisionBasisRole.default("direct"),
  epistemic_relation: epistemicRelation.optional(),
});
const useBasis = z
  .object({
    use_event: z.object({ consumer: referenceText, event: z.uuid() }).strict(),
  })
  .strict();
const content = z
  .object({
    label: z.string().min(1).max(512),
    description: z.string().max(8192).default(""),
    kind_hint: z.string().max(128).optional(),
  })
  .strict();
export const reviseInput = content.extend({ target: referenceText }).strict();
export const mergeInput = z
  .object({
    survivor: referenceText,
    retired: z.array(referenceText).min(1).max(32),
    basis: z.array(revisionBasisInput).min(1).max(256),
  })
  .strict();
export const splitInput = z
  .object({
    parent: referenceText,
    children: z.array(content).min(2).max(32),
    basis: z.array(revisionBasisInput).min(1).max(256),
  })
  .strict();
export const associationInput = z
  .object({
    from: referenceText,
    to: referenceText,
    relation: z.string().min(1).max(128),
    polarity: z.enum(["positive", "negative", "neutral"]).default("neutral"),
    basis_class: z
      .enum([
        "host_explicit",
        "source_evidence",
        "cognitive_derivation",
        "meaningful_use",
        "derived_structure",
      ])
      .default("host_explicit"),
    basis: z
      .array(z.union([revisionBasisInput, useBasis]))
      .min(1)
      .max(256),
  })
  .strict();
export const attachmentInput = z
  .object({
    basis: z
      .array(z.union([revisionBasisInput, useBasis]))
      .min(1)
      .max(256),
  })
  .strict();
export async function tagTarget(env: CliEnvironment, text: string) {
  const ref = await env.resolveReference(text, "tag");
  const tag = await env.client.concepts.getTag({
    subjectId: env.subjectId,
    id: ref.value,
  });
  return { tagId: ref.value, expectedRevisionId: tag.currentRevisionId };
}
type RevisionBasis = NonNullable<
  Parameters<NousClient["concepts"]["mergeTags"]>[0]["basis"]
>[number];
export async function revisionBasis(
  env: CliEnvironment,
  entry: z.infer<typeof revisionBasisInput>,
): Promise<RevisionBasis> {
  const target = await env.resolveReference(entry.ref);
  if (target.kind === "occurrence")
    return {
      basis: {
        case: "evidence",
        value: {
          occurrenceId: target.value,
          locator: { case: "wholeOccurrence", value: true },
          basisRole: entry.role,
          epistemicRelation: entry.epistemic_relation,
        },
      },
    };
  if (
    ![
      "memory_revision",
      "cognitive_schema_revision",
      "episode_revision",
      "journal_revision",
    ].includes(target.kind)
  )
    throw new CliError(
      "REFERENCE_TYPE_MISMATCH",
      "Basis requires an occurrence or exact cognition revision",
    );
  return {
    basis: {
      case: "cognitionDependency",
      value: {
        targetRevision: target,
        basisRole: entry.role,
        epistemicRelation: entry.epistemic_relation,
      },
    },
  };
}
export async function associationBasis(
  env: CliEnvironment,
  entry: z.infer<typeof revisionBasisInput> | z.infer<typeof useBasis>,
) {
  if ("use_event" in entry)
    return {
      basis: {
        case: "useEvent" as const,
        value: {
          subjectId: env.subjectId,
          consumerRef: entry.use_event.consumer,
          eventId: entry.use_event.event,
        },
      },
    };
  return {
    basis: {
      case: "revision" as const,
      value: await revisionBasis(env, entry),
    },
  };
}
export function tagContent(value: z.infer<typeof content>) {
  return {
    label: value.label,
    description: value.description,
    kindHint: value.kind_hint,
  };
}
