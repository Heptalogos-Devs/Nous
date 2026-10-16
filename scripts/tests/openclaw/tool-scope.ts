// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Exercise the pinned production executor after the real installer applied v5.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import vm from "node:vm";
const source = await readFile(
  join(process.argv[2]!, "dist/agent-core-BxgCPa8O.js"),
  "utf8",
);
const start = source.indexOf("async function prepareToolCallExecution(");
const end = source.indexOf("async function finalizeExecutedToolCall(", start);
assert(start >= 0 && end > start);
let implementations = 0;
let preparations = 0;
let deniedPhase = "";
let offline = false;
const phases: string[] = [];
const context = vm.createContext({
  getInternalToolExecutionPreparer: (tool: { preparer?: unknown }) =>
    tool.preparer,
  runWithAgentToolExecutionContext: (_context: unknown, run: () => unknown) =>
    run(),
  createErrorToolResult: (text: string) => ({
    content: [{ type: "text", text }],
  }),
  coerceErrorMessage: String,
});
const prepare = vm.runInContext(
  source.slice(start, end) + ";prepareToolCallExecution",
  context,
) as (...args: unknown[]) => Promise<{
  kind: string;
  execute: (
    start?: () => void,
  ) => Promise<{ executionStarted: boolean; isError: boolean }>;
}>;
const managed = {
  async authorizeToolExecution(input: {
    toolName: string;
    toolCallId: string;
    phase: string;
  }) {
    assert.equal(input.toolName, "actual_read");
    assert.equal(input.toolCallId, "native-call");
    phases.push(input.phase);
    if (offline || input.phase === deniedPhase)
      throw Error("Host denied or offline");
    return { run: async (run: () => unknown) => await run() };
  },
};
const tool = {
  name: "actual_read",
  execute: async () => {
    implementations++;
    return { content: [] };
  },
};
const prepared = {
  tool,
  toolCall: {
    id: "native-call",
    name: "actual_read",
    arguments: { toolName: "send", toolCallId: "forged" },
  },
  args: { toolName: "send", toolCallId: "forged" },
};
const controller = new AbortController();
const ready = await prepare(
  prepared,
  {},
  controller.signal,
  async () => {},
  managed,
);
assert.equal((await ready.execute()).executionStarted, true);
assert.equal(implementations, 1);
assert.deepEqual(phases, ["prepare", "execute"]);
deniedPhase = "prepare";
await assert.rejects(() =>
  prepare(prepared, {}, controller.signal, async () => {}, managed),
);
assert.equal(implementations, 1);
deniedPhase = "execute";
const changed = await prepare(
  prepared,
  {},
  controller.signal,
  async () => {},
  managed,
);
assert.equal((await changed.execute()).executionStarted, false);
assert.equal(implementations, 1);
offline = true;
await assert.rejects(() =>
  prepare(prepared, {}, controller.signal, async () => {}, managed),
);
offline = false;
deniedPhase = "";
const internal = {
  ...prepared,
  tool: {
    ...tool,
    preparer: async () => {
      preparations++;
      return {
        kind: "ready",
        args: {},
        dispose() {},
        async execute(onStart: () => void) {
          onStart();
          implementations++;
          return { content: [] };
        },
      };
    },
  },
};
deniedPhase = "prepare";
await assert.rejects(() =>
  prepare(internal, {}, controller.signal, async () => {}, managed),
);
assert.equal(preparations, 0);
deniedPhase = "";
const internalReady = await prepare(
  internal,
  {},
  controller.signal,
  async () => {},
  managed,
);
assert.equal(preparations, 1);
deniedPhase = "execute";
assert.equal((await internalReady.execute()).executionStarted, false);
assert.equal(implementations, 1);
deniedPhase = "";
const standalone = await prepare(
  prepared,
  {},
  controller.signal,
  async () => {},
);
assert.equal((await standalone.execute()).executionStarted, true);
assert.equal(implementations, 2);
const cancelled = await prepare(
  prepared,
  {},
  controller.signal,
  async () => {},
  managed,
);
controller.abort();
assert.equal((await cancelled.execute()).executionStarted, false);
assert.equal(implementations, 2);
console.log(
  JSON.stringify({
    nativeToolExecutionGuard: "PASS",
    internalPreparerGuard: "PASS",
    currentScopeRevocation: "PASS",
    standaloneUnchanged: "PASS",
    modelCalls: 0,
    qqTraffic: 0,
  }),
);
