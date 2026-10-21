import { createHash, randomBytes } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";
import { Configuration, DefaultApi, problemMiddleware } from "@nous-wave/client";
import { ContextPrepareSchema, ContextViewSchema } from "@nous-wave/contracts/cognition";
import { sameRef, RefSchema, ConsumerSchema, type Ref } from "@nous-wave/contracts/common";
import {
  EncounterAcceptSchema,
  MaterialCaptureSchema,
  MaterialViewSchema,
} from "@nous-wave/contracts/materials";
import {
  NativeInvocationPrepareSchema,
  NativeInvocationCompletionSchema,
  NativeOriginSchema,
  ModelCallViewSchema,
} from "@nous-wave/contracts/models";
import { readReal, parseInstant } from "@nous-wave/platform/time";
import { z } from "zod";
import {
  bindingRead,
  modelBinding,
  callFacts,
  parseResourceIdentity,
  resourceIdentity,
  invocationIdentity,
} from "./native.js";

const directBinding = z.object({
  authorityRef: z.string().regex(/^delegation:[0-9a-f-]{36}$/),
  subjectId: z.string(),
  consumer: ConsumerSchema,
  purpose: z.string().default("worker"),
  audience: z.array(z.string()).default([]),
  billing: z.literal("nous").default("nous"),
});
const binding = z.union([directBinding, z.strictObject({ billing: z.literal("external") })]);
export const configSchema = z.object({
  runtimeId: z.string().min(1),
  baseURL: z.url().optional(),
  tokenEnv: z.string().default("NOUS_SERVICE_TOKEN"),
  installationId: z.string(),
  bindings: z.record(z.string(), binding),
  modelResources: z.record(z.string(), z.string()),
});
type AgentContext = {
  agentId?: string;
  sessionKey?: string;
  parentSessionKey?: string;
  sessionId?: string;
  runId?: string;
};
export type PrepareEvent = {
  traceparent?: string;
  role?: string;
  callId: string;
  runId: string;
  sessionId: string;
  provider: string;
  model: string;
  contextLimit: number;
  outputTokens: number;
  inputTokensEstimate: number;
  transcriptCursor?: string;
  systemPrompt?: string;
  messages: unknown[];
  tools: unknown[];
};
type Prepared = {
  view: z.infer<typeof ContextViewSchema>;
  systemPromptAddition?: string;
  inputRefs?: Ref[];
};
export type AcceptEvent = {
  traceparent?: string;
  role?: string;
  accepted?: { outputRef?: unknown; selectedOutputRefs?: unknown[] };
  callId: string;
  runId: string;
  sessionId: string;
  prepared: { view: unknown };
  outcome?: "not_sent";
  output?: {
    role: string;
    content: unknown[];
    timestamp: number;
    usage?: unknown;
    provider?: string;
    model?: string;
    responseId?: string;
    stopReason?: string;
    diagnostics?: { type: string }[];
    modelContext?: unknown;
  };
  transcriptEntryId?: string;
};
type DispatchEvent = Omit<AcceptEvent, "output" | "transcriptEntryId"> & {
  provider: string;
  model: string;
  inputTokensEstimate: number;
  outputTokens: number;
};
export interface NativePluginApi {
  pluginConfig?: unknown;
  runtime: {
    modelContext?: { version: number; harnesses: readonly string[] };
    modelConfig: {
      readPublishedModelBinding(input: {
        agentId: string;
        provider: string;
        modelId: string;
      }): Promise<{
        agentId: string;
        provider: string;
        modelId: string;
        configRevision: string;
        input: readonly string[];
        reasoning: boolean;
        supportsTools: boolean;
        contextWindow?: number;
        maxTokens?: number;
      }>;
    };
    gateway: { request<T>(method: string, input: Record<string, unknown>): Promise<T> };
  };
  on(
    name: "model_context_prepare",
    handler: (event: PrepareEvent, context: AgentContext) => Promise<Prepared | undefined>,
  ): void;
  on(
    name: "model_context_accept",
    handler: (
      event: AcceptEvent,
      context: AgentContext,
    ) => Promise<{ outputRef: Ref; selectedOutputRefs: Ref[] } | undefined>,
    options?: { priority?: number },
  ): void;
  registerHttpRoute(input: {
    path: string;
    auth: "gateway";
    match: "exact";
    handler(request: IncomingMessage, response: ServerResponse): Promise<boolean>;
  }): void;
  on(
    name: "model_context_dispatch",
    handler: (event: DispatchEvent, context: AgentContext) => Promise<void>,
  ): void;
}

