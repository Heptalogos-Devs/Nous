// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Exercise the installed native SQLite owner, including a genuinely delayed
// summary: foreground appends continue while the captured prefix is in flight.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve as resolvePath } from "node:path";
import { pathToFileURL } from "node:url";
const native = resolvePath(process.argv[2]!);
const state = await mkdtemp(join(tmpdir(), "nous-prefix-"));
process.env.OPENCLAW_STATE_DIR = state;
interface Entry {
  id: string;
  type: string;
  message?: { role: string; content: unknown };
}
interface Session {
  appendMessage(message: unknown): string;
  appendResetBoundary(reason: string): string;
  captureCompactionPrefix(firstKeptEntryId: string): unknown;
  appendCompactionPrefix(
    snapshot: unknown,
    summary: string,
    tokensBefore: number,
    details?: unknown,
    fromHook?: boolean,
    metadata?: unknown,
  ): Promise<string>;
  buildSessionContext(): {
    messages: Array<{ role: string; content: unknown }>;
  };
  getBranch(): Entry[];
  flushPendingPersistence(): void;
}
const { t: SessionManager } = (await import(
  pathToFileURL(join(native, "dist/session-manager-CNTG_BAJ.js")).href
)) as { t: { open(target: Record<string, string>): Session } };
const target = {
  agentId: "prefix-fixture",
  sessionId: "prefix-session",
  sessionKey: "agent:prefix-fixture:main",
  storePath: join(state, "sessions.json"),
};
const first = SessionManager.open(target);
const append = (session: Session, role: string, text: string) =>
  session.appendMessage({
    role,
    content: role === "user" ? text : [{ type: "text", text }],
    timestamp: Date.now(),
    ...(role === "assistant"
      ? {
          api: "openai-completions",
          provider: "fixture",
          model: "fixture",
          stopReason: "stop",
          usage: {
            input: 0,
            output: 0,
            cacheRead: 0,
            cacheWrite: 0,
            totalTokens: 0,
            cost: {
              input: 0,
              output: 0,
              cacheRead: 0,
              cacheWrite: 0,
              total: 0,
            },
          },
        }
      : {}),
  });
