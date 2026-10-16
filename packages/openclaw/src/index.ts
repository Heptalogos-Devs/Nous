// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { toJson } from "@bufbuild/protobuf";
import {
  standaloneSourceConfiguration,
  standaloneUsage,
} from "./standalone-source.js";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { connectNousInstance } from "@nous-wave/client/node";
import {
  withAcceptedContactRoots,
  decodeAcceptedObservation,
} from "@nous-wave/client/host-bridge";
import { createManagedBinding, eventIdentity } from "@nous-wave/client/managed";
import { CognitionUsageSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  delegateCompactionToRuntime,
  definePluginEntry,
  buildJsonPluginConfigSchema,
  type OpenClawPluginApi,
} from "openclaw/plugin-sdk/core";
import { z } from "zod";
import type { Context } from "@openclaw/ai";
import type {
  ManagedContextEngine,
  ManagedModelInput,
  ManagedModelApplication,
} from "./contract.js";
import { cognitionRenderer } from "./render.js";
import { HostBridge, hostBridgeConfiguration, occurrenceJson } from "./host.js";
import { hostedReadonlyTools } from "./readonly-tools.js";
import { HostedLifecycle } from "./hosted.js";
import {
  loadTranscriptEventsSync,
  resolveStorePath,
} from "openclaw/plugin-sdk/session-store-runtime";

const namespace = "nous-cognition";
const sessionBinding = z
  .object({
    subjectId: z.uuid(),
    nativeSessionId: z.string().min(1),
    nativeHistoryGeneration: z.string().min(1),
    nousSessionId: z.uuid(),
  })
  .strict();
const configuration = z
  .object({
    runRoot: z.string().min(1),
    subjectId: z.uuid(),
    consumerId: z.string().min(1),
    workContextId: z.uuid().optional(),
    hostBridge: hostBridgeConfiguration.optional(),
    sourceRead: standaloneSourceConfiguration.optional(),
    cognitionTokenCap: z.number().int().min(512).default(4096),
  })
  .strict();