const now = readReal;
const textOf = (value: unknown): string =>
  typeof value === "string" ? value : JSON.stringify(value);
const traceHeader = (event: { runId: string; traceparent?: string }) =>
  event.traceparent ??
  `00-${createHash("sha256").update(event.runId).digest("hex").slice(0, 32)}-${randomBytes(8).toString("hex")}-01`;
function latestNativePortion(messages: unknown[]): string {
  const previousResponse = messages.findLastIndex(
    (message) =>
      typeof message === "object" &&
      message !== null &&
      "role" in message &&
      message.role === "assistant",
  );
  const current = messages.slice(previousResponse + 1);
  const rendered = JSON.stringify(
    current.length ? current : messages.slice(-1),
    function (key, value) {
      if (key === "data" && typeof value === "string" && this.type === "image")
        return "[native image payload]";
      return value;
    },
  );
  return [...rendered].slice(0, 8000).join("");
}
function currentNativeMessages(messages: unknown[]): unknown[] {
  const previousResponse = messages.findLastIndex(
    (message) =>
      typeof message === "object" &&
      message !== null &&
      "role" in message &&
      message.role === "assistant",
  );
  const current = messages.slice(previousResponse + 1);
  return current.length ? current : messages.slice(-1);
}

export function registerNousCognition(api: NativePluginApi) {
  if (api.runtime.modelContext?.version !== 1)
    throw new Error("Nous requires the source-built OpenClaw model context handoff");
  const config = configSchema.parse(api.pluginConfig);
  function sdk(
    event: { callId: string; runId: string; traceparent?: string },
    current: z.infer<typeof directBinding>,
    suffix: string,
    billing = false,
    submissionId = `${event.callId}:${suffix}`,
  ) {
    if (!config.baseURL) throw new Error("Nous Host endpoint is not configured");
    const token = process.env[config.tokenEnv];
    if (!token) throw new Error("Nous Host credential is unavailable");
    return new DefaultApi(
      new Configuration({
        basePath: config.baseURL,
        accessToken: token,
        middleware: [problemMiddleware],
        headers: {
          Authorization: `Bearer ${token}`,
          "idempotency-key": submissionId,
          traceparent: traceHeader(event),
          "x-purpose": current.purpose,
          "x-audience": JSON.stringify(current.audience),
          ...(!billing ? { "x-delegation-authority": current.authorityRef } : {}),
        },
      }),
    );
  }

  async function resolve(context: AgentContext) {
    // Agent identity comes from the native host; model output cannot select it.
    let parent = context.parentSessionKey;
    const visited = new Set<string>();
    while (parent && visited.size < 32 && !visited.has(parent)) {
      visited.add(parent);
      const agentId = /^agent:([^:]+):/.exec(parent)?.[1];
      if (agentId && config.bindings[agentId]) return config.bindings[agentId];
      const result = await api.runtime.gateway.request<{
        session: { parentSessionKey?: string; spawnedBy?: string } | null;
      }>("sessions.describe", { key: parent });
      parent = result.session?.parentSessionKey ?? result.session?.spawnedBy;
    }
    return context.agentId ? config.bindings[context.agentId] : undefined;
  }
  async function nativeScope(facts: z.infer<typeof callFacts>) {
    let key: string | undefined = facts.sessionKey;
    const ancestors: { sessionKey: string; sessionId: string }[] = [],
      visited = new Set<string>();
    let current: z.infer<typeof binding> | undefined;
    while (key && visited.size < 32 && !visited.has(key)) {
      visited.add(key);
      const described: {
        session: { sessionId: string; parentSessionKey?: string; spawnedBy?: string } | null;
      } = await api.runtime.gateway.request("sessions.describe", { key });
      const session: { sessionId: string; parentSessionKey?: string; spawnedBy?: string } | null =
        described.session;
      if (!session || (ancestors.length === 0 && session.sessionId !== facts.currentSessionId))
        throw new Error("Native session scope changed during read");
      ancestors.push({ sessionKey: key, sessionId: session.sessionId });
      const agentId = /^agent:([^:]+):/.exec(key)?.[1];
      if (!current && agentId && config.bindings[agentId]) current = config.bindings[agentId];
      key = session.parentSessionKey ?? session.spawnedBy;
    }
    if (key || !current) throw new Error("Native call has no current Nous consumer binding");
    if (current.billing === "external") throw new Error("Native Task accounting is owned by Hept");
    return {
      subjectId: current.subjectId,
      consumer: current.consumer,
      authorityRef: current.authorityRef,
      purpose: current.purpose,
      audience: current.audience,
      billing: current.billing,
      ancestors,
    };
  }
  async function readNativeCall(body: unknown) {
    const native = NativeOriginSchema.parse(body);
    if (native.runtimeId !== config.runtimeId)
      throw new Error("Native runtime/call identity is not bound to this host");
    invocationIdentity({ runId: native.runId, callId: native.callId });
    const { call } = await api.runtime.gateway.request<{ call: unknown | null }>(
      "sessions.modelCall",
      {
        key: native.sessionKey,
        sessionId: native.sessionId,
        runId: native.runId,
        callId: native.callId,
      },
    );
    if (!call) return null;
    const facts = callFacts.parse(call);
    if (
      facts.sessionId !== native.sessionId ||
      facts.sessionKey !== native.sessionKey ||
      facts.runId !== native.runId ||
      facts.callId !== native.callId
    )
      throw new Error("Native call identity changed");
    if (
      native.entryId &&
      native.entryId !== facts.prepared.entryId &&
      native.entryId !== facts.terminal?.entryId
    )
      throw new Error("Native entry is outside this call");
    return facts;
  }
  async function readCall(body: unknown) {
    const facts = await readNativeCall(body);
    return facts ? { ...facts, scope: await nativeScope(facts) } : null;
  }
  async function readBinding(body: unknown) {
    const input = bindingRead.parse(body);
    if (input.runtimeId !== config.runtimeId)
      throw new Error("Native runtime is not bound to this host");
    const selected = parseResourceIdentity(input.nativeResourceId);
    if (!config.bindings[selected.agentId])
      throw new Error("Native agent has no configured Nous consumer");
    const actual = await api.runtime.modelConfig.readPublishedModelBinding(selected);
    return modelBinding.parse({
      runtimeId: config.runtimeId,
      nativeResourceId: input.nativeResourceId,
      configRevision: actual.configRevision,
      actualProvider: actual.provider,
      actualModelId: actual.modelId,
      kind: "language",
      capabilities: [
        ...actual.input.map((kind) => `input:${kind}`),
        ...(actual.supportsTools ? ["tools"] : []),
        ...(actual.reasoning ? ["reasoning"] : []),
      ],
      contextLimit: actual.contextWindow,
      outputLimit: actual.maxTokens,
    });
  }
  for (const [operation, execute] of Object.entries({ readBinding, readCall }))
    api.registerHttpRoute({
      path: `/plugins/nous/models/${operation}`,
      auth: "gateway",
      match: "exact",
      async handler(request, response) {
        response.setHeader("content-type", "application/json");
        response.setHeader("cache-control", "no-store");
        if (request.method !== "POST") {
          response.statusCode = 405;
          response.end(JSON.stringify({ error: "POST required" }));
          return true;
        }
        try {
          let size = 0;
          const chunks: Buffer[] = [];
          for await (const chunk of request) {
            size += chunk.length;
            if (size > 1_048_576) throw new Error("Native request exceeds its limit");
            chunks.push(Buffer.from(chunk));
          }
          const result = await execute(JSON.parse(Buffer.concat(chunks).toString("utf8")));
          response.end(JSON.stringify(result));
        } catch (error) {
          response.statusCode = error instanceof z.ZodError ? 400 : 409;
          response.end(JSON.stringify({ error: "Native model fact read failed" }));
        }
        return true;
      },
    });
  function origin(
    event: { sessionId: string; runId: string; callId: string },
    context: AgentContext,
  ) {
    if (!context.sessionKey) throw new Error("Native call has no session key");
    return NativeOriginSchema.parse({
      type: "native",
      runtimeId: config.runtimeId,
      sessionId: event.sessionId,
      sessionKey: context.sessionKey,
      runId: event.runId,
      callId: event.callId,
    });
  }
  const source = (
    event: { callId: string; sessionId: string },
    subjectId: string | undefined,
    stage: string,
  ): Ref => ({
    product: "nous",
    installationId: config.installationId,
    subjectId,
    kind: "openclaw.transcript",
    id: `${event.sessionId}:${event.callId}:${stage}`,
  });

  async function cognition(
    event: { callId: string; runId: string; sessionId: string; traceparent?: string },
    context: AgentContext,
    current: z.infer<typeof binding>,
    operation: "capture" | "prepare" | "accept" | "uploadBlob",
    input: unknown,
    suffix: string,
    submissionId = `${event.callId}:${suffix}`,
  ): Promise<unknown> {
    if (current.billing === "external") {
      if (!context.sessionKey) throw new Error("Native cognition requires its actual session key");
      return api.runtime.gateway.request(`hept.cognition.${operation}`, {
        native: {
          runtimeId: config.runtimeId,
          sessionKey: context.sessionKey,
          sessionId: event.sessionId,
          runId: event.runId,
        },
        input,
        submissionId,
        traceparent: traceHeader(event),
      });
    }
    const client = sdk(event, current, suffix, false, submissionId);
    if (operation === "capture")
      return client.materialsCapture({
        subjectId: current.subjectId,
        materialCaptureInput: MaterialCaptureSchema.parse(input),
      });
    if (operation === "prepare")
      return client.contextsPrepare({
        subjectId: current.subjectId,
        contextPrepareInput: ContextPrepareSchema.parse(input),
      });
    if (operation === "accept")
      return client.encountersAccept({
        subjectId: current.subjectId,
        encounterAcceptInput: EncounterAcceptSchema.parse(input),
      });
    const image = z.strictObject({ data: z.string(), mediaType: z.string() }).parse(input);
    return client.materialsUploadBlob(
      {
        subjectId: current.subjectId,
        body: new Blob([Uint8Array.from(Buffer.from(image.data, "base64"))], {
          type: image.mediaType,
        }),
      },
      async ({ init }) => {
        const headers = new Headers(init.headers);
        headers.set("x-content-media-type", image.mediaType);
        return { ...init, headers };
      },
    );
  }

  api.on("model_context_prepare", async (event, context) => {
    const current = await resolve(context);
    if (!current) return undefined;
    const modelResourceId =
      current.billing === "external" && context.agentId
        ? resourceIdentity({
            agentId: context.agentId,
            provider: event.provider,
            modelId: event.model,
          })
        : config.modelResources[`${event.provider}/${event.model}`];
    if (!modelResourceId)
      throw new Error(
        `Register and map the actual model resource ${event.provider}/${event.model}`,
      );
    const inputText = latestNativePortion(event.messages);
    const requiredSources = [
      ...new Map(
        event.messages
          .flatMap((message) => {
            if (!message || typeof message !== "object" || !("modelContext" in message)) return [];
            const metadata = message.modelContext;
            if (
              !metadata ||
              typeof metadata !== "object" ||
              !("inputRefs" in metadata) ||
              !Array.isArray(metadata.inputRefs)
            )
              return [];
            return [
              ...metadata.inputRefs,
              ...("outputRef" in metadata && metadata.outputRef ? [metadata.outputRef] : []),
            ].map((value) => RefSchema.parse(value));
          })
          .map((ref) => [JSON.stringify(ref), ref]),
      ).values(),
    ];
    const origin = source(
      event,
      current.billing === "nous" ? current.subjectId : undefined,
      "input",
    );
    const document: Ref = { ...origin, kind: "openclaw.session", id: event.sessionId };
    const material = MaterialViewSchema.parse(
      await cognition(
        event,
        context,
        current,
        "capture",
        {
          kind: "native_model_input",
          mediaType: "text/plain",
          text: inputText,
          source: {
            ref: origin,
            roots: [document],
            originVersion: event.transcriptCursor ?? event.callId,
          },
        },
        "input",
      ),
    );
    const latestMaterials: z.input<typeof ContextPrepareSchema>["latestMaterials"] = [
      { ref: material.ref, text: inputText, required: true },
    ];
    let imageIndex = 0;
    for (const message of currentNativeMessages(event.messages)) {
      if (
        !message ||
        typeof message !== "object" ||
        !("content" in message) ||
        !Array.isArray(message.content)
      )
        continue;
      for (const part of message.content) {
        if (
          !part ||
          typeof part !== "object" ||
          part.type !== "image" ||
          typeof part.data !== "string" ||
          typeof part.mimeType !== "string"
        )
          continue;
        const bytes = Buffer.from(part.data, "base64");
        const digest = createHash("sha256").update(bytes).digest("hex");
        const blob = z
          .object({ ref: RefSchema })
          .parse(
            await cognition(
              event,
              context,
              current,
              "uploadBlob",
              { data: part.data, mediaType: part.mimeType },
              `image:${digest}`,
              `${event.sessionId}:image:${digest}`,
            ),
          );
        const image = MaterialViewSchema.parse(
          await cognition(
            event,
            context,
            current,
            "capture",
            {
              kind: "native_image_input",
              mediaType: part.mimeType,
              blobRef: blob.ref,
              source: {
                ref: { ...origin, selector: { kind: "part", partId: `image:${imageIndex}` } },
                roots: [document],
                originVersion: event.transcriptCursor ?? event.callId,
              },
            },
            `image:${imageIndex++}`,
          ),
        );
        latestMaterials.push({
          ref: image.ref,
          text: `[Native image: ${part.mimeType}]`,
          selector: { kind: "image", x: 0, y: 0, width: 1, height: 1 },
          required: true,
        });
      }
    }
    const request = ContextPrepareSchema.parse({
      consumer: material.scope,
      role: event.role ?? (current.billing === "nous" ? current.consumer.kind : "worker"),
      roleTask: "",
      situation: "",
      query: inputText.slice(0, 4000),
      latestMaterials,
      requiredSources,
      model: {
        resourceId: modelResourceId,
        modelId: event.model,
        contextLimit: event.contextLimit,
      },
      budget: {
        outputTokens: event.outputTokens,
        toolTokens: 0,
        requiredTokens: Math.max(0, event.inputTokensEstimate - [...inputText].length),
      },
      observed: now(),
    });
    const view = ContextViewSchema.parse(
      await cognition(event, context, current, "prepare", request, "prepare"),
    );
    if (
      view.subjectId !== material.ref.subjectId ||
      JSON.stringify(view.consumer) !== JSON.stringify(material.scope)
    )
      throw new Error("Native cognition scope changed during preparation");
    // Native inputs and role instructions are already present. Only the selected
    // cognition occupies this ephemeral slot; the host applies it after compaction.
    const systemPromptAddition = view.blocks
      .filter((block) => block.kind !== "task" && block.kind !== "sources")
      .map((block) => block.text)
      .join("\n\n");
    return {
      view,
      systemPromptAddition,
      inputRefs: view.inputPortions.map((portion) => ({
        ...portion.ref,
        ...(portion.selector ? { selector: portion.selector } : {}),
      })),
    };
  });

  api.on("model_context_dispatch", async (event, context) => {
    const current = await resolve(context);
    if (!current || current.billing === "external") return;
    const native = origin(event, context),
      facts = await readCall(native);
    if (!facts || facts.scope.subjectId !== current.subjectId)
      throw new Error("Native prepared model call is not available in this consumer");
    const invocationId = invocationIdentity(facts);
    const modelResourceId =
      config.modelResources[`${facts.prepared.provider}/${facts.prepared.modelId}`];
    if (!modelResourceId) throw new Error("Actual native model resource is not mapped");
    await sdk(event, current, "invocation", true).modelInvocationsPrepare({
      subjectId: current.subjectId,
      nativeInvocationPrepareInput: NativeInvocationPrepareSchema.parse({
        invocationId,
        roleId: "native_agent",
        modelResourceId,
        actualProvider: facts.prepared.provider,
        actualModelId: facts.prepared.modelId,
        native,
        inputTokensEstimate: {
          value: facts.prepared.inputTokensEstimate,
          method: "openclaw.native-final-context-estimate",
        },
        maxOutputTokens: facts.prepared.outputTokens,
        inputRefs: facts.prepared.inputRefs,
      }),
    });
    await sdk(event, current, "dispatch", true).modelInvocationsDispatch({
      subjectId: current.subjectId,
      id: invocationId,
      modelInvocationsDispatchRequest: {},
    });
  });

  api.on(
    "model_context_accept",
    async (event, context) => {
      const current = await resolve(context);
      if (!current) return;
      const view = ContextViewSchema.parse(event.prepared.view);
      if (current.billing === "nous" && view.subjectId !== current.subjectId)
        throw new Error("Native context subject binding changed");
      const invocationId = invocationIdentity(event);
      if (event.outcome === "not_sent") {
        if (current.billing === "nous") {
          try {
            await sdk(event, current, "completion", true).modelInvocationsComplete({
              subjectId: current.subjectId,
              id: invocationId,
              nativeInvocationCompletionInput: NativeInvocationCompletionSchema.parse({
                invocationId,
                status: "not_sent",
              }),
            });
          } catch (error) {
            if (
              !(
                error &&
                typeof error === "object" &&
                "code" in error &&
                error.code === "models.invocation_not_found"
              )
            )
              throw error;
          }
        }
        return;
      }
      if (!event.output) throw new Error("Native completed model output is missing");
      const facts = await readNativeCall(origin(event, context));
      if (!facts) throw new Error("Native original model call is not present");
      const usage = facts.terminal?.usage;
      const succeeded =
        !!facts.terminal &&
        !facts.terminal.providerRefused &&
        !["error", "aborted"].includes(facts.terminal.stopReason ?? "");
      const status = succeeded
        ? "completed"
        : facts.terminal?.providerRefused
          ? "not_sent"
          : "unknown";
      const completion = NativeInvocationCompletionSchema.parse({
        invocationId,
        status,
        usage: usage
          ? {
              inputTokens: usage.input + usage.cacheRead + usage.cacheWrite,
              outputTokens: usage.output,
              cacheReadTokens: usage.cacheRead,
              cacheWriteTokens: usage.cacheWrite,
            }
          : undefined,
        rawUsage: usage,
        providerRequestId: facts.terminal?.providerRequestId,
      });
      let modelCall: z.infer<typeof ModelCallViewSchema> | undefined;
      if (current.billing === "nous")
        modelCall = ModelCallViewSchema.parse(
          await sdk(event, current, "completion:fact", true).modelInvocationsComplete({
            subjectId: current.subjectId,
            id: invocationId,
            nativeInvocationCompletionInput: completion,
          }),
        );
      const outputOrigin = source(event, view.subjectId, "output");
      const { modelContext: _nativeMetadata, ...providerOutput } = event.output;
      const output = MaterialViewSchema.parse(
        await cognition(
          event,
          context,
          current,
          "capture",
          {
            kind: "native_model_output",
            mediaType: "application/json",
            text: textOf(providerOutput),
            source: {
              ref: outputOrigin,
              roots: [],
              inputs: view.inputPortions.map((portion) => ({
                ...portion.ref,
                ...(portion.selector ? { selector: portion.selector } : {}),
              })),
              originVersion: event.callId,
            },
          },
          "output",
        ),
      );
      const selectedOutputRefs = event.output.content.flatMap((part, index) =>
        part &&
        typeof part === "object" &&
        "type" in part &&
        part.type === "text" &&
        "text" in part &&
        typeof part.text === "string" &&
        part.text.trim()
          ? [
              {
                ...output.ref,
                selector: { kind: "node" as const, pointer: `/content/${index}/text` },
              },
            ]
          : [],
      );
      if (!succeeded) return { outputRef: output.ref, selectedOutputRefs };
      const request = EncounterAcceptSchema.parse({
        consumer: view.consumer,
        invocationId,
        step: 0,
        contextViewRef: view.ref,
        portions: [
          ...view.inputPortions.map((part) => ({
            ref: { ...part.ref, ...(part.selector ? { selector: part.selector } : {}) },
          })),
        ],
        resultRef: output.ref,
        observed: modelCall?.observed?.point ?? view.observed,
        occurred: {
          start: {
            domainId: "real",
            at: parseInstant(new Date(event.output.timestamp).toISOString()),
          },
        },
        purpose: current.billing === "nous" ? current.purpose : "worker",
        completion: "complete",
      });
      await cognition(event, context, current, "accept", request, "accept");
      return { outputRef: output.ref, selectedOutputRefs };
    },
    { priority: 100 },
  );
  api.on(
    "model_context_accept",
    async (event, context) => {
      const current = await resolve(context);
      if (!current || current.billing !== "nous" || !event.accepted?.outputRef) return undefined;
      const facts = await readCall(origin(event, context));
      if (
        !facts?.terminal ||
        facts.terminal.providerRefused ||
        ["error", "aborted"].includes(facts.terminal.stopReason ?? "")
      )
        return undefined;
      const outputRef = RefSchema.parse(event.accepted.outputRef);
      if (
        !facts.terminal.outputRef ||
        !sameRef(RefSchema.parse(facts.terminal.outputRef), outputRef)
      )
        throw new Error("Native output reference is not durably accepted");
      const usage = facts.terminal.usage,
        invocationId = invocationIdentity(facts);
      await sdk(event, current, "completion:adoption", true).modelInvocationsComplete({
        subjectId: current.subjectId,
        id: invocationId,
        nativeInvocationCompletionInput: NativeInvocationCompletionSchema.parse({
          invocationId,
          status: "completed",
          outputRef,
          usage: usage
            ? {
                inputTokens: usage.input + usage.cacheRead + usage.cacheWrite,
                outputTokens: usage.output,
                cacheReadTokens: usage.cacheRead,
                cacheWriteTokens: usage.cacheWrite,
              }
            : undefined,
          rawUsage: usage,
          providerRequestId: facts.terminal.providerRequestId,
        }),
      });
      return undefined;
    },
    { priority: 0 },
  );
}

export default {
  id: "nous-cognition",
  name: "Nous Cognition",
  description: "Prepare and accept cognition at each native model boundary",
  register: registerNousCognition,
};
