// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { createOpenAI } from "@ai-sdk/openai";
import {
  embedMany,
  generateText,
  Output,
  NoObjectGeneratedError,
  APICallError,
  transcribe,
  jsonSchema,
  type UserContent,
} from "ai";
import { z } from "zod";
import { getStreamAsArrayBuffer } from "get-stream";
import type { ModelProfile, ExecutionProfile } from "./profiles.js";
import type { PromptAsset } from "./prompts.js";
import { directAudioFormats, type DirectMedia } from "./input.js";

export type ProviderExecution = {
  profile: ModelProfile;
  binding: ExecutionProfile;
  prompt?: PromptAsset;
  baseURL: string;
  credential: string;
  timeout: number;
};
type ProviderOutput = {
  owner: z.ZodType;
  sdkSchema: ReturnType<typeof jsonSchema>;
  providerSchema: Record<string, unknown>;
  providerName: string;
  digest: string;
};
export function usageCounts(input: unknown) {
  if (!input || typeof input !== "object") return undefined;
  const counters = Object.fromEntries(Object.entries(input));
  const count = (entry: unknown) => {
    const value =
      entry && typeof entry === "object" && "total" in entry
        ? entry.total
        : entry;
    return typeof value === "number" &&
      Number.isSafeInteger(value) &&
      value >= 0
      ? value
      : undefined;
  };
  const usage = {
    inputTokens: count(counters.inputTokens ?? counters.prompt_tokens),
    outputTokens: count(counters.outputTokens ?? counters.completion_tokens),
    totalTokens: count(counters.totalTokens ?? counters.total_tokens),
  };
  return Object.values(usage).some((value) => value !== undefined)
    ? usage
    : undefined;
}

export class ProviderFailure extends Error {
  constructor(
    message: string,
    readonly usage?: unknown,
    readonly outcome: "failed" | "unknown" = "failed",
  ) {
    super(message);
  }
}
class MediaProtocolError extends ProviderFailure {}
class ModelOutputError extends ProviderFailure {}
function provider(role: ProviderExecution) {
  return createOpenAI({
    baseURL: role.baseURL,
    apiKey: role.credential,
    name: "standard-gateway",
    fetch: (destination, init) =>
      fetch(destination, { ...init, redirect: "error" }),
  });
}
function providerSignal(role: ProviderExecution, signal?: AbortSignal) {
  return signal
    ? AbortSignal.any([signal, AbortSignal.timeout(role.timeout)])
    : AbortSignal.timeout(role.timeout);
}
export function safeProviderFailure(error: unknown) {
  if (error instanceof ProviderFailure) return error.message;
  if (error instanceof MediaProtocolError) return error.message;
  if (error instanceof ModelOutputError) return error.message;
  if (NoObjectGeneratedError.isInstance(error))
    return error.finishReason && error.finishReason !== "stop"
      ? `output_incomplete_${error.finishReason}`
      : "output_schema_invalid";
  if (error instanceof z.ZodError) return "output_schema_invalid";
  if (error instanceof SyntaxError) return "output_json_invalid";
  if (
    APICallError.isInstance(error) &&
    error.statusCode &&
    Number.isInteger(error.statusCode) &&
    error.statusCode >= 100 &&
    error.statusCode <= 599
  )
    return `gateway_http_${error.statusCode}`;
  if (
    error instanceof Error &&
    ["AbortError", "TimeoutError"].includes(error.name)
  )
    return "timeout_or_cancellation";
  return "validation_or_transport";
}

const rerankResponse = z.object({
  results: z
    .array(
      z.object({
        index: z.number().int().nonnegative(),
        relevance_score: z.number().finite(),
      }),
    )
    .max(64),
  usage: z.unknown().optional(),
});
function parseRerankResponse(input: unknown, count: number) {
  const result = rerankResponse.parse(input);
  const indices = new Set<number>();
  for (const item of result.results) {
    if (item.index >= count || indices.has(item.index))
      throw new Error("Invalid rerank candidate index");
    indices.add(item.index);
  }
  return result;
}