function register(api: OpenClawPluginApi): void {
  const config = configuration.parse(api.pluginConfig);
  const connect = () => connectNousInstance({ runRoot: config.runRoot });
  const bridge = config.hostBridge
    ? new HostBridge(config.hostBridge)
    : undefined;
  api.session.state.registerSessionExtension({
    namespace,
    description:
      "Stable Nous Subject/Session binding; no cognitive content or cursor is persisted here.",
  });
  if (config.hostBridge)
    api.registerTool(() => hostedReadonlyTools(), {
      names: ["nous_query", "nous_read"],
    });
  api.registerContextEngine(namespace, () => {
    const workers = new Map<
      string,
      Promise<ReturnType<typeof createManagedBinding>>
    >();
    const hosted = bridge
      ? new HostedLifecycle(
          bridge,
          config.subjectId,
          connect,
          (message) => api.logger.error(message),
          config.consumerId,
        )
      : undefined;
    const flushNativeHistory = async (
      sessionId: string,
      sessionKey: string,
      agentId?: string,
      signal?: AbortSignal,
    ) => {
      if (!hosted) return;
      const events = loadTranscriptEventsSync({
        sessionId,
        sessionKey,
        agentId,
        storePath: resolveStorePath(api.config.session?.store, { agentId }),
      });
      await hosted.flush(
        events.flatMap((value) =>
          value &&
          typeof value === "object" &&
          "type" in value &&
          value.type === "message" &&
          "message" in value
            ? [value.message]
            : [],
        ),
        signal,
      );
    };
    let invocation = 0;
    const observedEvents = new Map<
      string,
      Awaited<
        ReturnType<
          Awaited<ReturnType<typeof connect>>["runtime"]["recordObservation"]
        >
      >
    >();
    const worker = (input: ManagedModelInput) => {
      const key = `${input.sessionId}:${input.runId}`;
      let pending = workers.get(key);
      if (!pending) {
        pending = (async () => {
          const connected = await connect();
          const scope = input.sessionKey
            ? { agentId: input.agentId, sessionKey: input.sessionKey }
            : undefined;
          const entry =
            scope &&
            api.runtime.agent.session.getSessionEntry({
              ...scope,
              readConsistency: "latest",
            });
          if (!scope || !entry)
            throw new Error(
              "Required managed native Session binding is unavailable; use a persisted native Session",
            );
          const parsed = sessionBinding.safeParse(
            entry.pluginExtensions?.[namespace]?.[namespace],
          );
          const saved = parsed.success ? parsed.data : undefined;
          let sessionId =
            saved?.subjectId === config.subjectId &&
            saved.nativeSessionId === input.sessionId &&
            saved.nativeHistoryGeneration === input.historyGeneration &&
            typeof saved.nousSessionId === "string"
              ? saved.nousSessionId
              : undefined;
          if (sessionId) {
            const session = await connected.runtime.getSession(
              { subjectId: config.subjectId, id: sessionId },
              { signal: input.signal },
            );
            if (session.closedAt)
              throw new Error(
                "Bound Nous Session is closed; reset the native session explicitly",
              );
          } else {
            if (!input.historyIsFresh)
              throw new Error(
                "Existing native history has no valid durable Nous binding; reset the native session before model submission",
              );
            if (input.parentSessionKey)
              throw new Error(
                "Delegated native input has no prepared durable Nous handoff binding",
              );
            const session = await connected.runtime.openSession(
              { subjectId: config.subjectId },
              { signal: input.signal },
            );
            sessionId = session.sessionId;
            await input.persistSessionBinding({
              subjectId: config.subjectId,
              nativeSessionId: input.sessionId,
              nativeHistoryGeneration: input.historyGeneration,
              nousSessionId: sessionId,
            });
            const written = api.runtime.agent.session.getSessionEntry({
              ...scope,
              readConsistency: "latest",
            });
            const confirmed = sessionBinding.safeParse(
              written?.pluginExtensions?.[namespace]?.[namespace],
            );
            if (
              !confirmed.success ||
              confirmed.data.nousSessionId !== sessionId ||
              confirmed.data.nativeHistoryGeneration !== input.historyGeneration
            )
              throw new Error(
                "Native Nous Session binding was not durably written",
              );
          }
          if (!hosted && config.workContextId) {
            const session = await connected.runtime.getSession(
              { subjectId: config.subjectId, id: sessionId },
              { signal: input.signal },
            );
            if (session.activeWorkContextId !== config.workContextId)
              await connected.runtime.setActiveWorkContext(
                {
                  subjectId: config.subjectId,
                  sessionId,
                  workContextId: config.workContextId,
                  expectedRuntimeRevision: session.runtimeRevision,
                  operationId: await eventIdentity(
                    `${input.sessionId}:${sessionId}:${config.workContextId}:${session.runtimeRevision}`,
                  ),
                },
                { signal: input.signal },
              );
          }
          return createManagedBinding({
            client: connect,
            subjectId: config.subjectId,
            sessionId,
            consumerId: config.consumerId,
          });
        })();
        workers.set(key, pending);
      }
      return pending;
    };

    const observe = async (
      input: ManagedModelInput,
      message: Context["messages"][number],
    ) => {
      if (hosted) return hosted.observe(input, message);
      const connected = await connect();
      const binding = await worker(input);
      const content =
        typeof message.content === "string"
          ? message.content
          : message.content
              .filter((part) => part.type === "text")
              .map((part) => (part.type === "text" ? part.text : ""))
              .join("\n");
      const material =
        message.role === "toolResult"
          ? JSON.stringify({
              content: message.content,
              details: message.details ?? null,
              isError: message.isError,
            })
          : content;
      if (!material) return;
      const key = `${input.sessionId}:${message.role}:${message.timestamp}:${"toolCallId" in message ? message.toolCallId : content}`;
      const previous = observedEvents.get(key);
      if (previous) return previous;
      const receipt = await connected.runtime.recordObservation(
        {
          subjectId: config.subjectId,
          sessionId: binding.sessionId,
          requestId: await eventIdentity(key),
          sourceClass:
            message.role === "toolResult"
              ? "tool"
              : message.role === "assistant"
                ? "generated"
                : "message",
          externalObjectRef: `object:openclaw:${await eventIdentity(key)}`,
          conversationRef: input.sessionKey,
          observedAt: timestampFromDate(new Date(message.timestamp)),
          occurredTime: {
            value: {
              case: "instant",
              value: timestampFromDate(new Date(message.timestamp)),
            },
          },
          material: {
            case: "inlineText",
            value: {
              text: material,
              mediaType:
                message.role === "toolResult"
                  ? "application/json"
                  : "text/plain",
            },
          },
          admit: message.role === "toolResult",
          context: {
            ...(config.sourceRead
              ? {
                  cognition_usage: toJson(
                    CognitionUsageSchema,
                    standaloneUsage(
                      input,
                      config.subjectId,
                      config.sourceRead,
                      true,
                    ),
                  ),
                }
              : {}),
            native_session_id: input.sessionId,
            role: message.role,
            audience_visibility: input.audience.visibility,
            ...(input.audience.channel
              ? { channel: input.audience.channel }
              : {}),
            ...(input.audience.senderId
              ? { unresolved_sender: input.audience.senderId }
              : {}),
            ...(input.audience.destination
              ? { destination: input.audience.destination }
              : {}),
            ...(message.role === "toolResult"
              ? {
                  tool_name: message.toolName,
                  tool_call_id: message.toolCallId,
                  is_error: message.isError,
                }
              : {}),
            ...(message.role === "assistant"
              ? { delivery: "model_output" }
              : {}),
          },
        },
        { signal: input.signal },
      );
      observedEvents.set(key, receipt);
      if (observedEvents.size > 4096)
        observedEvents.delete(observedEvents.keys().next().value!);
      return receipt;
    };

    const engine: ManagedContextEngine = {
      info: {
        id: namespace,
        name: "Nous Cognition",
        version: "1",
        ownsCompaction: false,
        managedModelInputRequired: true,
        transcriptPersistence: "host",
      },
      async ingest() {
        return { ingested: false };
      },
      async assemble(params) {
        return {
          messages: params.messages,
          estimatedTokens: 0,
        };
      },
      async compact(params) {
        await flushNativeHistory(
          params.sessionId,
          params.sessionKey,
          params.agentId,
          params.abortSignal,
        );
        const entry = api.runtime.agent.session.getSessionEntry({
          sessionKey: params.sessionKey,
          agentId: params.agentId,
          readConsistency: "latest",
        });
        const parsed = sessionBinding.safeParse(
          entry?.pluginExtensions?.[namespace]?.[namespace],
        );
        if (
          !parsed.success ||
          parsed.data.subjectId !== config.subjectId ||
          parsed.data.nativeSessionId !== params.sessionId
        )
          throw new Error(
            "Native compaction has no current durable Nous binding",
          );
        const checked = await (
          await connect()
        ).runtime.reportUse(
          {
            subjectId: config.subjectId,
            sessionId: parsed.data.nousSessionId,
            consumerRef: config.consumerId,
            modelInput: true,
            events: [],
          },
          { signal: params.abortSignal },
        );
        if (!checked.modelInputChecked)
          throw new Error(
            "Required native compaction history check was not enforced",
          );
        return delegateCompactionToRuntime(params);
      },
      captureNativeEvent(input, message, application, outcome) {
        hosted?.capture(input, message, application, outcome);
      },
      async recordNativeEvent(input, message) {
        await observe(input, message);
      },
      async prepareSubagentSpawn(params) {
        if (
          params.contextMode !== "isolated" ||
          typeof params.parentRunId !== "string" ||
          !params.parentRunId ||
          typeof params.persistSessionBinding !== "function"
        )
          throw new Error(
            "Required native isolated cognition handoff control is unavailable",
          );
        const parent = api.runtime.agent.session.getSessionEntry({
          sessionKey: params.parentSessionKey,
          readConsistency: "latest",
        });
        const child = api.runtime.agent.session.getSessionEntry({
          sessionKey: params.childSessionKey,
          readConsistency: "latest",
        });
        const parsed = sessionBinding.safeParse(
          parent?.pluginExtensions?.[namespace]?.[namespace],
        );
        if (
          !parsed.success ||
          parsed.data.subjectId !== config.subjectId ||
          parsed.data.nativeSessionId !== parent?.sessionId ||
          !child?.sessionId
        )
          throw new Error(
            "Native handoff has no current bound parent or child generation",
          );
        const connected = await connect();
        const session = await connected.runtime.openSession({
          subjectId: config.subjectId,
          parentSessionId: parsed.data.nousSessionId,
        });
        try {
          if (bridge)
            await bridge.handoff({
              version: 1,
              requestId: await eventIdentity(
                JSON.stringify([
                  params.parentSessionKey,
                  params.parentRunId,
                  params.childSessionKey,
                  child.sessionId,
                  "handoff",
                ]),
              ),
              subjectId: config.subjectId,
              parentRunId: params.parentRunId,
              parentSessionKey: params.parentSessionKey,
              parentSessionId: parsed.data.nativeSessionId,
              parentNousSessionId: parsed.data.nousSessionId,
              childSessionKey: params.childSessionKey,
              childSessionId: child.sessionId,
              childNousSessionId: session.sessionId,
              contextMode: "isolated",
            });
          await params.persistSessionBinding({
            subjectId: config.subjectId,
            nativeSessionId: child.sessionId,
            nativeHistoryGeneration: child.sessionId,
            nousSessionId: session.sessionId,
          });
        } catch (error) {
          await connected.runtime.closeSession({
            subjectId: config.subjectId,
            id: session.sessionId,
          });
          throw error;
        }
        return {
          rollback: async () => {
            await connected.runtime.closeSession({
              subjectId: config.subjectId,
              id: session.sessionId,
            });
          },
        };
      },
      async authorizeToolExecution(input, application, execution) {
        input.signal.throwIfAborted();
        if (input.boundaryVersion !== 4)
          throw new Error(
            "Required native tool execution boundary is unavailable",
          );
        if (hosted)
          return hosted.authorizeToolExecution(input, application, execution);
        return { run: async (implementation) => await implementation() };
      },
      async prepareModelInput(input) {
        if (
          input.boundaryVersion !== 4 ||
          typeof input.historyGeneration !== "string" ||
          !input.historyGeneration ||
          typeof input.historyIsFresh !== "boolean" ||
          typeof input.persistSessionBinding !== "function"
        )
          throw new Error(
            "Required native history admission boundary is unavailable; prepare the controlled OpenClaw distribution",
          );
        input.signal.throwIfAborted();
        const binding = await worker(input);
        const invocationId = `${input.runId}:nous:${++invocation}`;
        if (hosted)
          await flushNativeHistory(
            input.sessionId,
            input.sessionKey!,
            input.agentId,
            input.signal,
          );
        const permission = await hosted?.prepare(
          input,
          binding.sessionId,
          invocationId,
        );
        const occurrences: {
          contactRef: string;
          receipt: ReturnType<typeof occurrenceJson>;
        }[] = [];
        if (permission) {
          const connected = await connect();
          const session = await connected.runtime.getSession(
            { subjectId: config.subjectId, id: binding.sessionId },
            { signal: input.signal },
          );
          if (session.activeWorkContextId !== permission.response.workContextId)
            await connected.runtime.setActiveWorkContext(
              {
                subjectId: config.subjectId,
                sessionId: binding.sessionId,
                workContextId: permission.response.workContextId,
                expectedRuntimeRevision: session.runtimeRevision,
                operationId: await eventIdentity(
                  `${permission.identity.contactRequestId}:work-context:${session.runtimeRevision}`,
                ),
              },
              { signal: input.signal },
            );
          for (const contact of permission.contacts)
            occurrences.push({
              contactRef: contact.contactRef,
              receipt: occurrenceJson(
                await connected.runtime.recordObservation(contact.observation, {
                  signal: input.signal,
                }),
              ),
            });
        }
        if (permission)
          permission.usage = withAcceptedContactRoots(
            permission.usage,
            config.subjectId,
            occurrences.map((entry) =>
              decodeAcceptedObservation(entry.receipt),
            ),
          );
        const currentTurn = input.context.messages.findLastIndex(
          (message) => message.role === "user",
        );
        const standaloneReceipts = [];
        for (const message of input.context.messages.slice(
          Math.max(0, currentTurn),
        )) {
          if (!hosted) {
            const receipt = await observe(input, message);
            if (receipt) standaloneReceipts.push(receipt);
          }
        }
        const modelContext = permission
          ? {
              ...input.context,
              tools: input.context.tools?.filter((tool) =>
                permission.response.allowedToolNames.includes(tool.name),
              ),
            }
          : input.context;
        const reservedOutput = input.reservedOutputTokens;
        const contextWindow = input.contextTokenBudget;
        if (!contextWindow)
          throw new Error(
            "Managed invocation requires the selected model's context window",
          );
        const available =
          contextWindow - reservedOutput - input.measureContext(modelContext);
        const budget = Math.min(config.cognitionTokenCap, available);
        const usage =
          permission?.usage ??
          withAcceptedContactRoots(
            standaloneUsage(input, config.subjectId, config.sourceRead),
            config.subjectId,
            standaloneReceipts,
          );
        const prepared = await binding.prepare({
          usage,
          workContextId:
            permission?.response.workContextId ?? config.workContextId,
          cognitionTokenBudget: budget,
          signal: input.signal,
        });
        const { input: context, applied } = binding.apply(
          modelContext,
          prepared,
          {
            render(base, segments, invocationUsage) {
              const rendered = cognitionRenderer.render(
                base,
                segments,
                invocationUsage,
              );
              if (
                input.measureContext(rendered) + reservedOutput >
                contextWindow
              )
                throw new Error(
                  "Complete model input exceeds the native context budget",
                );
              return rendered;
            },
          },
        );
        const application: ManagedModelApplication = {
          context,
          applied,
          invocationId,
        };
        await binding.admit(
          {
            invocationId: application.invocationId,
            applied,
            occurredAt: new Date(),
          },
          { signal: input.signal },
        );
        if (hosted && permission)
          await hosted.admitted(
            permission,
            prepared.projection!,
            occurrences,
            input.signal,
          );
        input.signal.throwIfAborted();
        api.logger.info(
          `Nous applied invocation=${application.invocationId} kind=${prepared.kind} projection=${applied.projectionId} sources=${applied.sourceRefs.length} budgetEstimate=openclaw_native`,
        );
        return application;
      },
      async reportModelOutcome(input, application, outcome, message) {
        if (hosted) return hosted.report(input, application, outcome, message);
        await (
          await worker(input)
        ).report(
          {
            invocationId: application.invocationId,
            applied: application.applied,
            outcome,
            occurredAt: new Date(message?.timestamp ?? Date.now()),
          },
          { signal: input.signal.aborted ? undefined : input.signal },
        );
        if (message && !input.signal.aborted) await observe(input, message);
      },
      async dispose() {
        await Promise.all(
          [...workers.values()].map(async (pending) => (await pending).close()),
        );
        workers.clear();
        observedEvents.clear();
      },
    };
    return engine;
  });
}

export default definePluginEntry({
  id: namespace,
  name: "Nous Cognition",
  description:
    "Apply the bound Subject's cognition at every native model invocation.",
  kind: "context-engine",
  configSchema: () =>
    buildJsonPluginConfigSchema(z.toJSONSchema(configuration)),
  register,
});
