// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { z } from "zod";

export const consumerStatePolicySchema = z
  .strictObject({
    receipt_limit: z.number().int().min(1).max(65536).default(256),
    file_max_bytes: z.number().int().min(1024).max(33554432).default(1048576),
    lock_stale_ms: z.number().int().min(2000).max(300000).default(10000),
    lock_wait_ms: z.number().int().min(1).max(300000).default(10000),
  })
  .prefault({});
export type ConsumerStatePolicy = z.infer<typeof consumerStatePolicySchema>;
export const consumerStatePaths = Object.keys(
  consumerStatePolicySchema.unwrap().shape,
).map((name) => `consumer_state.${name}`);