export async function generateProvider(
  role: ProviderExecution,
  content: UserContent,
  output: ProviderOutput | undefined,
  signal?: AbortSignal,
  media?: DirectMedia,
) {
  if (
    role.profile.protocol !== "openai-chat" &&
    role.profile.protocol !== "openai-responses"
  )
    throw new Error("Role is not a text generation protocol");
  const schema = output?.owner;
  if (output && !role.profile.capabilities.includes("structured_output"))
    throw new Error("Strict structured output capability is unavailable");
  let mediaStage = "admission";
  let receivedUsage: unknown;
  try {
    if (media) {
      const audio = media.mediaType.startsWith("audio/");
      const encoded = Buffer.from(media.bytes).toString("base64");
      const part = audio
        ? {
            type: "input_audio",
            input_audio: {
              data: encoded,
              format: directAudioFormats[media.mediaType],
            },
          }
        : {
            type: "video_url",
            video_url: { url: `data:${media.mediaType};base64,${encoded}` },
          };
      mediaStage = "transport";
      const response = await fetch(`${role.baseURL}/chat/completions`, {
        method: "POST",
        redirect: "error",
        signal: providerSignal(role, signal),
        headers: {
          Authorization: `Bearer ${role.credential}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          model: role.profile.model,
          stream: false,
          messages: [
            { role: "system", content: role.prompt?.text ?? "" },
            {
              role: "user",
              content: [{ type: "text", text: content }, part],
            },
          ],
          temperature: role.binding.temperature,
          top_p: role.binding.top_p,
          max_tokens: role.binding.max_output_tokens,
          ...(role.binding.reasoning === "provider-default"
            ? {}
            : { reasoning_effort: role.binding.reasoning }),
          ...(schema
            ? {
                response_format: {
                  type: "json_schema",
                  json_schema: {
                    name: output!.providerName,
                    strict: true,
                    schema: output!.providerSchema,
                  },
                },
              }
            : {}),
        }),
      });
      if (!response.ok || !response.body) {
        await response.body?.cancel();
        throw new MediaProtocolError(
          `Direct media gateway HTTP ${response.status}`,
        );
      }
      mediaStage = "response_read";
      const bytes = await getStreamAsArrayBuffer(response.body, {
        maxBuffer: 1048576,
      });
      mediaStage = "response_validation";
      const wire: unknown = JSON.parse(
        new TextDecoder("utf-8", { fatal: true }).decode(bytes),
      );
      if (wire && typeof wire === "object" && "usage" in wire)
        receivedUsage = wire.usage;
      const result = z
        .object({
          choices: z
            .array(
              z.object({
                finish_reason: z.literal("stop"),
                message: z.object({ content: z.string().min(1).max(65536) }),
              }),
            )
            .min(1)
            .max(1),
          usage: z.unknown().optional(),
        })
        .parse(wire);
      const text = result.choices[0]!.message.content;
      const value = schema ? schema.parse(JSON.parse(text)) : text;
      return {
        value,
        usage: usageCounts(result.usage),
        adapter: "gateway-chat-media",
      };
    }
    const result = await generateText({
      model:
        role.profile.protocol === "openai-chat"
          ? provider(role).chat(role.profile.model)
          : provider(role).responses(role.profile.model),
      instructions: role.prompt?.text,
      messages: [{ role: "user", content }],
      output: output
        ? Output.object({
            schema: output.sdkSchema,
            name: output.providerName,
          })
        : undefined,
      temperature: role.binding.temperature,
      topP: role.binding.top_p,
      maxOutputTokens: role.binding.max_output_tokens,
      reasoning: role.binding.reasoning,
      providerOptions: role.binding.provider_options,
      maxRetries: 0,
      abortSignal: providerSignal(role, signal),
    });
    receivedUsage = result.totalUsage;
    if (!schema && !result.text.trim()) throw new Error("Empty model output");
    if (schema && result.finishReason !== "stop")
      throw new ModelOutputError(
        `output_incomplete_${result.finishReason}`,
        result.totalUsage,
      );
    const value = schema ? schema.parse(result.output) : result.text;
    return {
      value,
      usage: result.totalUsage,
      adapter: "ai-sdk",
    };
  } catch (error) {
    if (signal?.aborted) throw signal.reason;
    if (error instanceof MediaProtocolError) throw error;
    if (media)
      throw new MediaProtocolError(
        `Direct media invocation failed at ${mediaStage}`,
        usageCounts(receivedUsage),
        ["transport", "response_read"].includes(mediaStage)
          ? "unknown"
          : "failed",
      );
    // Provider error bodies may echo headers, input material, or credentials.
    throw new ProviderFailure(
      safeProviderFailure(error),
      NoObjectGeneratedError.isInstance(error) ||
        error instanceof ModelOutputError
        ? usageCounts(error.usage)
        : usageCounts(receivedUsage),
      receivedUsage === undefined &&
        ["validation_or_transport", "timeout_or_cancellation"].includes(
          safeProviderFailure(error),
        )
        ? "unknown"
        : "failed",
    );
  }
}

export async function embedProvider(
  role: ProviderExecution,
  texts: string[],
  model: string,
  signal?: AbortSignal,
) {
  if (
    role.profile.model !== model ||
    !role.profile.embedding ||
    !texts.length ||
    texts.length > 64 ||
    texts.reduce((bytes, text) => bytes + Buffer.byteLength(text), 0) > 1048576
  )
    throw new Error("Embedding batch violates configured profile/bounds");
  try {
    const embeddings: number[][] = [];
    for (
      let offset = 0;
      offset < texts.length;
      offset += role.profile.embedding.max_batch_size
    ) {
      const result = await embedMany({
        model: provider(role).embeddingModel(model),
        values: texts.slice(
          offset,
          offset + role.profile.embedding.max_batch_size,
        ),
        maxRetries: 0,
        maxParallelCalls: 1,
        abortSignal: providerSignal(role, signal),
      });
      embeddings.push(...result.embeddings);
    }
    if (
      embeddings.length !== texts.length ||
      embeddings.some(
        (vector) =>
          vector.length !== role.profile.embedding!.dimension ||
          vector.some((value) => !Number.isFinite(value)),
      )
    )
      throw new Error("Invalid embedding batch output");
    return {
      value: embeddings,
      adapter: "ai-sdk",
    };
  } catch (error) {
    if (signal?.aborted) throw signal.reason;
    const status =
      error &&
      typeof error === "object" &&
      "statusCode" in error &&
      typeof error.statusCode === "number" &&
      Number.isInteger(error.statusCode)
        ? `gateway_http_${error.statusCode}`
        : "embedding_validation_or_transport";
    // Provider causes can expose credentials or corpus text; only numeric status is public.
    throw new ModelOutputError(status);
  }
}

export async function transcribeProvider(
  role: ProviderExecution,
  audio: Uint8Array,
  signal?: AbortSignal,
) {
  try {
    const result = await transcribe({
      model: provider(role).transcription(role.profile.model),
      audio,
      maxRetries: 0,
      abortSignal: providerSignal(role, signal),
    });
    if (!result.text.trim()) throw new Error("Empty transcript");
    return { value: result.text, adapter: "ai-sdk" };
  } catch {
    if (signal?.aborted) throw signal.reason;
    throw new Error("Speech transcription invocation failed");
  }
}

export async function rerankProvider(
  role: ProviderExecution,
  query: string,
  documents: string[],
  topN: number,
  signal?: AbortSignal,
) {
  if (
    !query.trim() ||
    !documents.length ||
    documents.length > 64 ||
    topN < 1 ||
    topN > documents.length ||
    Buffer.byteLength(JSON.stringify(documents)) > 2 * 1024 * 1024
  )
    throw new Error("Rerank request exceeds bounds");
  try {
    const response = await fetch(`${role.baseURL}/rerank`, {
      method: "POST",
      redirect: "error",
      signal: providerSignal(role, signal),
      headers: {
        Authorization: `Bearer ${role.credential}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        model: role.profile.model,
        query,
        documents,
        top_n: topN,
        return_documents: false,
      }),
    });
    if (!response.ok || !response.body) {
      await response.body?.cancel();
      throw new Error("Rerank gateway failure");
    }
    const bytes = await getStreamAsArrayBuffer(response.body, {
      maxBuffer: 1048576,
    });
    const result = parseRerankResponse(
      JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)),
      documents.length,
    );
    return {
      value: result.results,
      adapter: "rerank-v1",
    };
  } catch {
    if (signal?.aborted) throw signal.reason;
    throw new Error(
      "Rerank invocation failed or response violated candidate mapping",
    );
  }
}
