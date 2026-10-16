// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { z } from "zod";
import { selfInterpretationSchema } from "./self.js";
const key = z.string().min(1).max(256),
  reason = z.string().min(1).max(8192),
  basisKeys = z.array(key).min(1).max(64);
const time = z.discriminatedUnion("kind", [
  z.strictObject({ kind: z.literal("unknown") }),
  z.strictObject({ kind: z.literal("instant"), at: z.iso.datetime() }),
  z.strictObject({
    kind: z.literal("interval"),
    start: z.iso.datetime().nullable(),
    end: z.iso.datetime().nullable(),
  }),
]);
const judgment = z.strictObject({
  state: z.enum(["hypothesis", "supported", "disputed"]),
  explanation: z.string().min(1).max(8192),
  estimate: z.number().min(0).max(1).nullable(),
  estimateMethod: z.string().max(8192).nullable(),
});
const socialContent = z.discriminatedUnion("family", [
  z.strictObject({
    family: z.literal("social_relation"),
    fromKey: key,
    toKey: key,
    relationType: z.enum([
      "familiarity",
      "affinity",
      "trust",
      "closeness",
      "collaboration",
      "mentor_of",
      "member_of",
      "conflict_with",
    ]),
    degree: z
      .discriminatedUnion("kind", [
        z.strictObject({
          kind: z.literal("ordinal"),
          value: z.string().max(128),
        }),
        z.strictObject({
          kind: z.literal("affinity"),
          value: z.number().min(-1).max(1),
        }),
        z.strictObject({
          kind: z.literal("state"),
          value: z.string().max(128),
        }),
      ])
      .nullable(),
    role: z.string().max(1024).nullable(),
    symmetric: z.boolean(),
    statement: z.string().min(1).max(16384),
    coordinationExpectations: z.array(z.string().max(8192)).max(32),
    judgment,
    basisKeys,
    validTime: time,
  }),
  z.strictObject({
    family: z.literal("language_convention"),
    expression: z.string().min(1).max(4096),
    variants: z.array(z.string().max(4096)).max(64),
    caseInsensitive: z.boolean(),
    meaning: z.string().min(1).max(16384),
    referentKey: key.nullable(),
    pragmaticUse: z.string().max(8192),
    judgment,
    basisKeys,
    validTime: time,
  }),
]);
const socialAction = z.discriminatedUnion("action", [
  z.strictObject({
    action: z.literal("keep"),
    targetKey: key.nullable(),
    reason,
  }),
  z.strictObject({
    action: z.literal("adopt"),
    content: socialContent,
    reason,
  }),
  z.strictObject({
    action: z.literal("revise"),
    targetKey: key,
    content: socialContent,
    reason,
  }),
  z.strictObject({
    action: z.literal("add_evidence"),
    targetKey: key,
    basisKey: key,
    role: z.enum(["support", "counterexample", "boundary_case"]),
    reason,
  }),
]);
const memoryContent = z.strictObject({
  text: z.string().min(1).max(32768),
  semanticRole: z.string().min(1).max(128),
  epistemicClass: z.enum([
    "observed",
    "reported",
    "inferred",
    "derived",
    "narrative",
    "simulated",
  ]),
  entityKeys: z.array(key).max(64),
  basisKeys,
  validTime: time,
});
const memoryAction = z.discriminatedUnion("action", [
  z.strictObject({
    action: z.literal("keep"),
    targetKey: key.nullable(),
    reason,
  }),
  z.strictObject({
    action: z.literal("adopt"),
    content: memoryContent,
    reason,
  }),
  z.strictObject({
    action: z.literal("revise"),
    targetKey: key,
    content: memoryContent,
    reason,
  }),
]);
export const subjectReflectionSchema = z
  .strictObject({
    actions: z
      .array(
        z.discriminatedUnion("domain", [
          z.strictObject({
            domain: z.literal("self"),
            scopeKey: key.nullable(),
            proposal: selfInterpretationSchema.shape.actions.element,
          }),
          z.strictObject({
            domain: z.literal("social"),
            scopeKey: key,
            proposal: socialAction,
          }),
          z.strictObject({
            domain: z.literal("memory"),
            scopeKey: key.nullable(),
            proposal: memoryAction,
          }),
        ]),
      )
      .min(1)
      .max(128),
  })
  .meta({
    title: "Shared Subject reflection",
    description:
      "One actual observed source batch, typed key-only proposals to implemented owners; no Motivation placeholder, management or purge action.",
  });
export type SubjectReflectionProposal = z.infer<typeof subjectReflectionSchema>;
