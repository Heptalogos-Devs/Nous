// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  resolveRole,
  type ReadyRole,
  type ModelProducerMetadata,
} from "./execution/role.js";
import { executeRoutes, GenerationFailure } from "./execution/routes.js";
import type { UserContent } from "ai";
import {
  generateProvider,
  embedProvider,
  transcribeProvider,
  rerankProvider,
} from "./protocols.js";

import { resolvedEmbeddingRoute } from "./embedding-profile.js";
import { canonicalDigest } from "../digest.js";
import {
  providerContractForRole,
  type ModelGenerationOutput,
} from "./schemas/contracts.js";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { roleNames, type ModelRole } from "./roles.js";
import {
  type ModelConfiguration,
  type RolePolicy,
  modelConfigurationSchema,
} from "./configuration.js";
import { PromptRegistry, type PromptAsset } from "./prompts.js";
import { modelRoleIdentity } from "./identity.js";
import { modelImplementations } from "./implementation.js";
import {
  generationInput,
  type InputModalities,
  type DirectMedia,
} from "./input.js";

type GenerationInput = {
  content: UserContent;
  media?: DirectMedia;
  context?: (input: InputModalities) => Record<string, unknown>;
};
export type ModelGenerationOverrides = {
  prompt?: ModelRole | { role: ModelRole; path: string };
  adapterIdentity?: string;
};

type GenerationOptions<
  R extends ModelRole,
  MappedOutput = ModelGenerationOutput<R>,
> = ModelGenerationOverrides & {
  signal?: AbortSignal;
  beforeAttempt?: () => void;
  mapOutput?: (
    value: ModelGenerationOutput<R>,
    input: InputModalities,
  ) => MappedOutput;
};

