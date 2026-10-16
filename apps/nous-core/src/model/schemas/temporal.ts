// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { z } from "zod";
const domain = z
  .string()
  .min(1)
  .max(256)
  .default("time:real")
  .describe(
    "The actual supplied time domain of the validity claim. time:real is physical UTC; a world domain must come from the captured source or semantic clock. Different domains have no implicit comparison or mapping.",
  );
export const temporalExtentSchema = z.discriminatedUnion("kind", [
  z.strictObject({ kind: z.literal("unknown") }),
  z.strictObject({ kind: z.literal("instant"), at: z.iso.datetime() }),
  z.strictObject({
    kind: z.literal("interval"),
    start: z.iso.datetime().nullable(),
    end: z.iso.datetime().nullable(),
  }),
]);
export const domainTemporalExtentSchema = z.discriminatedUnion("kind", [
  z.strictObject({ kind: z.literal("unknown"), timeDomainRef: domain }),
  z.strictObject({
    kind: z.literal("instant"),
    at: z.iso.datetime(),
    timeDomainRef: domain,
  }),
  z.strictObject({
    kind: z.literal("interval"),
    start: z.iso.datetime().nullable(),
    end: z.iso.datetime().nullable(),
    timeDomainRef: domain,
  }),
]);
