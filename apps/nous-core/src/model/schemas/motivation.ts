import {
  domainTemporalExtentSchema,
  temporalExtentSchema as time,
} from "./temporal.js";
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { z } from "zod";
const key = z.string().min(1).max(256);
type CriterionBlueprint =
  | {
      kind: "eventWitness";
      eventType: string;
      target: string | null;
      activity: string | null;
    }
  | { kind: "ownerState"; targetKey: string; state: string }
  | {
      kind: "measurement";
      name: string;
      unit: string;
      comparison: "lt" | "le" | "eq" | "ge" | "gt";
      value: number;
    }
  | {
      kind: "relation";
      targetKey: string;
      relationType: string;
      degree: string | null;
    }
  | { kind: "semantic"; question: string; materialKeys: string[] }
  | { kind: "all" | "any"; items: CriterionBlueprint[] }
  | { kind: "not"; item: CriterionBlueprint };
export const criterionBlueprintSchema: z.ZodType<CriterionBlueprint> = z.lazy(
  () =>
    z.discriminatedUnion("kind", [
      z.strictObject({
        kind: z.literal("eventWitness"),
        eventType: key,
        target: key.nullable(),
        activity: key.nullable(),
      }),
      z.strictObject({
        kind: z.literal("ownerState"),
        targetKey: key,
        state: z.enum([
          "accepted",
          "withdrawn",
          "valid",
          "revalidation_required",
          "normal",
          "suppressed",
          "active",
          "paused",
          "ended",
        ]),
      }),
      z.strictObject({
        kind: z.literal("measurement"),
        name: key,
        unit: key,
        comparison: z.enum(["lt", "le", "eq", "ge", "gt"]),
        value: z.number(),
      }),
      z.strictObject({
        kind: z.literal("relation"),
        targetKey: key,
        relationType: key,
        degree: key.nullable(),
      }),
      z.strictObject({
        kind: z.literal("semantic"),
        question: z.string().min(1).max(8192),
        materialKeys: z.array(key).min(1).max(64),
      }),
      z.strictObject({
        kind: z.literal("all"),
        items: z.array(criterionBlueprintSchema).min(1).max(64),
      }),
      z.strictObject({
        kind: z.literal("any"),
        items: z.array(criterionBlueprintSchema).min(1).max(64),
      }),
      z.strictObject({
        kind: z.literal("not"),
        item: criterionBlueprintSchema,
      }),
    ]),
);

const content = z.strictObject({
  family: z.enum(["desire", "concern", "desired_condition"]),
  matterKey: key,
  statement: z.string().min(1).max(16384),
  basisKeys: z.array(key).min(1).max(64),
  validTime: domainTemporalExtentSchema,
  links: z
    .array(
      z.strictObject({
        relation: z.enum([
          "refines",
          "prerequisite",
          "conflicts",
          "long_term_priority",
          "supports",
          "replaces",
        ]),
        targetKey: key,
      }),
    )
    .max(64),
  condition: z
    .strictObject({
      expression: criterionBlueprintSchema,
      temporalForm: z.enum(["achieve", "cease", "maintain", "avoid"]),
      timeDomainRef: key,
      window: time,
      coverageRequirement: z.string().min(1).max(8192),
      holdSeconds: z.number().int().nonnegative().max(31536000).nullable(),
    })
    .nullable(),
});
const reason = z.string().min(1).max(8192);
export const motivationActionSchema = z.discriminatedUnion("action", [
  z.strictObject({
    action: z.literal("keep"),
    targetKey: key.nullable(),
    reason,
  }),
  z.strictObject({ action: z.literal("adopt"), content, reason }),
  z.strictObject({
    action: z.literal("revise"),
    targetKey: key,
    content,
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
export type MotivationAction = z.infer<typeof motivationActionSchema>;
export type { CriterionBlueprint };
