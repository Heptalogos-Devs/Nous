// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  resolveRole,
  snapshotSchema,
  type ReadyRole,
  type ModelRoleSnapshot,
  type ModelProducerMetadata,
} from "./execution/snapshot.js";
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
  modelRoleGraph,
} from "./configuration.js";
import { PromptRegistry, type PromptAsset } from "./prompts.js";
import { invocationConfigDigest } from "./identity.js";
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
type GenerationOptions<R extends ModelRole> = {
  signal?: AbortSignal;
  snapshot?: ModelRoleSnapshot;
  prompt?: ModelRole | { role: ModelRole; path: string };
  beforeAttempt?: () => void;
  validateOutput?: (
    value: ModelGenerationOutput<R>,
    input: InputModalities,
  ) => void;
};

export class ModelInvocations {
  private prompts?: PromptRegistry;
  private promptPaths: Partial<Record<ModelRole, string>> = {};
  private readonly active = new Map<ModelRole, ReadyRole[]>();
  private configuration!: ModelConfiguration;
  private readonly credentialOrigins = new Set<string>();
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
    for (const gateway of Object.values(config.gateway_profiles))
      if (gateway.enabled)
        runtime.credentialOrigins.add(
          `${gateway.credential_env}@${new URL(gateway.base_url).origin}`,
        );
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
  snapshot(name: ModelRole): ModelRoleSnapshot {
    const role = this.require(name);
    return snapshotSchema.parse({
      format: "nous.model.execution",
      implementationDigest: modelImplementations.provider,
      outputSchemaDigest: providerContractForRole(name)?.digest,
      role: name,
      configuration: modelRoleGraph(this.configuration, name),
      prompt: role.prompt,
      profileDigest: role.profileDigest,
      configDigest: canonicalDigest({
        policy: role.policy,
        configuration: modelRoleGraph(this.configuration, name),
        prompt: role.prompt && {
          id: role.prompt.id,
          digest: role.prompt.digest,
        },
        outputSchemaDigest: providerContractForRole(name)?.digest,
      }),
    });
  }
  async snapshotPrompt(
    name: ModelRole,
    path?: string,
    adapter?: string,
  ): Promise<ModelRoleSnapshot> {
    const snapshot = this.snapshot(name);
    if (path) {
      const prompt = await this.prompts?.load(name, path);
      if (!prompt) throw new Error("Strategy prompt unavailable");
      snapshot.prompt = prompt;
    }
    snapshot.configDigest = invocationConfigDigest(
      snapshot.configDigest,
      path ? snapshot.prompt : undefined,
      adapter,
    );
    return snapshot;
  }
  private async withRoutes<
    T extends {
      value: unknown;
      producerMetadata: ModelProducerMetadata;
      usage?: unknown;
    },
    Input = undefined,
  >(
    name: ModelRole,
    invoke: (role: ReadyRole, input: Input) => Promise<T>,
    signal?: AbortSignal,
    fixed?: ModelRoleSnapshot,
    beforeAttempt?: () => void,
    prepare?: (role: ReadyRole) => Input,
  ) {
    return executeRoutes(
      name,
      fixed ?? this.snapshot(name),
      this.credentialOrigins,
      invoke,
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
  private metadata(role: ReadyRole, adapter: string): ModelProducerMetadata {
    return {
      implementation: `${adapter}:${modelImplementations.provider}`,
      protocol: role.profile.protocol,
      model: role.profile.model,
      modelRevision: role.profile.model_revision,
      profileDigest: role.profileDigest,
      promptId: role.prompt?.id,
      promptDigest: role.prompt?.digest,
      configDigest: role.configDigest,
      modelRole: role.name,
      modelProfile: role.binding.model,
      executionProfile: role.executionName,
      inferenceControlsDigest: canonicalDigest(role.binding),
      rolePolicyDigest: role.policyDigest,
    };
  }
  async generate<R extends ModelRole>(
    name: R,
    request: GenerationInput,
    options: GenerationOptions<R> = {},
  ) {
    const {
      signal,
      prompt: promptRole,
      beforeAttempt,
      validateOutput,
    } = options;
    const { content, media } = request;
    let snapshot = options.snapshot ?? this.snapshot(name);
    if (promptRole) {
      const selected =
        typeof promptRole === "string" ? promptRole : promptRole.role;
      const prompt = await this.prompts?.load(
        selected,
        typeof promptRole === "string"
          ? this.promptPaths[selected]
          : promptRole.path,
      );
      if (!prompt) throw new Error("Strategy prompt unavailable");
      snapshot = {
        ...snapshot,
        prompt,
        configDigest: invocationConfigDigest(snapshot.configDigest, prompt),
      };
    }
    return this.withRoutes<
      {
        value: ModelGenerationOutput<R>;
        producerMetadata: ModelProducerMetadata;
        usage?: unknown;
        input: InputModalities;
      },
      { input: InputModalities; content: UserContent }
    >(
      name,
      async (role, { input, content: supplied }) => {
        const outputContract = providerContractForRole(name);
        const { adapter, ...output } = await generateProvider(
          role,
          supplied,
          outputContract,
          signal,
          media,
        );
        const result = {
          ...output,
          value: output.value as ModelGenerationOutput<R>,
          producerMetadata: {
            ...this.metadata(role, adapter),
            outputSchemaDigest: outputContract?.digest,
          },
        };
        try {
          validateOutput?.(result.value, input);
        } catch {
          // Owner validation errors may contain source content; keep the public classification bounded.
          throw new GenerationFailure(
            name,
            "output_semantics_invalid",
            undefined,
            result.usage,
          );
        }
        return { ...result, input };
      },
      signal,
      snapshot,
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
  async transcription(
    audio: Uint8Array,
    signal?: AbortSignal,
    fixed?: ModelRoleSnapshot,
  ) {
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
      fixed,
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
