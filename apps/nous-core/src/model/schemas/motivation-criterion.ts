// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { z } from "zod";
/** Fixed operation of the shared reflection binding; it cannot declare coverage. */
export const motivationCriterionSchema = z.strictObject({
  results: z
    .array(
      z.strictObject({
        key: z.string().min(1).max(256),
        truth: z.enum(["true", "false", "unknown"]),
        reason: z.string().min(1).max(8192),
        evidenceKeys: z.array(z.string().min(1).max(256)).max(64),
      }),
    )
    .max(64),
});
export type MotivationCriterionOutput = z.infer<
  typeof motivationCriterionSchema
>;
