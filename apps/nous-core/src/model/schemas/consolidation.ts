import { domainTemporalExtentSchema as time } from "./temporal.js";
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { z } from "zod";
const key = z
  .string()
  .min(1)
  .max(256)
  .describe("Exact invocation-local key from the relevant supplied catalog.");

const memory = z.strictObject({
  cognitiveRole: z.enum([
    "experiential",
    "declarative",
    "procedural_experience",
  ]),
  formationMode: z.enum(["grounded", "synthesized"]),
  groundingMemberKey: key
    .describe(
      "Exact members[].key for grounded formation, or null for synthesis.",
    )
    .nullable(),
  semanticRole: z.string().min(1).max(128),
  text: z.string().min(1).max(32768),
  title: z.string().max(8192).nullable(),
  basisKeys: z
    .array(
      key.describe(
        "Exact basis[].key; member and candidate keys are not support selectors. A revision cannot depend on its own target object; the owner records parent lineage automatically.",
      ),
    )
    .min(1)
    .max(128),
  entityKeys: z
    .array(
      key.describe(
        "Exact entities[].key, not an entity reference or display name.",
      ),
    )
    .max(128),
  validTime: time.describe(
    "World-valid time of this claim, distinct from publication, observation and formation time.",
  ),
  epistemicClass: z.enum([
    "observed",
    "reported",
    "derived",
    "inferred",
    "narrative",
    "simulated",
  ]),
});
const schema = z.strictObject({
  title: z.string().max(8192).nullable(),
  structuralClaim: z.string().min(1).max(16384),
  applicability: z
    .string()
    .min(1)
    .max(16384)
    .describe(
      "Conditions under which the generalized structural claim applies.",
    ),
  boundaryDefinition: z
    .string()
    .min(1)
    .max(16384)
    .describe("Limits and exceptions of the proposed generalization."),
  formationKind: z.enum(["explicit_import", "synthesized"]),
  entityKeys: z
    .array(
      key.describe(
        "Exact entities[].key, not an entity reference or display name.",
      ),
    )
    .max(128),
  validTime: time.describe(
    "World-valid time of this claim, distinct from publication, observation and formation time.",
  ),
  evidence: z
    .array(
      z.strictObject({
        role: z.enum(["support", "counterexample", "boundary_case"]),
        basisKey: key.describe(
          "Exact basis[].key grounding this evidence role. A revised Schema cannot cite its own target object as evidence; parent lineage is recorded automatically.",
        ),
      }),
    )
    .min(1)
    .max(128),
});
const intent = z.enum(["correct", "rephrase", "reinterpret"]);
const endpoint = z.discriminatedUnion("kind", [
  z.strictObject({
    kind: z.literal("candidate"),
    key: key.describe(
      "Exact candidates[].key for a Memory revision; strong Memory relations do not accept CognitiveSchema or source context endpoints.",
    ),
  }),
  z.strictObject({
    kind: z.literal("action"),
    index: z.number().int().min(0).max(15),
  }),
]);
const consolidationActionSchema = z.discriminatedUnion("action", [
  z.strictObject({
    action: z.literal("skip"),
    reason: z.string().min(1).max(8192),
  }),
  z.strictObject({ action: z.literal("create_memory"), content: memory }),
  z.strictObject({
    action: z.literal("revise_memory"),
    targetKey: key.describe(
      "Exact candidates[].key for the current object being revised.",
    ),
    intent,
    content: memory,
  }),
  z.strictObject({ action: z.literal("create_schema"), content: schema }),
  z.strictObject({
    action: z.literal("revise_schema"),
    targetKey: key.describe(
      "Exact candidates[].key for the current object being revised.",
    ),
    intent,
    content: schema,
  }),
  z.strictObject({
    action: z.literal("link_relation"),
    from: endpoint,
    to: endpoint,
    relation: z.enum([
      "derived_from",
      "contradicts",
      "temporal_successor",
      "replaces_basis",
      "elaborates",
    ]),
  }),
]);
export const consolidationSchema = z
  .strictObject({
    actions: z
      .array(consolidationActionSchema)
      .min(1)
      .max(16)
      .describe(
        "Ordered proposals; relation action endpoints use zero-based indices of earlier create_memory or revise_memory actions in this array.",
      ),
  })
  .meta({
    title: "Memory consolidation",
    description:
      "Source-grounded proposals to create or revise Memory and CognitiveSchema and relate legitimate objects.",
  });
export type ConsolidationProposal = z.infer<typeof consolidationSchema>;
