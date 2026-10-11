// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { z } from "zod";
import {
  gatewaySchema,
  modelSchema,
  executionProfileSchema,
  profileNameSchema,
  executionTimeoutSchema,
  generationTokensSchema,
  type ExecutionProfile,
} from "./profiles.js";
import { structuredContractForRole } from "./schemas/contracts.js";

import { roleNames, type ModelRole } from "./roles.js";
const rolePolicySchema = z.strictObject({
  routes: z
    .array(profileNameSchema)
    .min(1)
    .max(4)
    .refine(
      (routes) => new Set(routes).size === routes.length,
      "Duplicate execution route",
    ),
  prompt: profileNameSchema.optional(),
  requirement: z.enum(["optional", "required"]).default("optional"),
});
const modelExecutionShape = {
  gateway_profiles: z.record(z.string(), gatewaySchema).default({}),
  model_profiles: z.record(z.string(), modelSchema).default({}),
  execution_profiles: z.record(z.string(), executionProfileSchema).default({}),
  roles: z.partialRecord(z.enum(roleNames), rolePolicySchema).default({}),
};
/** Resource references and protocol-dependent defaults form one atomic owning policy. */
export const modelExecutionSchema = z
  .strictObject(modelExecutionShape)
  .superRefine((graph, context) => {
    const issue = (path: (string | number)[], message: string) =>
      context.addIssue({ code: "custom", path, message });
    for (const [name, model] of Object.entries(graph.model_profiles)) {
      if (!graph.gateway_profiles[model.gateway])
        issue(
          ["model_profiles", name, "gateway"],
          `Gateway profile ${model.gateway} does not exist`,
        );
      const expected =
        model.protocol === "openai-embeddings"
          ? "embedding"
          : model.protocol === "rerank-v1"
            ? "rerank"
            : model.protocol === "openai-audio-transcription"
              ? "speech_transcription"
              : "text";
      if (!model.capabilities.includes(expected))
        issue(
          ["model_profiles", name, "capabilities"],
          `Protocol ${model.protocol} requires ${expected}`,
        );
      const allowed = ["openai-chat", "openai-responses"].includes(
        model.protocol,
      )
        ? new Set([
            "text",
            "image_input",
            "audio_input",
            "video_input",
            "structured_output",
          ])
        : new Set(
            model.protocol === "openai-audio-transcription"
              ? ["speech_transcription", "audio_input"]
              : [expected],
          );
      for (const [index, capability] of model.capabilities.entries())
        if (!allowed.has(capability))
          issue(
            ["model_profiles", name, "capabilities", index],
            `Protocol ${model.protocol} does not consume ${capability}`,
          );
    }
    for (const [name, execution] of Object.entries(graph.execution_profiles)) {
      const model = graph.model_profiles[execution.model];
      if (!model) {
        issue(
          ["execution_profiles", name, "model"],
          `Model profile ${execution.model} does not exist`,
        );
        continue;
      }
      if (!model.reasoning_levels.includes(execution.reasoning))
        issue(
          ["execution_profiles", name, "reasoning"],
          `Model profile ${execution.model} does not support ${execution.reasoning}`,
        );
      if (
        !["openai-chat", "openai-responses"].includes(model.protocol) &&
        (execution.temperature !== undefined ||
          execution.top_p !== undefined ||
          execution.max_output_tokens !== undefined ||
          execution.reasoning !== "provider-default")
      )
        issue(
          ["execution_profiles", name],
          `Protocol ${model.protocol} does not consume generation controls`,
        );
    }
    for (const [name, policy] of Object.entries(graph.roles)) {
      for (const [index, route] of policy.routes.entries()) {
        const execution = graph.execution_profiles[route];
        if (!execution) {
          issue(
            ["roles", name, "routes", index],
            `Execution profile ${route} does not exist`,
          );
          continue;
        }
        const model = graph.model_profiles[execution.model];
        const problem = model && modelRoleProblem(name, execution, model);
        if (problem)
          issue(["roles", name, "routes", index], `${route}: ${problem}`);
      }
    }
  })
  .overwrite((graph) => ({
    ...graph,
    execution_profiles: Object.fromEntries(
      Object.entries(graph.execution_profiles).map(([name, execution]) => {
        const model = graph.model_profiles[execution.model]!;
        const gateway = graph.gateway_profiles[model.gateway]!;
        return [
          name,
          {
            ...execution,
            timeout_ms: execution.timeout_ms ?? gateway.request_timeout_ms,
            ...(["openai-chat", "openai-responses"].includes(model.protocol)
              ? {
                  max_output_tokens:
                    execution.max_output_tokens ??
                    generationTokensSchema.parse(undefined),
                }
              : {}),
          },
        ];
      }),
    ),
  }))
  .prefault({});
export type ModelExecutionConfiguration = z.infer<typeof modelExecutionSchema>;

