// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { jsonSchema } from "ai";
import { z } from "zod";
import { canonicalDigest } from "../../digest.js";

export type StructuredOutputContract<T> = {
  providerSchema: Record<string, unknown>;
  digest: string;
  sdkSchema: ReturnType<typeof jsonSchema<T>>;
};

export function structuredOutputContract<S extends z.ZodType>(
  owner: S,
): StructuredOutputContract<z.output<S>> {
  const providerSchema = z.toJSONSchema(owner, { target: "draft-7" });
  return {
    providerSchema,
    digest: canonicalDigest(providerSchema),
    sdkSchema: jsonSchema<z.output<S>>(providerSchema, {
      validate(value) {
        const result = owner.safeParse(value);
        return result.success
          ? { success: true, value: result.data }
          : { success: false, error: result.error };
      },
    }),
  };
}
