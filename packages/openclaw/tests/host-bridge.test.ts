// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create, toJson } from "@bufbuild/protobuf";
import { timestampFromDate, timestampDate } from "@bufbuild/protobuf/wkt";
import {
  CognitionUsageSchema,
  ObservationInputSchema,
  AcceptedObservationSchema,
  ManagedContextResponseSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  hostPrepareRequestSchema,
  hostAdmittedRequestSchema,
  decodeProjection,
  hostFeedbackRequestSchema,
  hostHandoffRequestSchema,
  HOST_BRIDGE_PATHS,
  type HostPrepareRequest,
  withHostObservationScope,
  encodeCognitionUsage,
  decodeObservationInput,
} from "@nous-wave/client/host-bridge";
import {
  createServer,
  type IncomingMessage,
  type ServerResponse,
} from "node:http";
import { writeFile, rm, chmod } from "node:fs/promises";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { expect, test, vi } from "vitest";
import type { OpenClawPluginApi } from "openclaw/plugin-sdk/core";
import type {
  ManagedContextEngine,
  ManagedModelInput,
} from "../src/contract.js";
import type { Context } from "@openclaw/ai";
import { workspaceTemp } from "../../../scripts/workspace.js";
const mocks = vi.hoisted(() => ({
  client: {} as unknown,
  history: [] as unknown[],
  delegated: vi.fn(),
}));
vi.mock("@nous-wave/client/node", () => ({
  connectNousInstance: async () => mocks.client,
}));
vi.mock("openclaw/plugin-sdk/core", () => ({
  definePluginEntry: (entry: unknown) => entry,
  buildJsonPluginConfigSchema: (schema: unknown) => schema,
  delegateCompactionToRuntime: (input: unknown) => {
    mocks.delegated(input);
    return { ok: true, compacted: true };
  },
}));
vi.mock("openclaw/plugin-sdk/session-store-runtime", () => ({
  loadTranscriptEventsSync: () => mocks.history,
  resolveStorePath: () => "fixture-native-store",
}));
import plugin from "../src/index.js";
import { HostBridge } from "../src/host.js";
import { eventIdentity } from "@nous-wave/client/managed";
const subjectId = "00000000-0000-4000-8000-000000000001";
const firstWork = "00000000-0000-4000-8000-000000000002";
const secondWork = "00000000-0000-4000-8000-000000000003";
async function fixture(hosted = true) {
  const root = await workspaceTemp("tests", "host-bridge-");
  const credentialFile = join(root, "credential");
  await writeFile(credentialFile, "fixture-host-token", { mode: 0o600 });
  const state = {
    prepareReject: false,
    prepareRedirect: false,
    prepareDelayMs: 0,
    pendingReject: false,
    committedReject: false,
    recordReject: false,
    handoffReject: false,
    foreign: false,
    workContextId: firstWork,
  };
  const prepares: HostPrepareRequest[] = [];
  const handoffs: unknown[] = [];
  const admittedChildren = new Set<string>();
  const pending = new Map<string, Record<string, unknown>>();
  const permissions = new Map<string, string>();
  const usages = new Map<
    string,
    ReturnType<typeof create<typeof CognitionUsageSchema>>
  >();
  const contactFacts = new Map<string, ReturnType<typeof toJson>>();
  const stored = new Map<
    string,
    ReturnType<typeof create<typeof ObservationInputSchema>>
  >();
  const observedAt = new Date();
  const handle = async (req: IncomingMessage, res: ServerResponse) => {
    try {
      expect(req.headers.authorization).toBe("Bearer fixture-host-token");
      let raw = "";
      for await (const chunk of req) raw += String(chunk);
      const body: unknown = JSON.parse(raw);
      res.setHeader("content-type", "application/json");
      if (req.url === HOST_BRIDGE_PATHS.prepare) {
        const request = hostPrepareRequestSchema.parse(body);
        prepares.push(request);
        if (state.prepareRedirect) {
          res.writeHead(302, { location: "/forbidden-followup" });
          res.end();
          return;
        }
        if (state.prepareDelayMs)
          await new Promise((done) => setTimeout(done, state.prepareDelayMs));
        if (
          request.native.parentSessionKey &&
          !admittedChildren.has(request.native.sessionKey)
        ) {
          res.writeHead(403);
          res.end("{}");
          return;
        }
        if (state.prepareReject) {
          res.writeHead(403);
          res.end("{}");
          return;
        }
        permissions.set(request.contactRequestId, state.workContextId);
        const actualUsage = create(CognitionUsageSchema, {
          agentInstanceId: `${request.native.sessionId}:${request.native.runId}`,
          purpose: request.native.parentSessionKey ? "worker" : "deliberation",
          intent: "Trusted current Activity",
          sourceRead: {
            subjectId,
            sourceRefs: [{ kind: "occurrence", value: "registered-root" }],
          },
          audience: {
            conversationRef: "conversation:qq:room",
            visibility: "group",
          },
        });
        usages.set(request.contactRequestId, actualUsage);
        const contacts = (request.native.parentSessionKey ? [] : [0, 1]).map(
          (index) => ({
            contactRef: `host:message:${index}`,
            observation: toJson(
              ObservationInputSchema,
              create(ObservationInputSchema, {
                subjectId: state.foreign ? randomUUID() : subjectId,
                sessionId: request.nousSessionId,
                requestId: `10000000-0000-4000-8000-00000000000${index}`,
                sourceClass: "message",
                externalObjectRef: `object:qq:fact:${index}`,
                conversationRef: `conversation:qq:room:${index}`,
                actorEntityRef: `entity:author-${index}`,
                observedAt: timestampFromDate(observedAt),
                occurredTime: {
                  value: {
                    case: "instant",
                    value: timestampFromDate(
                      new Date(observedAt.getTime() - 3600000 - index * 1000),
                    ),
                  },
                },
                material: {
                  case: "inlineText",
                  value: {
                    text: `actual author ${index}`,
                    mediaType: "text/plain",
                  },
                },
                context: {
                  message_fact_id: `fact-${index}`,
                  activity_ref: state.workContextId,
                  contact_request_id: request.contactRequestId,
                  cognition_usage: encodeCognitionUsage(actualUsage),
                },
              }),
            ),
          }),
        );
        for (const contact of contacts) {
          if (state.foreign) continue;
          if (!contactFacts.has(contact.contactRef))
            contactFacts.set(contact.contactRef, contact.observation);
          else contact.observation = contactFacts.get(contact.contactRef)!;
        }
        res.end(
          JSON.stringify({
            version: 1,
            contactRequestId: request.contactRequestId,
            workContextId: state.workContextId,
            usage: toJson(
              CognitionUsageSchema,
              create(CognitionUsageSchema, {
                agentInstanceId: `${request.native.sessionId}:${request.native.runId}`,
                purpose: request.native.parentSessionKey
                  ? "worker"
                  : "deliberation",
                intent: "Trusted current Activity",
                sourceRead: {
                  subjectId,
                  sourceRefs: [
                    { kind: "occurrence", value: "registered-root" },
                  ],
                },
                audience: {
                  conversationRef: "conversation:qq:room",
                  visibility: "group",
                },
              }),
            ),
            contacts,
            scope: {
              activityRef: state.workContextId,
              actorEntityRef: "entity:subject",
              context: { host_scope: "adopted" },
            },
          }),
        );
        return;
      }
      if (req.url === HOST_BRIDGE_PATHS.admitted) {
        const admitted = hostAdmittedRequestSchema.parse(body);
        usages.set(
          admitted.contactRequestId,
          decodeProjection(admitted.projection).usage!,
        );
      }
      if (req.url === HOST_BRIDGE_PATHS.feedback) {
        const request = hostFeedbackRequestSchema.parse(body);
        if (state.pendingReject) {
          res.writeHead(503);
          res.end("{}");
          return;
        }
        if (request.stage === "pending") {
          const id = request.observation.requestId as string;
          if (!pending.has(id))
            pending.set(
              id,
              toJson(
                ObservationInputSchema,
                withHostObservationScope(
                  decodeObservationInput(request.observation),
                  {
                    contactRequestId: request.contactRequestId,
                    activityRef: permissions.get(request.contactRequestId)!,
                    usage: usages.get(request.contactRequestId)!,
                  },
                ),
              ) as Record<string, unknown>,
            );
          res.end(
            JSON.stringify({
              version: 1,
              stage: "pending",
              accepted: true,
              observation: pending.get(id),
            }),
          );
          return;
        }
        if (state.committedReject) {
          res.writeHead(503);
          res.end("{}");
          return;
        }
        res.end(
          JSON.stringify({ version: 1, stage: "committed", accepted: true }),
        );
        return;
      }
      if (req.url === HOST_BRIDGE_PATHS.handoff) {
        const handoff = hostHandoffRequestSchema.parse(body);
        handoffs.push(handoff);
        if (state.handoffReject) {
          res.writeHead(403);
          res.end("{}");
          return;
        }
        admittedChildren.add(handoff.childSessionKey);
      }
      res.end(JSON.stringify({ version: 1, accepted: true }));
    } catch (error) {
      res.writeHead(500);
      res.end(JSON.stringify({ error: String(error) }));
    }
  };
  const server = createServer((req, res) => {
    void handle(req, res);
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  const address = server.address();
  if (!address || typeof address === "string")
    throw new Error("fixture server");
  const origin = `http://127.0.0.1:${address.port}`;
  const session = {
    sessionId: randomUUID(),
    runtimeRevision: 1n,
    activeWorkContextId: undefined as string | undefined,
  };
  const sessions = new Map<string, typeof session>([
    [session.sessionId, session],
  ]);
  const runtime = {
    openSession: vi.fn(async (input: { parentSessionId?: string }) => {
      if (!input.parentSessionId) return { ...session };
      const child = {
        ...session,
        sessionId: randomUUID(),
        activeWorkContextId: undefined,
      };
      sessions.set(child.sessionId, child);
      return { ...child };
    }),
    getSession: vi.fn(async (input: { id: string }) => ({
      ...sessions.get(input.id)!,
    })),
    setActiveWorkContext: vi.fn(
      async (value: { sessionId: string; workContextId?: string }) => {
        const selected = sessions.get(value.sessionId)!;
        selected.activeWorkContextId = value.workContextId;
        selected.runtimeRevision++;
        return { ...selected };
      },
    ),
    recordObservation: vi.fn(
      async (
        value: ReturnType<typeof create<typeof ObservationInputSchema>>,
      ) => {
        if (state.recordReject) throw new Error("Nous offline");
        if (!stored.has(value.requestId!))
          stored.set(value.requestId!, structuredClone(value));
        return create(AcceptedObservationSchema, {
          occurrenceId: await eventIdentity(`receipt:${value.requestId}`),
        });
      },
    ),
    reportUse: vi.fn(async () => ({ modelInputChecked: true })),
    closeSession: vi.fn(async () => ({})),
  };
  const build = vi.fn(
    async (input: {
      projection: {
        subjectId: string;
        sessionId: string;
        consumerId: string;
        usage: unknown;
      };
    }) =>
      create(ManagedContextResponseSchema, {
        kind: "RESET",
        cursor: { trackId: "track", epochId: "epoch", revision: 1 },
        projection: {
          ...input.projection,
          projectionId: "projection:actual",
          segments: [],
        },
      } as never),
  );
  mocks.client = { runtime, cognition: { buildManagedContext: build } };
  const entries = new Map<
    string,
    { sessionId: string; pluginExtensions?: Record<string, unknown> }
  >();
  entries.set("native:main", { sessionId: "native-session" });
  entries.set("native:child", { sessionId: "native-child" });
  let engine: ManagedContextEngine;
  let engineFactory: () => ManagedContextEngine;
  const logger = { info: vi.fn(), error: vi.fn() };
  const api = {
    pluginConfig: {
      runRoot: "fixture-run",
      subjectId,
      consumerId: "consumer:fixture",
      ...(hosted ? { hostBridge: { origin, credentialFile } } : {}),
    },
    config: {},
    logger,
    session: { state: { registerSessionExtension: vi.fn() } },
    runtime: {
      agent: {
        session: {
          getSessionEntry: ({ sessionKey }: { sessionKey: string }) =>
            entries.get(sessionKey),
        },
      },
    },
    registerContextEngine: (
      _id: string,
      createEngine: () => ManagedContextEngine,
    ) => {
      engineFactory = createEngine;
      engine = createEngine();
    },
  };
  plugin.register(api as unknown as OpenClawPluginApi);
  const input = (runId = "native-run"): ManagedModelInput =>
    ({
      boundaryVersion: 3,
      historyIsFresh: true,
      historyGeneration: "native-session",
      sessionId: "native-session",
      sessionKey: "native:main",
      agentId: "main",
      runId,
      prompt: "trusted raw prompt",
      model: {},
      context: {
        messages: [
          {
            role: "user",
            content: "host envelope should not become a generic Observation",
            timestamp: Date.now(),
          },
        ],
        systemPrompt: "native",
      },
      signal: new AbortController().signal,
      contextTokenBudget: 8192,
      reservedOutputTokens: 512,
      measureContext: () => 100,
      audience: { visibility: "internal" },
      persistSessionBinding: async (value: Record<string, string>) => {
        entries.get("native:main")!.pluginExtensions = {
          "nous-cognition": { "nous-cognition": value },
        };
      },
    }) as unknown as ManagedModelInput;
  let stopped = false;
  const stopHost = async () => {
    if (stopped) return;
    stopped = true;
    await new Promise<void>((done, fail) =>
      server.close((error) => (error ? fail(error) : done())),
    );
  };
  return {
    stopHost,
    state,
    prepares,
    handoffs,
    pending,
    stored,
    runtime,
    build,
    entries,
    engine: engine!,
    reopen: () => engineFactory(),
    input,
    logger,
    origin,
    credentialFile,
    close: async () => {
      await stopHost();
      await rm(root, { recursive: true, force: true });
    },
  };
}

test("strict hosted preparation preserves two authors and event/acquisition provenance, switches Activity, and rejects denial/cancellation/foreign contacts", async () => {
  const f = await fixture();
  mocks.history = [];
  try {
    const input = f.input();
    await f.engine.prepareModelInput(input);
    expect(f.prepares[0]?.promptDigest).toMatch(/^[a-f0-9]{64}$/);
    expect(JSON.stringify(f.prepares)).not.toContain(input.prompt);
    expect(f.stored.size).toBe(2);
    const actualRoots = [...f.stored.keys()];
    const projected = f.build.mock.calls[0]![0].projection.usage as ReturnType<
      typeof create<typeof CognitionUsageSchema>
    >;
    expect(projected.sourceRead?.sourceRefs.map((ref) => ref.value)).toEqual([
      "registered-root",
      ...(await Promise.all(
        actualRoots.map((id) => eventIdentity(`receipt:${id}`)),
      )),
    ]);
    for (const record of f.stored.values()) {
      expect(record.context?.activity_ref).toBe(firstWork);
      expect(record.context?.contact_request_id).toMatch(/^[0-9a-f-]{36}$/);
      expect(record.context?.cognition_usage).toMatchObject({
        agentInstanceId: "native-session:native-run",
      });
    }
    expect([...f.stored.values()].map((value) => value.actorEntityRef)).toEqual(
      ["entity:author-0", "entity:author-1"],
    );
    for (const observation of f.stored.values()) {
      expect(observation.externalObjectRef).toMatch(/^object:qq:fact:/);
      expect(timestampDate(observation.observedAt!).getTime()).toBeGreaterThan(
        timestampDate(observation.occurredTime!.value.value as never).getTime(),
      );
    }
    f.state.workContextId = secondWork;
    await f.engine.prepareModelInput({
      ...input,
      context: {
        ...input.context,
        messages: [
          ...input.context.messages,
          {
            role: "toolResult",
            toolCallId: "not-yet-emitted",
            toolName: "read",
            content: [],
            isError: false,
            timestamp: Date.now(),
          },
        ],
      },
    });
    expect(
      f.runtime.setActiveWorkContext.mock.calls.map(
        (value) => value[0].workContextId,
      ),
    ).toEqual([firstWork, secondWork]);
    expect(f.stored.size).toBe(2);
    const count = f.build.mock.calls.length;
    f.state.prepareReject = true;
    await expect(f.engine.prepareModelInput(input)).rejects.toThrow("rejected");
    expect(f.build).toHaveBeenCalledTimes(count);
    f.state.prepareReject = false;
    f.state.foreign = true;
    await expect(f.engine.prepareModelInput(input)).rejects.toThrow("foreign");
    f.state.foreign = false;
    f.state.prepareDelayMs = 30;
    const during = new AbortController();
    const waiting = f.engine.prepareModelInput({
      ...input,
      signal: during.signal,
    });
    setTimeout(() => during.abort(), 5);
    await expect(waiting).rejects.toThrow();
    expect(f.build).toHaveBeenCalledTimes(count);
    const controller = new AbortController();
    controller.abort();
    await expect(
      f.engine.prepareModelInput({ ...input, signal: controller.signal }),
    ).rejects.toThrow();
  } finally {
    await f.close();
  }
});

test("failed pending feedback preserves actual result and new user turn/compact must reconcile original scope before proceeding", async () => {
  const f = await fixture();
  mocks.history = [];
  try {
    const first = f.input();
    const application = await f.engine.prepareModelInput(first);
    const tool = {
      role: "toolResult",
      toolCallId: "actual-tool",
      toolName: "write",
      content: [{ type: "text", text: "actual file already written" }],
      isError: false,
      timestamp: Date.now(),
    } as Context["messages"][number];
    f.state.pendingReject = true;
    await expect(
      f.engine.recordNativeEvent(first, tool),
    ).resolves.toBeUndefined();
    mocks.history = [
      JSON.parse(JSON.stringify({ type: "message", message: tool })),
    ];
    const recovered = f.reopen();
    const next = f.input("new-user-run");
    next.context.messages = [
      {
        role: "user",
        content: "a completely new user turn",
        timestamp: Date.now(),
      },
    ];
    await expect(recovered.prepareModelInput(next)).rejects.toThrow("rejected");
    await expect(
      recovered.compact({
        sessionId: first.sessionId,
        sessionKey: first.sessionKey!,
        agentId: first.agentId,
      }),
    ).rejects.toThrow("rejected");
    expect(mocks.delegated).not.toHaveBeenCalled();
    f.state.pendingReject = false;
    f.state.recordReject = true;
    await expect(recovered.prepareModelInput(next)).rejects.toThrow(
      "Nous offline",
    );
    expect(f.pending.size).toBe(1);
    f.state.recordReject = false;
    f.state.workContextId = secondWork;
    await recovered.prepareModelInput(next);
    const saved = [...f.stored.values()].find(
      (value) => value.sourceClass === "tool",
    )!;
    expect(saved.context?.activity_ref).toBe(firstWork);
    expect(saved.context?.native_run_id).toBe("native-run");
    const assistant = {
      role: "assistant",
      content: [{ type: "text", text: "actual output" }],
      timestamp: Date.now(),
      stopReason: "stop",
    } as Context["messages"][number];
    f.state.committedReject = true;
    await expect(
      f.engine.reportModelOutcome(first, application, "completed", assistant),
    ).resolves.toBeUndefined();
    mocks.history.push(
      JSON.parse(JSON.stringify({ type: "message", message: assistant })),
    );
    f.state.committedReject = false;
    await recovered.prepareModelInput(next);
    expect(
      [...f.stored.values()].filter((value) => value.sourceClass === "tool"),
    ).toHaveLength(1);
    expect(
      [...f.stored.values()].some(
        (value) => value.sourceClass === "host_event",
      ),
    ).toBe(true);
    await recovered.compact({
      sessionId: first.sessionId,
      sessionKey: first.sessionKey!,
      agentId: first.agentId,
    });
    expect(mocks.delegated).toHaveBeenCalledOnce();
  } finally {
    await f.close();
  }
});

test("subagent handoff requires Host admission and closes rejected child; standalone plugin remains usable", async () => {
  const f = await fixture();
  mocks.history = [];
  try {
    await f.engine.prepareModelInput(f.input());
    f.state.handoffReject = true;
    const params = {
      parentSessionKey: "native:main",
      childSessionKey: "native:child",
      contextMode: "isolated" as const,
      persistSessionBinding: vi.fn(async () => {}),
    };
    await expect(f.engine.prepareSubagentSpawn(params)).rejects.toThrow(
      "rejected",
    );
    expect(f.runtime.closeSession).toHaveBeenCalledOnce();
    f.state.handoffReject = false;
    params.persistSessionBinding = vi.fn(
      async (value?: Record<string, string>) => {
        f.entries.get("native:child")!.pluginExtensions = {
          "nous-cognition": { "nous-cognition": value },
        };
      },
    );
    const accepted = await f.engine.prepareSubagentSpawn(params);
    const child = {
      ...f.input("child-run"),
      sessionId: "native-child",
      sessionKey: "native:child",
      historyGeneration: "native-child",
      agentId: "child-worker",
      parentSessionKey: "native:main",
    };
    await f.engine.prepareModelInput(child);
    expect(f.prepares.at(-1)?.native.parentSessionKey).toBe("native:main");
    expect(f.build.mock.calls.at(-1)?.[0].projection.usage).toMatchObject({
      purpose: "worker",
      agentInstanceId: "native-child:child-run",
    });
    expect(f.handoffs).toHaveLength(2);
    await accepted.rollback();
    expect(f.runtime.closeSession).toHaveBeenCalledTimes(2);
  } finally {
    await f.close();
  }
  const standalone = await fixture(false);
  mocks.history = [];
  try {
    await standalone.engine.prepareModelInput(standalone.input());
    await expect(
      standalone.engine.prepareModelInput({
        ...standalone.input(),
        boundaryVersion: 2,
      } as unknown as ManagedModelInput),
    ).rejects.toThrow("boundary");
    expect(standalone.prepares).toHaveLength(0);
    expect(
      [...standalone.stored.values()].some((value) =>
        value.externalObjectRef?.startsWith("object:openclaw:"),
      ),
    ).toBe(true);
  } finally {
    await standalone.close();
  }
});

test("bridge refuses external origins, nonprivate credentials, and offline Host", async () => {
  const f = await fixture();
  mocks.history = [];
  try {
    expect(
      () =>
        new HostBridge({
          origin: "https://example.com",
          credentialFile: f.credentialFile,
        }),
    ).toThrow("loopback");
    f.state.prepareRedirect = true;
    await expect(f.engine.prepareModelInput(f.input())).rejects.toThrow();
    expect(f.build).not.toHaveBeenCalled();
    f.state.prepareRedirect = false;
    await chmod(f.credentialFile, 0o644);
    await expect(f.engine.prepareModelInput(f.input())).rejects.toThrow(
      "private",
    );
  } finally {
    await f.close();
  }
  const offline = await fixture();
  try {
    await offline.stopHost();
    await expect(
      offline.engine.prepareModelInput(offline.input()),
    ).rejects.toThrow();
    expect(offline.build).not.toHaveBeenCalled();
  } finally {
    await offline.close();
  }
});
