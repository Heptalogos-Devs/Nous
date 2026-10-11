// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { z } from "zod";
import { remoteEndpointSchema } from "../remote-endpoint.js";

export const profileNameSchema = z.string().min(1).max(512);
export const executionTimeoutSchema = z.number().int().min(1).max(300_000);
export const gatewaySchema = z.strictObject({
  base_url: remoteEndpointSchema,
  credential_env: z.string().regex(/^[A-Za-z_][A-Za-z0-9_]*$/),
  enabled: z.boolean().default(true),
  request_timeout_ms: executionTimeoutSchema.default(30_000),
});
const embeddingSchema = z.strictObject({
  dimension: z.number().int().min(1).max(8192),
  max_batch_size: z.number().int().min(1).max(64).default(64),
  weights_revision: profileNameSchema,
  task: profileNameSchema,
  input_representation: profileNameSchema,
  preprocessing_identity: profileNameSchema,
  preprocessing_revision: profileNameSchema,
  normalization: profileNameSchema,
  output_semantics: profileNameSchema,
});
const reasoningLevels = [
  "provider-default",
  "none",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
] as const;
const modelBaseSchema = z.strictObject({
  gateway: profileNameSchema,
  model: z.string().max(512).default(""),
  capabilities: z
    .array(
      z.enum([
        "text",
        "image_input",
        "audio_input",
        "video_input",
        "structured_output",
        "embedding",
        "speech_transcription",
        "rerank",
      ]),
    )
    .min(1)
    .overwrite((values) => [...new Set(values)].sort()),
  model_revision: profileNameSchema.optional(),
  reasoning_levels: z
    .array(z.enum(reasoningLevels))
    .min(1)
    .max(7)
    .default(["provider-default"])
    .overwrite((values) => [...new Set(values)].sort()),
});
// The protocol/embedding contract is native Zod, including the exported JSON Schema.
export const modelSchema = z.discriminatedUnion("protocol", [
  modelBaseSchema.extend({
    protocol: z.literal("openai-embeddings"),
    embedding: embeddingSchema,
  }),
  modelBaseSchema.extend({
    protocol: z.enum([
      "openai-chat",
      "openai-responses",
      "openai-audio-transcription",
      "rerank-v1",
    ]),
    embedding: z.never().optional(),
  }),
]);
export const generationTokensSchema = z
  .number()
  .int()
  .min(1)
  .max(65_536)
  .default(4096);
export const executionProfileSchema = z.strictObject({
  model: profileNameSchema,
  reasoning: z.enum(reasoningLevels).default("provider-default"),
  temperature: z.number().min(0).max(2).optional(),
  top_p: z.number().min(0).max(1).optional(),
  max_output_tokens: generationTokensSchema.unwrap().optional(),
  timeout_ms: executionTimeoutSchema.optional(),
  provider_options: z
    .record(z.string().max(64), z.record(z.string().max(128), z.json()))
    .refine(
      (value) => Buffer.byteLength(JSON.stringify(value)) <= 16384,
      "Provider options exceed 16 KiB",
    )
    .default({}),
});
export type ModelProfile = z.infer<typeof modelSchema>;
export type ExecutionProfile = z.infer<typeof executionProfileSchema>;