append(first, "user", "A");
append(first, "assistant", "B");
append(first, "user", "C");
first.appendMessage({
  role: "assistant",
  content: [
    {
      type: "toolCall",
      id: "CALL_D",
      name: "fixture_read",
      arguments: { path: "fixture" },
    },
  ],
  api: "openai-completions",
  provider: "fixture",
  model: "fixture",
  stopReason: "toolUse",
  timestamp: Date.now(),
});
first.appendMessage({
  role: "toolResult",
  toolCallId: "CALL_D",
  toolName: "fixture_read",
  content: [{ type: "text", text: "RESULT_D" }],
  isError: false,
  timestamp: Date.now(),
});
append(first, "assistant", "D");
const keep = append(first, "user", "E");
append(first, "assistant", "F");
first.flushPendingPersistence();
const compaction = (await import(
  pathToFileURL(join(native, "dist/compaction-DlElLXrZ.js")).href
)) as {
  g(
    entries: Entry[],
    settings: Record<string, unknown>,
  ): { ok: boolean; value: { firstKeptEntryId: string } };
  l(
    preparation: unknown,
    model: unknown,
    apiKey: string,
    headers: unknown,
    instructions: unknown,
    signal: AbortSignal,
    thinking: unknown,
    stream: unknown,
    runtime: unknown,
  ): Promise<{
    ok: boolean;
    value: { summary: string; tokensBefore: number; details: unknown };
  }>;
};
const core = (await import(
  pathToFileURL(join(native, "dist/agent-core-BxgCPa8O.js")).href
)) as { n: unknown };
const preparation = compaction.g(first.getBranch(), {
  enabled: true,
  reserveTokens: 128,
  keepRecentTokens: 1,
  nousFixedPrefix: true,
});
assert(preparation.ok && preparation.value);
assert.equal(preparation.value.firstKeptEntryId, keep);
const snapshot = first.captureCompactionPrefix(
  preparation.value.firstKeptEntryId,
);
let received!: () => void;
let requestReceived = new Promise<void>((resolve) => {
  received = resolve;
});
let release!: () => void;
let releaseResponse = new Promise<void>((resolve) => {
  release = resolve;
});
let modelRequests = 0;
const provider = createServer((request, response) => {
  void (async () => {
    let body = "";
    for await (const bytes of request) body += String(bytes);
    modelRequests++;
    const input = JSON.parse(body) as { messages: unknown; stream: boolean };
    const prompt = JSON.stringify(input.messages);
    assert(prompt.includes("A") && prompt.includes("D"));
    if (modelRequests === 1)
      assert(
        prompt.includes("fixture_read") && prompt.includes("RESULT_D"),
        "Native summary must retain complete tool pairs",
      );
    assert(!prompt.includes("CURRENT_NOUS_SLICE"));
    received();
    await releaseResponse;
    response.setHeader("content-type", "text/event-stream");
    const chunk = {
      id: "summary",
      object: "chat.completion.chunk",
      created: 1,
      model: "prefix-summary",
      choices: [
        {
          index: 0,
          delta: { role: "assistant", content: "SUMMARY_A_TO_D" },
          finish_reason: null,
        },
      ],
    };
    response.write("data: " + JSON.stringify(chunk) + "\n\n");
    response.write(
      "data: " +
        JSON.stringify({
          ...chunk,
          choices: [{ index: 0, delta: {}, finish_reason: "stop" }],
        }) +
        "\n\n",
    );
    response.end("data: [DONE]\n\n");
  })().catch((error) => {
    response.destroy(error as Error);
  });
});
await new Promise<void>((resolve) => provider.listen(0, "127.0.0.1", resolve));
const address = provider.address();
assert(address && typeof address !== "string");
try {
  const summary = compaction.l(
    preparation.value,
    {
      id: "prefix-summary",
      name: "Prefix summary",
      api: "openai-completions",
      provider: "fixture",
      baseUrl: "http://127.0.0.1:" + address.port + "/v1",
      reasoning: false,
      input: ["text"],
      contextWindow: 8192,
      maxTokens: 128,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    },
    "fixture-only",
    undefined,
    undefined,
    new AbortController().signal,
    "off",
    undefined,
    core.n,
  );
  await Promise.race([
    requestReceived,
    summary.then((result) => {
      throw Error(
        "Summary ended before reaching local provider: " +
          JSON.stringify(result) +
          String((result as { error?: Error }).error?.message),
      );
    }),
  ]);
  // A distinct real native owner appends G/H while summary generation waits.
  const foreground = SessionManager.open(target);
  append(foreground, "user", "G");
  append(foreground, "assistant", "H");
  foreground.flushPendingPersistence();
  release();
  const result = await summary;
  assert(result.ok, JSON.stringify(result));
  await first.appendCompactionPrefix(
    snapshot,
    result.value.summary,
    result.value.tokensBefore,
    result.value.details,
  );
  // Exercise the actual native direct owner and prepared-model lease. Soft and
  // manual/hard requests share one delayed model operation for this Session.
  const directModule = (await import(
    pathToFileURL(join(native, "dist/compact-gStBcHDe.js")).href
  )) as {
    t(
      params: Record<string, unknown>,
    ): Promise<{ ok: boolean; compacted: boolean; reason?: string }>;
  };
  const target2 = {
    ...target,
    sessionId: "prefix-background",
    sessionKey: "agent:prefix-fixture:background",
  };
  const second = SessionManager.open(target2);
  append(second, "user", "A".repeat(1200));
  append(second, "assistant", "B".repeat(1200));
  append(second, "user", "C".repeat(1200));
  append(second, "assistant", "D".repeat(1200));
  append(second, "user", "E");
  append(second, "assistant", "F");
  second.flushPendingPersistence();
  requestReceived = new Promise<void>((resolve) => {
    received = resolve;
  });
  releaseResponse = new Promise<void>((resolve) => {
    release = resolve;
  });
  const model = {
    id: "prefix-summary",
    name: "Prefix summary",
    reasoning: false,
    input: ["text"],
    contextWindow: 8192,
    maxTokens: 128,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  };
  const config = {
    agents: {
      defaults: {
        workspace: state,
        model: { primary: "fixture/prefix-summary" },
        compaction: { mode: "default" },
      },
    },
    models: {
      providers: {
        fixture: {
          baseUrl: "http://127.0.0.1:" + address.port + "/v1",
          apiKey: "fixture-only",
          api: "openai-completions",
          models: [model],
        },
      },
    },
    plugins: { slots: { contextEngine: "nous-cognition", memory: "none" } },
  };
  const params = {
    ...target2,
    sessionTarget: target2,
    workspaceDir: state,
    config,
    provider: "fixture",
    model: "prefix-summary",
    tokenBudget: 8192,
    force: true,
    trigger: "manual",
    contextEngineRuntimeContext: {
      nousPrefixPolicy: {
        background: true,
        inputBudget: 256,
        assertPublish: async () => {},
      },
    },
  };
  const soft = await directModule.t(params);
  assert(soft.ok && !soft.compacted, JSON.stringify(soft));
  await Promise.race([
    requestReceived,
    new Promise((_, reject) =>
      setTimeout(
        () => reject(Error("Native compaction model did not start")),
        10000,
      ).unref(),
    ),
  ]);
  const joined = directModule.t({
    ...params,
    contextEngineRuntimeContext: {
      nousPrefixPolicy: {
        ...params.contextEngineRuntimeContext.nousPrefixPolicy,
        background: false,
      },
    },
  });
  const extra = SessionManager.open(target2);
  append(extra, "user", "G");
  append(extra, "assistant", "H");
  extra.flushPendingPersistence();
  release();
  const published = await joined;
  assert.equal(
    modelRequests,
    2,
    "manual request must join the one native background model call",
  );
  assert(published.ok && published.compacted, JSON.stringify(published));
  const final = JSON.stringify(
    SessionManager.open(target2).buildSessionContext().messages,
  );
  for (const value of ["SUMMARY_A_TO_D", "E", "F", "G", "H"])
    assert(final.includes(value), final);
} finally {
  release();
  await new Promise<void>((resolve, reject) =>
    provider.close((error) => (error ? reject(error) : resolve())),
  );
}
const reopened = SessionManager.open(target);
const text = JSON.stringify(reopened.buildSessionContext().messages);
assert(text.includes("SUMMARY_A_TO_D"));
for (const value of ["E", "F", "G", "H"])
  assert(text.includes('"' + value + '"'), text);
