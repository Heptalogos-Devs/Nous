// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Real pinned Gateway RPC -> agent runner -> registered plugin factory/execution.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { mkdtemp, mkdir, writeFile, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { transform } from "esbuild";
import { randomUUID } from "node:crypto";
const native = resolve(process.argv[2]!);
const root = await mkdtemp(join(tmpdir(), "native-bindings-"));
const proof = join(root, "events.jsonl");
const parentRunId = randomUUID();
const pluginRoot = join(native, "node_modules/binding-fixture");
await mkdir(pluginRoot, { recursive: true });
await writeFile(
  join(pluginRoot, "package.json"),
  JSON.stringify({
    name: "binding-fixture",
    version: "1.0.0",
    type: "module",
    openclaw: { extensions: ["./index.mjs"] },
  }),
);
await writeFile(
  join(pluginRoot, "openclaw.plugin.json"),
  JSON.stringify({
    id: "nous-cognition",
    kind: "context-engine",
    channels: ["completion-fixture"],
    contracts: { tools: ["fixture_read"] },
    configSchema: { type: "object", additionalProperties: true },
  }),
);
await writeFile(
  join(pluginRoot, "index.mjs"),
  (
    await transform(
      await readFile(
        new URL(
          "../../../packages/openclaw/test/subagent-completion-plugin.ts",
          import.meta.url,
        ),
        "utf8",
      ),
      { loader: "ts", format: "esm", target: "node24" },
    )
  ).code,
);
let modelCalls = 0;
const turns = new Map<string, number>();
const provider = createServer((req, res) => {
  void (async () => {
    let body = "";
    for await (const b of req) body += String(b);
    const input = JSON.parse(body) as {
      messages: { role: string; content: unknown; tool_call_id?: string }[];
      stream?: boolean;
      model: string;
      max_tokens?: number;
      max_completion_tokens?: number;
    };
    modelCalls++;
    assert.equal(
      input.max_tokens ?? input.max_completion_tokens,
      input.model === "worker-model" ? 29 : 37,
      "Actual provider payload must use the trusted output limit on every model/tool step",
    );
    const text = JSON.stringify(input.messages);
    const isWorker = input.model === "worker-model";
    if (isWorker) assert(!text.includes("EPHEMERAL_CURRENT_CONTEXT_FIXTURE"));
    else if (!text.includes("ready for parent review"))
      assert(text.includes("EPHEMERAL_CURRENT_CONTEXT_FIXTURE"));
    const completion = !isWorker && text.includes("ready for parent review");
    const key = isWorker ? "worker" : completion ? "completion" : "parent";
    const turn = (turns.get(key) ?? 0) + 1;
    turns.set(key, turn);
    const done = turn > 1;
    const callId = key + "-call-" + turn;
    const toolName = isWorker
      ? "fixture_read"
      : completion
        ? "fixture_binding"
        : "sessions_spawn";
    const toolArgs =
      isWorker || completion
        ? {}
        : {
            task: "CHILD_SYNTHETIC_VERIFY_TASK",
            mode: "run",
            cleanup: "keep",
            runTimeoutSeconds: 30,
          };
    const answer = isWorker
      ? "CHILD_SAVED_SYNTHETIC_OFFICIAL_RESULT"
      : completion
        ? "VERIFIED_SAVED_CHILD_RESULT"
        : "NO_REPLY";
    assert(modelCalls <= 8, "Unexpected native model loop");
    const response = {
      id: "fixture-" + modelCalls,
      object: "chat.completion",
      created: 1,
      model: "bindings-model",
      choices: [
        {
          index: 0,
          message: done
            ? { role: "assistant", content: answer }
            : {
                role: "assistant",
                content: null,
                tool_calls: [
                  {
                    id: callId,
                    type: "function",
                    function: {
                      name: toolName,
                      arguments: JSON.stringify(toolArgs),
                    },
                  },
                ],
              },
          finish_reason: done ? "stop" : "tool_calls",
        },
      ],
      usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
    };
    if (!input.stream) {
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(response));
      return;
    }
    res.setHeader("content-type", "text/event-stream");
    const delta = done
      ? { role: "assistant", content: answer }
      : {
          role: "assistant",
          tool_calls: [
            {
              index: 0,
              id: callId,
              type: "function",
              function: {
                name: toolName,
                arguments: JSON.stringify(toolArgs),
              },
            },
          ],
        };
    res.end(
      "data: " +
        JSON.stringify({
          id: response.id,
          object: "chat.completion.chunk",
          created: 1,
          model: "bindings-model",
          choices: [
            { index: 0, delta, finish_reason: done ? "stop" : "tool_calls" },
          ],
          usage: response.usage,
        }) +
        "\n\ndata: [DONE]\n\n",
    );
  })().catch(() => res.destroy());
});
await new Promise<void>((done) => provider.listen(0, "127.0.0.1", done));
const address = provider.address();
assert(address && typeof address !== "string");
const reserve = createServer();
await new Promise<void>((done) => reserve.listen(0, "127.0.0.1", done));
const gatewayAddress = reserve.address();
assert(gatewayAddress && typeof gatewayAddress !== "string");
const port = gatewayAddress.port;
await new Promise<void>((done) => reserve.close(() => done()));
const configPath = join(root, "openclaw.json");
await writeFile(
  configPath,
  JSON.stringify({
    logging: { level: "debug", file: join(root, "native.log") },
    gateway: {
      mode: "local",
      port,
      bind: "loopback",
      auth: { mode: "token", token: "fixture-only-gateway" },
    },
    agents: {
      defaults: {
        workspace: join(root, "workspace"),
        subagents: { model: "fixture/worker-model", runTimeoutSeconds: 30 },
        model: { primary: "fixture/bindings-model" },
        models: { "fixture/worker-model": { params: { maxTokens: 29 } } },
      },
      list: [{ id: "main" }],
    },
    models: {
      providers: {
        fixture: {
          baseUrl: "http://127.0.0.1:" + address.port + "/v1",
          apiKey: "fixture-only-provider",
          api: "openai-completions",
          models: [
            {
              id: "worker-model",
              name: "Fixture worker",
              reasoning: false,
              input: ["text"],
              cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
              contextWindow: 32768,
              maxTokens: 2048,
            },
            {
              id: "bindings-model",
              name: "Fixture",
              reasoning: false,
              input: ["text"],
              cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
              contextWindow: 32768,
              maxTokens: 2048,
            },
          ],
        },
      },
    },
    tools: {
      allow: ["sessions_spawn", "fixture_read", "fixture_binding"],
      subagents: { tools: { allow: ["fixture_read"] } },
    },
    plugins: {
      enabled: true,
      allow: ["nous-cognition"],
      load: { paths: [pluginRoot] },
      slots: { memory: "none", contextEngine: "nous-cognition" },
      entries: {
        "nous-cognition": {
          enabled: true,
          config: { hostBridge: { fixture: true } },
        },
      },
    },
  }),
);
process.env.OPENCLAW_STATE_DIR = join(root, "state");
process.env.OPENCLAW_CONFIG_PATH = configPath;
const child = spawn(
  process.execPath,
  [
    join(native, "dist/index.js"),
    "gateway",
    "run",
    "--port",
    String(port),
    "--bind",
    "loopback",
    "--allow-unconfigured",
  ],
  {
    env: {
      ...process.env,
      NOUS_COMPLETION_FIXTURE_PROOF: proof,
      NOUS_COMPLETION_FIXTURE_PARENT: parentRunId,
      NODE_USE_ENV_PROXY: "0",
      NO_PROXY: "127.0.0.1,localhost",
    },
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
  },
);
let log = "";
child.stdout.on("data", (b: Buffer) => {
  log = (log + b.toString()).slice(-16000);
});
child.stderr.on("data", (b: Buffer) => {
  log = (log + b.toString()).slice(-16000);
});
const sdk = (await import(
  pathToFileURL(join(native, "dist/plugin-sdk/gateway-runtime.js")).href
)) as {
  callGatewayFromCli(
    method: string,
    options: unknown,
    params: unknown,
  ): Promise<unknown>;
};
const opts = {
  url: "ws://127.0.0.1:" + port,
  token: "fixture-only-gateway",
  timeout: "30000",
  expectFinal: true,
};
try {
  let ready = false;
  for (let attempt = 0; attempt < 120; attempt++) {
    if (child.exitCode !== null) throw Error("Fixture Gateway exited: " + log);
    try {
      await sdk.callGatewayFromCli("health", { ...opts, timeout: "1000" }, {});
      ready = true;
      break;
    } catch {
      await new Promise((done) => setTimeout(done, 250));
    }
  }
  assert(ready, "Fixture Gateway did not start: " + log);
  const sessionKey = "agent:main:completion-fixture:group:target";
  const result = await sdk.callGatewayFromCli("agent", opts, {
    agentId: "main",
    sessionKey,
    message: "PARENT_SYNTHETIC_TASK",
    extraSystemPrompt: "  EPHEMERAL_CURRENT_CONTEXT_FIXTURE\n当前片段\n  ",
    idempotencyKey: parentRunId,
    timeout: 30,
    deliver: true,
    channel: "completion-fixture",
    accountId: "fixture-account",
    to: "group:target",
  });
  assert.equal((result as { runId: string }).runId, parentRunId);
  const events = async () =>
    (await readFile(proof, "utf8").catch(() => ""))
      .trim()
      .split("\n")
      .filter(Boolean)
      .map(
        (s) =>
          JSON.parse(s) as {
            event: string;
            continuation?: unknown;
            native?: { runId: string; sessionKey: string };
            outcome?: string;
            parentRunId?: string;
            childRunId: string;
            childSessionKey: string;
            endedAt: string;
          },
      );
  const until = async (test: () => Promise<boolean>) => {
    const deadline = Date.now() + 30000;
    while (!(await test())) {
      if (Date.now() > deadline) throw Error("Completion fixture timed out");
      await new Promise((done) => setTimeout(done, 100));
    }
  };
  await until(async () => (await events()).some((e) => e.event === "terminal"));
  const before = await events();
  assert.equal(
    before.filter((e) => e.event === "sent").length,
    0,
    "Host refused completion must not reach sender or steering into another run",
  );
  const terminal = before.find(
    (e) => e.event === "terminal" && !e.continuation,
  )!;
  assert.equal(terminal.parentRunId, parentRunId);
  await until(async () =>
    (await events()).some(
      (e) =>
        e.event === "runTerminal" &&
        (e.native as { runId?: string } | undefined)?.runId ===
          terminal.childRunId,
    ),
  );
  const directTerminal = (await events()).find(
    (e) =>
      e.event === "runTerminal" &&
      (e.native as { runId?: string } | undefined)?.runId ===
        terminal.childRunId,
  )!;
  assert.equal(directTerminal.outcome, "completed");
  assert.equal(
    (directTerminal.native as { sessionKey: string }).sessionKey,
    terminal.childSessionKey,
  );
  const childCalls = turns.get("worker");
  assert.equal(childCalls, 2);
  await sdk.callGatewayFromCli("fixture.releaseCompletion", opts, {
    childRunId: terminal.childRunId,
    childSessionKey: terminal.childSessionKey,
    expectedEndedAt: Date.parse(terminal.endedAt),
  });
  const after = await events();
  assert.equal(
    after.filter((e) => e.event === "sent").length,
    0,
    "Terminal/recovery cannot send as Subject",
  );
  assert.equal(
    after.filter((e) => e.event === "registered").length,
    0,
    "Completion cannot register a parent continuation",
  );
  assert.equal(
    turns.get("completion") ?? 0,
    0,
    "Completion cannot call main model",
  );
  assert.equal(
    turns.get("worker"),
    childCalls,
    "Exact recovery never reruns the paid child",
  );
  assert(
    after
      .filter((e) => e.event === "terminal")
      .every((e) => !e.continuation && e.parentRunId === parentRunId),
  );
  console.log(
    JSON.stringify({
      actualNativeParentWorkerCompletion: "PASS",
      exactSavedChildRecovery: "PASS",
      requiredHostAdmission: "PASS",
      terminalCustodyWithoutModelOrSend: "PASS",
      modelCalls,
      paidCalls: 0,
      qqTraffic: 0,
    }),
  );
} catch (error) {
  await writeFile(join(root, "gateway.log"), log);
  const diagnostic = await readFile(join(root, "native.log"), "utf8").catch(
    () => "",
  );
  throw new Error(
    "Native bindings fixture evidence " +
      root +
      " modelCalls=" +
      modelCalls +
      "\n" +
      log.slice(-4000) +
      "\n" +
      diagnostic.slice(-12000),
    { cause: error },
  );
} finally {
  if (child.exitCode === null) {
    const stopped = new Promise<void>((done) =>
      child.once("exit", () => done()),
    );
    child.kill();
    await stopped;
  }
  provider.closeAllConnections();
  await new Promise<void>((done) => provider.close(() => done()));
}