export function modelRoleGraph(
  graph: ModelExecutionConfiguration,
  name: ModelRole,
): ModelExecutionConfiguration {
  const policy = graph.roles[name];
  const executions = new Set(policy?.routes ?? []);
  const models = new Set(
    [...executions].map((key) => graph.execution_profiles[key]!.model),
  );
  const gateways = new Set(
    [...models].map((key) => graph.model_profiles[key]!.gateway),
  );
  const select = <T>(values: Record<string, T>, keys: Set<string>) =>
    Object.fromEntries([...keys].sort().map((key) => [key, values[key]!]));
  return modelExecutionSchema.parse({
    gateway_profiles: select(graph.gateway_profiles, gateways),
    model_profiles: select(graph.model_profiles, models),
    execution_profiles: select(graph.execution_profiles, executions),
    roles: policy ? { [name]: policy } : {},
  });
}
export const modelConfigurationShape = {
  material_inputs: z
    .strictObject({
      formation_source_max_bytes: z
        .number()
        .int()
        .min(1)
        .max(1048576)
        .default(32768),
      derivation_source_max_bytes: z
        .number()
        .int()
        .min(1)
        .max(1048576)
        .default(1048576),
    })
    .prefault({}),
  video: z
    .strictObject({
      input_mode: z.enum(["direct", "frames"]).default("direct"),
      ffmpeg_executable: z.string().min(1).nullable().default(null),
      max_source_bytes: z
        .number()
        .int()
        .min(1024)
        .max(67108864)
        .default(16777216),
      max_video_seconds: z.number().positive().max(3600).default(60),
      max_frames: z.number().int().min(1).max(16).default(8),
      frame_end_margin_seconds: z.number().min(0).max(1).default(0.1),
      max_frame_bytes: z.number().int().min(1024).max(1048576).default(262144),
      max_audio_bytes: z.number().int().min(44).max(16777216).default(4194304),
      process_timeout_ms: executionTimeoutSchema.default(30000),
      prompt: z.string().min(1).default("material/video-description.md"),
    })
    .prefault({}),
  audio: z
    .strictObject({
      input_mode: z.enum(["direct", "transcription"]).default("direct"),
      max_source_bytes: z
        .number()
        .int()
        .min(1024)
        .max(25165824)
        .default(16777216),
    })
    .prefault({}),
  material_strategy: z
    .enum(["description_only", "direct_structured", "describe_then_structure"])
    .default("description_only"),
  ...modelExecutionShape,
};
const graphFrom = (
  config: z.infer<
    ReturnType<typeof z.strictObject<typeof modelConfigurationShape>>
  >,
) => ({
  gateway_profiles: config.gateway_profiles,
  model_profiles: config.model_profiles,
  execution_profiles: config.execution_profiles,
  roles: config.roles,
});
export const modelConfigurationSchema = z
  .strictObject(modelConfigurationShape)
  .superRefine((config, context) => {
    const graph = modelExecutionSchema.safeParse(graphFrom(config));
    if (!graph.success)
      for (const issue of graph.error.issues)
        context.addIssue({
          code: "custom",
          path: issue.path,
          message: issue.message,
        });
  })
  .overwrite((config) => {
    const graph = modelExecutionSchema.safeParse(graphFrom(config));
    return graph.success ? { ...config, ...graph.data } : config;
  });

/** Cross-profile graph readiness is an owner diagnostic, not a second Catalog validator. */
export function modelRoleProblem(
  role: string,
  binding: ExecutionProfile,
  model: z.infer<typeof modelSchema>,
): string | undefined {
  const protocol =
    role === "query_embedding"
      ? "openai-embeddings"
      : role === "query_rerank"
        ? "rerank-v1"
        : role === "speech_transcription"
          ? "openai-audio-transcription"
          : undefined;
  const capability =
    role === "query_embedding"
      ? "embedding"
      : role === "query_rerank"
        ? "rerank"
        : role === "speech_transcription"
          ? "speech_transcription"
          : "text";
  if (
    (protocol
      ? model.protocol !== protocol
      : !["openai-chat", "openai-responses"].includes(model.protocol)) ||
    !model.capabilities.includes(capability)
  )
    return "Role protocol/capability mismatch";
  if (
    structuredContractForRole(role as ModelRole) &&
    !model.capabilities.includes("structured_output")
  )
    return "Role requires structured_output";
  if (
    role === "material_direct_structuring" &&
    !["image_input", "audio_input", "video_input"].some((inputCapability) =>
      model.capabilities.includes(
        inputCapability as "image_input" | "audio_input" | "video_input",
      ),
    )
  )
    return "Direct structuring requires a declared media input capability";
  if (!model.reasoning_levels.includes(binding.reasoning))
    return "Unsupported reasoning level";
  if (
    protocol &&
    (binding.reasoning !== "provider-default" ||
      binding.temperature !== undefined ||
      binding.top_p !== undefined ||
      binding.max_output_tokens !== undefined)
  )
    return "Role does not consume generation parameters";
}
export type ModelConfiguration = z.infer<typeof modelConfigurationSchema>;
export type RolePolicy = z.infer<typeof rolePolicySchema>;