for (const value of ["A", "B", "C", "D"])
  assert(!text.includes('"' + value + '"'), text);
// Another summary cannot publish over an already changed summary generation.
await assert.rejects(
  first.appendCompactionPrefix(snapshot, "STALE", 1000),
  /invalidated/,
);
const cut = reopened
  .getBranch()
  .findLast((entry) => entry.message?.role === "user")!.id;
const resetSnapshot = reopened.captureCompactionPrefix(cut);
reopened.appendResetBoundary("reset");
await assert.rejects(
  reopened.appendCompactionPrefix(resetSnapshot, "STALE_RESET", 1000),
  /invalidated|complete prefix/,
);
// Native durable role/source permission changes participate in the same TX CAS.
const entryStore = (await import(
  pathToFileURL(join(native, "dist/session-accessor.sqlite-entry-BlklNPKQ.js"))
    .href
)) as {
  d(target: unknown): Record<string, unknown>;
  v(target: unknown, entry: unknown): void;
};
const scopedTarget = {
  ...target,
  sessionId: "scope-session",
  sessionKey: "agent:prefix-fixture:scope",
};
const scoped = SessionManager.open(scopedTarget);
append(scoped, "user", "A");
append(scoped, "assistant", "B");
const scopedKeep = append(scoped, "user", "E");
append(scoped, "assistant", "F");
scoped.flushPendingPersistence();
const baseScope = {
  role: "subject.execution",
  view: "worker",
  worldId: "fixture-world",
  model: ["fixture", "prefix-summary", 8192, 256, 128],
  history: ["A"],
  tools: ["fixture_read"],
  sourceRead: {
    subjectId: "fixture",
    sourceRefs: [{ kind: "occurrence", value: "A" }],
    originConversationRefs: [],
  },
};
const setScope = (scope: unknown) =>
  entryStore.v(scopedTarget, {
    ...entryStore.d(scopedTarget),
    pluginExtensions: {
      "nous-cognition": {
        "nous-cognition": {
          subjectId: "fixture",
          nativeSessionId: scopedTarget.sessionId,
          compactionScope: JSON.stringify(scope),
        },
      },
    },
  });
setScope(baseScope);
const scopedSnapshot = scoped.captureCompactionPrefix(scopedKeep);
setScope({
  ...baseScope,
  history: ["A", "G"],
  sourceRead: {
    ...baseScope.sourceRead,
    sourceRefs: [
      ...baseScope.sourceRead.sourceRefs,
      { kind: "occurrence", value: "G" },
    ],
  },
});
append(scoped, "user", "G");
append(scoped, "assistant", "H");
await scoped.appendCompactionPrefix(scopedSnapshot, "SCOPED_SUMMARY", 1000);
const nextCut = scoped
  .getBranch()
  .findLast((entry) => entry.message?.role === "user")!.id;
const roleSnapshot = scoped.captureCompactionPrefix(nextCut);
setScope({ ...baseScope, view: "review" });
await assert.rejects(
  scoped.appendCompactionPrefix(roleSnapshot, "FOREIGN_ROLE_SUMMARY", 1000),
  /invalidated/,
);
setScope(baseScope);
const permissionSnapshot = scoped.captureCompactionPrefix(nextCut);
setScope({
  ...baseScope,
  sourceRead: { ...baseScope.sourceRead, sourceRefs: [] },
});
await assert.rejects(
  scoped.appendCompactionPrefix(
    permissionSnapshot,
    "WITHDRAWN_SOURCE_SUMMARY",
    1000,
  ),
  /invalidated/,
);
console.log(
  JSON.stringify({
    nativeSqlitePrefixCas: "PASS",
    nativeSummaryModelAndSingleFlight: "PASS",
    nativeRoleAndPermissionCas: "PASS",
    preservedConcurrentTail: "PASS",
    staleSummaryAndResetRejected: "PASS",
  }),
);