export class ModelInvocations {
  private prompts?: PromptRegistry;
  private configuration!: ModelConfiguration;
  private promptPaths: Partial<Record<ModelRole, string>> = {};
  private readonly active = new Map<ModelRole, ReadyRole[]>();
  private readonly requirements = new Map<
    ModelRole,
    RolePolicy["requirement"]
  >();
  private readonly states = new Map<
    ModelRole,
    { state: string; detail: string }
  >();
  static async create(
    config: ModelConfiguration,
    promptRoot = resolve(
      dirname(fileURLToPath(import.meta.url)),
      "../../../../prompts",
    ),
    overridePromptRoot?: string,
  ) {
    const runtime = new ModelInvocations();
    config = modelConfigurationSchema.parse(config);
    runtime.configuration = config;
    const prompts = new PromptRegistry(promptRoot, overridePromptRoot);
    runtime.prompts = prompts;
    for (const name of roleNames) {
      const policy = config.roles[name];
      runtime.requirements.set(name, policy?.requirement ?? "optional");
      if (!policy) {
        runtime.states.set(name, {
          state: "NOT_CONFIGURED",
          detail: "Role has no policy",
        });
        continue;
      }
      if (policy.prompt) runtime.promptPaths[name] = policy.prompt;
      const routes: ReadyRole[] = [];
      let problem = "Execution route unavailable";
      let prompt: PromptAsset | undefined;
      try {
        prompt = await prompts.load(name, policy.prompt);
      } catch {
        problem = "Prompt asset unavailable or invalid";
      }
      for (const executionName of policy.routes) {
        try {
          routes.push(resolveRole(config, name, executionName, prompt));
        } catch (error) {
          problem =
            error instanceof Error
              ? error.message
              : "Execution route unavailable";
        }
      }
      if (name === "query_embedding" && routes.length > 1) {
        const signature = (route: ReadyRole) =>
          canonicalDigest({
            model: route.profile.model,
            embedding: route.profile.embedding,
          });
        if (routes.some((route) => signature(route) !== signature(routes[0]!)))
          throw new Error(
            "Embedding routes require the identical full embedding space signature",
          );
      }
      runtime.active.set(name, routes);
      runtime.states.set(name, {
        state: routes.length ? "READY" : "UNAVAILABLE",
        detail: routes.length ? "Executable ordered routes available" : problem,
      });
    }
    return runtime;
  }
  get capabilities() {
    return roleNames.map((role) => ({
      name: `model.${role}`,
      ...(this.states.get(role) ?? {
        state: "NOT_CONFIGURED",
        detail: "Role has no binding",
      }),
    }));
  }
  profile(role: ModelRole) {
    return this.active.get(role)?.[0]?.profile;
  }
  requirement(role: ModelRole) {
    return this.requirements.get(role) ?? "optional";
  }
  executionDigest(role: ModelRole): string {
    const routes = this.active.get(role) ?? [];
    if (!routes.length) throw new Error(`Model role ${role} unavailable`);
    return canonicalDigest(routes.map((route) => route.configDigest));
  }
  private async withRoutes<T, Input = undefined>(
    name: ModelRole,
    invoke: (
      role: ReadyRole,
      input: Input,
      overrides: { prompt?: PromptAsset; adapterIdentity?: string },
    ) => Promise<T>,
    signal?: AbortSignal,
    beforeAttempt?: () => void,
    prepare?: (role: ReadyRole) => Input,
    prompt?: PromptAsset,
    adapterIdentity?: string,
  ) {
    const routes = this.active.get(name) ?? [];
    if (!routes.length) this.require(name);
    const overrides = { prompt, adapterIdentity };
    return executeRoutes(
      name,
      routes,
      (role, input) => invoke(role, input, overrides),
      signal,
      beforeAttempt,
      prepare,
    );
  }
  identity(role: ModelRole) {
    return this.active.get(role)?.[0]?.configDigest;
  }
  private require(role: ModelRole): ReadyRole {
    const value = this.active.get(role)?.[0];
    if (!value) throw new Error(`Model role ${role} unavailable`);
    return value;
  }
  private metadata(
    role: ReadyRole,
    adapter: string,
    overrides: { prompt?: PromptAsset; adapterIdentity?: string } = {},
  ): ModelProducerMetadata {
    const prompt = overrides.prompt ?? role.prompt;
    const configDigest =
      overrides.prompt || overrides.adapterIdentity
        ? canonicalDigest({
            execution: modelRoleIdentity(
              role.profile,
              {
                base_url: role.baseURL,
                request_timeout_ms: role.timeout,
              },
              role.binding,
              prompt,
            ).configDigest,
            policyDigest: role.policyDigest,
            executionName: role.executionName,
            ...(overrides.adapterIdentity
              ? { adapterIdentity: overrides.adapterIdentity }
              : {}),
          })
        : role.configDigest;
    return {
      implementation: `${adapter}:${modelImplementations.provider}`,
      protocol: role.profile.protocol,
      model: role.profile.model,
      modelRevision: role.profile.model_revision,
      profileDigest: role.profileDigest,
      promptId: prompt?.id,
      promptDigest: prompt?.digest,
      configDigest,
      modelRole: role.name,
      modelProfile: role.binding.model,
      executionProfile: role.executionName,
      inferenceControlsDigest: canonicalDigest(role.binding),
      rolePolicyDigest: role.policyDigest,
    };
  }
  async generate<R extends ModelRole, MappedOutput = ModelGenerationOutput<R>>(
    name: R,
    request: GenerationInput,
    options: GenerationOptions<R, MappedOutput> = {},
  ) {
    const {
      signal,
      prompt: promptRole,
      adapterIdentity,
      beforeAttempt,
      mapOutput,
    } = options;
    const { content, media } = request;
    let prompt: PromptAsset | undefined;
    if (promptRole) {
      const selected =
        typeof promptRole === "string" ? promptRole : promptRole.role;
      prompt = await this.prompts?.load(
        selected,
        typeof promptRole === "string"
          ? this.promptPaths[selected]
          : promptRole.path,
      );
      if (!prompt) throw new Error("Strategy prompt unavailable");
    }
    return this.withRoutes<
      {
        value: MappedOutput;
        producerMetadata: ModelProducerMetadata;
        input: InputModalities;
      },
      { input: InputModalities; content: UserContent }
    >(
      name,
      async (role, { input, content: supplied }, overrides) => {
        const outputContract = providerContractForRole(name);
        const { adapter, ...output } = await generateProvider(
          role,
          supplied,
          outputContract,
          signal,
          media,
          overrides.prompt,
        );
        const value = output.value as ModelGenerationOutput<R>;
        let mappedValue: MappedOutput;
        try {
          mappedValue = mapOutput
            ? mapOutput(value, input)
            : (value as MappedOutput);
        } catch {
          // Owner validation errors may contain source content; keep the public classification bounded.
          throw new GenerationFailure(name, "output_semantics_invalid");
        }
        return {
          ...output,
          value: mappedValue,
          producerMetadata: {
            ...this.metadata(role, adapter, overrides),
            outputSchemaDigest: outputContract?.digest,
          },
          input,
        };
      },
      signal,
      beforeAttempt,
      (role) => {
        const input = generationInput(role.profile, content, media);
        const context = request.context?.(input);
        const supplied = context
          ? typeof content === "string"
            ? JSON.stringify({ input_context: context, content })
            : [
                {
                  type: "text" as const,
                  text: JSON.stringify({ input_context: context }),
                },
                ...content,
              ]
          : content;
        return {
          input: context ? { ...input, text: true } : input,
          content: supplied,
        };
      },
      prompt,
      adapterIdentity,
    );
  }
  async embeddingBatch(texts: string[], model: string, signal?: AbortSignal) {
    const result = await this.withRoutes(
      "query_embedding",
      async (role) => {
        const { adapter, ...output } = await embedProvider(
          role,
          texts,
          model,
          signal,
        );
        return { ...output, producerMetadata: this.metadata(role, adapter) };
      },
      signal,
    );
    const producer = resolvedEmbeddingRoute(
      this.configuration,
      result.producerMetadata.executionProfile,
    )?.producer;
    if (!producer)
      throw new Error(
        "Successful embedding execution has no canonical producer",
      );
    return { ...result, producer };
  }
  async transcription(audio: Uint8Array, signal?: AbortSignal) {
    return this.withRoutes(
      "speech_transcription",
      async (role) => {
        const { adapter, ...result } = await transcribeProvider(
          role,
          audio,
          signal,
        );
        return { ...result, producerMetadata: this.metadata(role, adapter) };
      },
      signal,
    );
  }
  async rerank(
    query: string,
    documents: string[],
    topN: number,
    signal?: AbortSignal,
  ) {
    return this.withRoutes(
      "query_rerank",
      async (role) => {
        const { adapter, ...result } = await rerankProvider(
          role,
          query,
          documents,
          topN,
          signal,
        );
        return { ...result, producerMetadata: this.metadata(role, adapter) };
      },
      signal,
    );
  }
}
