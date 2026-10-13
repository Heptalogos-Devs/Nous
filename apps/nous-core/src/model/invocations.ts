import { Code, ConnectError } from "@connectrpc/connect";
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  resolveRole,
  bindPrompt,
  type ReadyRole,
  type PreparedModelRole,
  type ModelProducerMetadata,
} from "./execution/binding.js";
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
  modelConfigurationSchema,
  modelRoleGraph,
} from "./configuration.js";
import { PromptRegistry, type PromptAsset } from "./prompts.js";
import { invocationConfigDigest } from "./identity.js";
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
  prepared?: PreparedModelRole;
  prompt?: ModelRole | { role: ModelRole; path: string };
  beforeAttempt?: () => void;
  validateOutput?: (
    value: ModelGenerationOutput<R>,
    input: InputModalities,
  ) => void;
};

export class ModelInvocations {
  private prompts?: PromptRegistry;
  private configuration!: ModelConfiguration;
  private readonly roles = new Map<
    ModelRole,
    { state: string; detail: string; prepared?: PreparedModelRole }
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
      if (!policy) {
        runtime.roles.set(name, {
          state: "NOT_CONFIGURED",
          detail: "Role has no policy",
        });
        continue;
      }
      const routes: PreparedModelRole["routes"] = [];
      let problem = "Execution route unavailable";
      let prompt: PromptAsset | undefined;
      try {
        prompt = await prompts.load(name, policy.prompt);
      } catch {
        problem = "Prompt asset unavailable or invalid";
      }
      const configDigest = canonicalDigest({
        policy,
        configuration: modelRoleGraph(config, name),
        prompt: prompt && { id: prompt.id, digest: prompt.digest },
        outputSchemaDigest: providerContractForRole(name)?.digest,
      });
      for (const executionName of policy.routes) {
        try {
          routes.push({
            executionName,
            modelProfile: config.execution_profiles[executionName]!.model,
            ready: resolveRole(
              config,
              name,
              executionName,
              prompt,
              configDigest,
            ),
          });
        } catch (error) {
          problem =
            error instanceof Error
              ? error.message
              : "Execution route unavailable";
          routes.push({
            executionName,
            modelProfile: config.execution_profiles[executionName]!.model,
          });
        }
      }
      const ready = routes.some((route) => route.ready);
      runtime.roles.set(name, {
        state: ready ? "READY" : "UNAVAILABLE",
        detail: ready ? "Executable ordered routes available" : problem,
        prepared: { role: name, routes, configDigest },
      });
    }
    return runtime;
  }
  get capabilities() {
    return roleNames.map((role) => ({
      name: `model.${role}`,
      state: this.roles.get(role)?.state ?? "NOT_CONFIGURED",
      detail: this.roles.get(role)?.detail ?? "Role has no binding",
    }));
  }
  profile(role: ModelRole) {
    return this.roles.get(role)?.prepared?.routes.find((route) => route.ready)
      ?.ready?.profile;
  }
  requirement(role: ModelRole) {
    return this.configuration?.roles[role]?.requirement ?? "optional";
  }
  /** Owners choose the optional result; required roles and cancellation always propagate. */
  allowDegradation(
    error: unknown,
    signal?: AbortSignal | null,
    role?: ModelRole,
    mandatory = false,
  ) {
    if (signal?.aborted) throw signal.reason ?? error;
    const failedRole = error instanceof GenerationFailure ? error.role : role;
    if (
      mandatory ||
      (failedRole && this.requirement(failedRole) === "required")
    )
      throw ConnectError.from(error, Code.FailedPrecondition);
  }
  prepare(name: ModelRole): PreparedModelRole {
    const value = this.roles.get(name)?.prepared;
    if (!value) throw new GenerationFailure(name, "role_unavailable");
    return value;
  }
  async preparePrompt(
    name: ModelRole,
    path?: string,
    adapter?: string,
  ): Promise<PreparedModelRole> {
    const prepared = this.prepare(name);
    if (!path && !adapter) return prepared;
    const prompt = path
      ? await this.prompts?.load(name, path).catch(() => {
          throw new GenerationFailure(name, "prompt_unavailable");
        })
      : undefined;
    if (path && !prompt)
      throw new GenerationFailure(name, "prompt_unavailable");
    const configDigest = invocationConfigDigest(
      prepared.configDigest,
      prompt,
      adapter,
    );
    return {
      ...prepared,
      configDigest,
      routes: prepared.routes.map((route) => ({
        ...route,
        ready:
          route.ready &&
          bindPrompt(route.ready, prompt ?? route.ready.prompt, configDigest),
      })),
    };
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
    fixed?: PreparedModelRole,
    beforeAttempt?: () => void,
    prepare?: (role: ReadyRole) => Input,
  ) {
    return executeRoutes(
      name,
      fixed ?? this.prepare(name),
      invoke,
      signal,
      beforeAttempt,
      prepare,
    );
  }
  identity(role: ModelRole) {
    return this.roles.get(role)?.prepared?.routes.find((route) => route.ready)
      ?.ready?.configDigest;
  }
  private metadata(role: ReadyRole, adapter: string): ModelProducerMetadata {
    return {
      implementation: adapter,
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
    let snapshot = options.prepared ?? this.prepare(name);
    if (promptRole) {
      const selected =
        typeof promptRole === "string" ? promptRole : promptRole.role;
      const prompt = await this.prompts?.load(
        selected,
        typeof promptRole === "string"
          ? this.configuration.roles[selected]?.prompt
          : promptRole.path,
      );
      if (!prompt) throw new Error("Strategy prompt unavailable");
      const configDigest = invocationConfigDigest(
        snapshot.configDigest,
        prompt,
      );
      snapshot = {
        ...snapshot,
        configDigest,
        routes: snapshot.routes.map((route) => ({
          ...route,
          ready: route.ready && bindPrompt(route.ready, prompt, configDigest),
        })),
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
    fixed?: PreparedModelRole,
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
