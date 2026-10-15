// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { z } from "zod";
const key = z.string().min(1).max(256);
const basisKeys = z.array(key).min(1).max(64);
const reason = z.string().min(1).max(8192);
const validTime = z.discriminatedUnion("kind", [
  z.strictObject({ kind: z.literal("unknown") }),
  z.strictObject({ kind: z.literal("instant"), at: z.iso.datetime() }),
  z.strictObject({
    kind: z.literal("interval"),
    start: z.iso.datetime().nullable(),
    end: z.iso.datetime().nullable(),
  }),
]);
const epistemicClass = z.enum([
  "observed",
  "reported",
  "inferred",
  "derived",
  "narrative",
  "simulated",
]);
const content = z.discriminatedUnion("family", [
  z.strictObject({
    family: z.literal("self_facet"),
    kind: z.enum([
      "identity",
      "role",
      "capability_belief",
      "limitation",
      "tendency",
      "value",
      "preference",
    ]),
    statement: z.string().min(1).max(16384),
    stance: z.enum(["adopted", "tentative", "aspirational", "attributed"]),
    epistemicClass,
    scope: z.strictObject({
      description: z.string().min(1).max(8192),
      purposes: z
        .array(z.enum(["deliberation", "expression", "reflection", "worker"]))
        .max(4),
      bindToInvocation: z
        .boolean()
        .describe(
          "Bind to the supplied WorkContext, entities and conversation; false means no such structural restriction. Preserve natural-language conditions in description.",
        ),
    }),
    exceptions: z
      .array(
        z.strictObject({ statement: z.string().min(1).max(8192), basisKeys }),
      )
      .max(32),
    basisKeys,
    validTime,
  }),
  z.strictObject({
    family: z.literal("narrative_thread"),
    theme: z.string().min(1).max(1024),
    interpretation: z.string().min(1).max(16384),
    epistemicClass,
    sourceKeys: z
      .array(key)
      .min(1)
      .max(64)
      .describe(
        "Exact currentSelf[].key values for the immutable cognition revisions interpreted here. Never invent an experience.",
      ),
    basisKeys,
    validTime,
  }),
]);
export const selfInterpretationSchema = z
  .strictObject({
    actions: z
      .array(
        z.discriminatedUnion("action", [
          z.strictObject({ action: z.literal("adopt"), content, reason }),
          z.strictObject({
            action: z.literal("revise"),
            targetKey: key,
            content,
            reason,
          }),
          z.strictObject({
            action: z.literal("evidence"),
            targetKey: key,
            role: z.enum(["support", "counterexample", "boundary_case"]),
            basisKey: key,
            reason,
          }),
          z.strictObject({
            action: z.literal("keep"),
            targetKey: key.nullable(),
            reason,
          }),
        ]),
      )
      .min(1)
      .max(16),
  })
  .meta({
    title: "Self interpretation",
    description:
      "Source-backed proposals for the Subject's own current understanding, scoped choices and narratives, with explicit keep and evidence-only outcomes.",
  });
export type SelfInterpretationProposal = z.infer<
  typeof selfInterpretationSchema
>;
