// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
/** Deterministic owner for the pinned native terminal-custody fixture. */
import { appendFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import assert from "node:assert/strict";
import { z } from "zod";
import {
  retryNousSubagentCompletion,
  type OpenClawPluginApi,
} from "openclaw/plugin-sdk/core";
import type { ManagedContextEngine } from "../src/contract.js";
const proof = process.env.NOUS_COMPLETION_FIXTURE_PROOF;
const parent = process.env.NOUS_COMPLETION_FIXTURE_PARENT;
assert(proof && parent);
const log = (event: string, fields: Record<string, unknown> = {}) =>
  appendFileSync(proof, JSON.stringify({ event, ...fields }) + "\n");
const handoffs = new Map<string, string>();
const registered = new Set([parent]);
const admitted = new Set<string>();
let counter = 0;
const engine: ManagedContextEngine = {
  info: {
    id: "nous-cognition",
    name: "Native completion fixture",
    version: "1",
    ownsCompaction: false,
    managedModelInputRequired: true,
    transcriptPersistence: "host",
  },
  async ingest() {
    return { ingested: false };
  },
  async assemble(p) {
    return { messages: p.messages, estimatedTokens: 0 };
  },
  async compact() {
    throw Error("Fixture must not compact");
  },
  async prepareSubagentSpawn(p) {
    if (!registered.has(p.parentRunId)) throw Error("Unregistered parent");
    handoffs.set(p.childSessionKey, p.parentRunId);
    await p.persistSessionBinding({
      parentNativeRunId: p.parentRunId,
      parentNativeSessionKey: p.parentSessionKey,
    });
    log("handoff", {
      parentRunId: p.parentRunId,
      childSessionKey: p.childSessionKey,
    });
    return { async rollback() {} };
  },
  async prepareSubagentCompletion(p) {
    const original = handoffs.get(p.childSessionKey);
    if (!original || (p.parentRunId && p.parentRunId !== original))
      throw Error("Exact child handoff unavailable");
    log("terminal", { ...p });
    return { parentRunId: original, resultRetained: true };
  },
  async prepareModelInput(p) {
    if (!registered.has(p.runId)) {
      if (!handoffs.has(p.sessionKey ?? ""))
        throw Error("Unregistered actual input");
      registered.add(p.runId);
    }
    admitted.add(p.runId);
    log("admitted", {
      runId: p.runId,
      prompt: p.prompt,
      sessionKey: p.sessionKey,
    });
    return {
      context: p.context,
      invocationId: p.runId + ":fixture:" + ++counter,
      applied: { projectionId: randomUUID(), sourceRefs: [] },
    };
  },
  async authorizeToolExecution() {
    return {
      async run(fn) {
        return await fn();
      },
    };
  },
  captureNativeEvent(p, m, a, o) {
    if (m.role === "user") return;
    const marked = m as typeof m & { nousHostBridge?: unknown };
    marked.nousHostBridge = {
      identity: {
        version: 1,
        contactRequestId: randomUUID(),
        subjectId: "11111111-1111-4111-8111-111111111111",
        nousSessionId: "22222222-2222-4222-8222-222222222222",
        invocationId: a?.invocationId ?? "fixture-event",
        native: {
          runId: p.runId,
          sessionKey: p.sessionKey,
          sessionId: p.sessionId,
          agentId: p.agentId,
          historyGeneration: p.historyGeneration,
        },
      },
      observedAt: new Date().toISOString(),
      ...(o ? { outcome: o, projectionId: a?.applied.projectionId } : {}),
    };
  },
  async recordNativeEvent() {},
  async reportModelOutcome(p, a, o, m) {
    if (m) engine.captureNativeEvent(p, m, a, o);
  },
  async afterTurn() {},
  async dispose() {},
};
export default {
  id: "nous-cognition",
  name: "Native completion fixture",
  register(api: OpenClawPluginApi) {
    api.session.state.registerSessionExtension({
      namespace: "nous-cognition",
      description: "Fixture exact native handoff",
    });
    api.registerContextEngine("nous-cognition", () => engine);
    api.registerTool(
      () => ({
        name: "fixture_read",
        label: "Read fixture",
        description: "Read one synthetic saved official result",
        parameters: {
          type: "object",
          properties: {},
          additionalProperties: false,
        },
        async execute(id) {
          log("worker-tool", { toolCallId: id });
          return {
            content: [{ type: "text", text: "SYNTHETIC_OFFICIAL_TITLE" }],
            details: { source: "fixture-only" },
          };
        },
      }),
      { names: ["fixture_read"] },
    );
    api.registerGatewayMethod(
      "fixture.releaseCompletion",
      async ({ params, respond }) => {
        try {
          const exact = z
            .object({
              childRunId: z.string(),
              childSessionKey: z.string(),
              expectedEndedAt: z.number(),
            })
            .strict()
            .parse(params);
          respond(true, await retryNousSubagentCompletion(exact));
        } catch (error) {
          respond(false, undefined, {
            code: "INVALID_REQUEST",
            message: String(error),
          });
        }
      },
      { scope: "operator.admin" },
    );
    api.registerChannel({
      plugin: {
        id: "completion-fixture",
        meta: {
          id: "completion-fixture",
          label: "Completion fixture",
          selectionLabel: "Fixture",
          docsPath: "/fixture",
          blurb: "No QQ traffic",
        },
        capabilities: { chatTypes: ["direct", "group"], blockStreaming: false },
        config: {
          listAccountIds: () => ["fixture-account"],
          defaultAccountId: () => "fixture-account",
          resolveAccount: () => ({
            accountId: "fixture-account",
            enabled: true,
            configured: true,
          }),
          isEnabled: () => true,
          isConfigured: () => true,
        },
        outbound: {
          deliveryMode: "direct",
          textChunkLimit: 4096,
          resolveTarget: ({ to }) =>
            to
              ? { ok: true, to }
              : { ok: false, error: new Error("Missing fixture target") },
        },
        message: {
          durableFinal: {
            capabilities: { text: true },
            automaticUnknownSendReconciliation: false,
          },
          deliveryCustody: {
            async adopt(p) {
              if (!p.runId || !admitted.has(p.runId))
                throw Error("Producer not Host admitted");
              log("custody", {
                runId: p.runId,
              });
              return { reference: "fixture-behavior:" + p.runId };
            },
            async authorize(p) {
              if (!p.runId || !admitted.has(p.runId))
                throw Error("Producer not admitted");
            },
          },
          send: {
            async text(p) {
              log("sent", { text: p.text, sourceIndex: p.sourceIndex });
              throw Error(
                "Terminal result custody must never reach the native sender",
              );
            },
          },
        },
      },
    });
  },
};
