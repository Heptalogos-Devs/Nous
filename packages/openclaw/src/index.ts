// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { connectNousInstance } from "@nous-wave/client/node";
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

const namespace = "nous-cognition";
const configuration = z
  .object({
    runRoot: z.string().min(1),
    subjectId: z.uuid(),
    consumerId: z.string().min(1),
    workContextId: z.uuid().optional(),
    cognitionTokenCap: z.number().int().min(512).default(4096),
  })
  .strict();

function register(api: OpenClawPluginApi): void {
  const config = configuration.parse(api.pluginConfig);
  const connect = () => connectNousInstance({ runRoot: config.runRoot });
  api.session.state.registerSessionExtension({
    namespace,
    description:
      "Stable Nous Subject/Session binding; no cognitive content or cursor is persisted here.",
  });
  api.registerContextEngine(namespace, () => {
    const workers = new Map<
      string,
      Promise<ReturnType<typeof createManagedBinding>>
    >();
    let invocation = 0;
    const observedEvents = new Set<string>();
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
            scope && api.runtime.agent.session.getSessionEntry(scope);
          const saved = entry?.pluginExtensions?.[namespace];
          let sessionId =
            saved?.subjectId === config.subjectId &&
            saved.nativeSessionId === input.sessionId &&
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
            const session = await connected.runtime.openSession(
              { subjectId: config.subjectId },
              { signal: input.signal },
            );
            sessionId = session.sessionId;
            if (scope && entry)
              await api.runtime.agent.session.patchSessionEntry({
                ...scope,
                preserveActivity: true,
                update: (latest) => {
                  if (latest.sessionId !== input.sessionId)
                    throw new Error(
                      "Native Session changed during Nous binding",
                    );
                  return {
                    pluginExtensions: {
                      ...latest.pluginExtensions,
                      [namespace]: {
                        subjectId: config.subjectId,
                        nativeSessionId: input.sessionId,
                        nousSessionId: sessionId!,
                      },
                    },
                  };
                },
              });
          }
          if (config.workContextId) {
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
      if (observedEvents.has(key)) return;
      await connected.runtime.recordObservation(
        {
          subjectId: config.subjectId,
          sessionId: binding.sessionId,
          requestId: await eventIdentity(key),
          sourceClass:
            message.role === "toolResult"
              ? "tool"
              : message.role === "assistant"
                ? "generation"
                : "conversation",
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
      observedEvents.add(key);
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
        return delegateCompactionToRuntime(params);
      },
      recordNativeEvent: observe,
      async prepareModelInput(input) {
        input.signal.throwIfAborted();
        const binding = await worker(input);
        const currentTurn = input.context.messages.findLastIndex(
          (message) => message.role === "user",
        );
        for (const message of input.context.messages.slice(
          Math.max(0, currentTurn),
        )) {
          await observe(input, message);
        }
        const reservedOutput = input.reservedOutputTokens;
        const contextWindow = input.contextTokenBudget;
        if (!contextWindow)
          throw new Error(
            "Managed invocation requires the selected model's context window",
          );
        const available =
          contextWindow - reservedOutput - input.measureContext(input.context);
        const budget = Math.min(config.cognitionTokenCap, available);
        const usage = create(CognitionUsageSchema, {
          agentInstanceId: `${input.sessionId}:${input.runId}`,
          purpose: input.sessionKey?.includes(":subagent:")
            ? "worker"
            : "deliberation",
          intent:
            input.prompt.trim() ||
            "Continue the current work using the actual tool results.",
          audience: {
            conversationRef: input.sessionKey ?? input.sessionId,
            visibility: input.audience.visibility,
          },
        });
        const prepared = await binding.prepare({
          usage,
          workContextId: config.workContextId,
          cognitionTokenBudget: budget,
          signal: input.signal,
        });
        const { input: context, applied } = binding.apply(
          input.context,
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
          invocationId: `${input.runId}:nous:${++invocation}`,
        };
        api.logger.info(
          `Nous applied invocation=${application.invocationId} kind=${prepared.kind} projection=${applied.projectionId} sources=${applied.sourceRefs.length} budgetEstimate=openclaw_native`,
        );
        return application;
      },
      async reportModelOutcome(input, application, outcome, message) {
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
