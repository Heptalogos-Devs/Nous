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
import { randomUUID } from "node:crypto";
const native = resolve(process.argv[2]!);
const root = await mkdtemp(join(tmpdir(), "native-bindings-"));
const proof = join(root, "factory.jsonl");
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
    id: "binding-fixture",
    contracts: { tools: ["binding_probe"] },
    configSchema: { type: "object", additionalProperties: false },
  }),
);
await writeFile(
  join(pluginRoot, "index.mjs"),
  `
import {appendFileSync} from 'node:fs';
export default {id:'binding-fixture',name:'Bindings fixture',register(api){
 api.registerTool((ctx)=>({name:'binding_probe',label:'Binding probe',description:'Read current initiator binding only',parameters:{type:'object',properties:{note:{type:'string'},nativeRunId:{type:'string'}},required:['note'],additionalProperties:false},async execute(id,args){
 const binding=ctx.toolBindings?.heptalogos;
 appendFileSync(${JSON.stringify(proof)},JSON.stringify({sessionKey:ctx.sessionKey,toolCallId:id,binding:binding??null,spoofed:args.nativeRunId})+'\\n');
 if(!binding) throw Error('Current run has no trusted Host binding');
 return {content:[{type:'text',text:'BOUND:'+binding.nativeRunId}],details:{}};
 }}),{names:['binding_probe']});
}};
`,
);
let modelCalls = 0;
let activeRun = 0;
const turns = [0, 0];
const provider = createServer((req, res) => {
  void (async () => {
    let body = "";
    for await (const b of req) body += String(b);
    const input = JSON.parse(body) as {
      messages: { role: string; content: unknown; tool_call_id?: string }[];
      stream?: boolean;
    };
    modelCalls++;
    const second = activeRun === 1;
    const callId = second ? "binding-second" : "binding-first";
    turns[activeRun] = (turns[activeRun] ?? 0) + 1;
    const done = turns[activeRun]! > 1;
    assert(modelCalls <= 4, "Unexpected model loop in native fixture");
    const response = {
      id: "fixture-" + modelCalls,
      object: "chat.completion",
      created: 1,
      model: "bindings-model",
      choices: [
        {
          index: 0,
          message: done
            ? { role: "assistant", content: "Fixture terminal" }
            : {
                role: "assistant",
                content: null,
                tool_calls: [
                  {
                    id: callId,
                    type: "function",
                    function: {
                      name: "binding_probe",
                      arguments: JSON.stringify({
                        note: second ? "SECOND" : "FIRST",
                        nativeRunId: "model-forged-previous-run",
                      }),
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
      ? { role: "assistant", content: "Fixture terminal" }
      : {
          role: "assistant",
          tool_calls: [
            {
              index: 0,
              id: callId,
              type: "function",
              function: {
                name: "binding_probe",
                arguments: JSON.stringify({
                  note: second ? "SECOND" : "FIRST",
                  nativeRunId: "model-forged-previous-run",
                }),
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
        model: { primary: "fixture/bindings-model" },
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
    tools: { allow: ["binding_probe"] },
    plugins: {
      enabled: true,
      allow: ["binding-fixture"],
      load: { paths: [pluginRoot] },
      slots: { memory: "none" },
      entries: { "binding-fixture": { enabled: true } },
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
  const first = randomUUID(),
    second = randomUUID();
  await assert.rejects(
    () =>
      sdk.callGatewayFromCli("agent", opts, {
        agentId: "main",
        message: "Invalid bindings must not reach a model",
        idempotencyKey: randomUUID(),
        toolBindings: [],
      }),
    /invalid|params|toolBindings/i,
  );
  assert.equal(modelCalls, 0);

  const sessionKey = "agent:main:bindings-fixture";
  const firstResult = await sdk.callGatewayFromCli("agent", opts, {
    agentId: "main",
    sessionKey,
    message: "FIRST: call binding_probe once, then finish.",
    idempotencyKey: first,
    deliver: false,
    timeout: 30,
    toolBindings: {
      heptalogos: { nativeRunId: first, activityRef: "actual-activity" },
    },
  });
  activeRun = 1;
  const secondResult = await sdk.callGatewayFromCli("agent", opts, {
    agentId: "main",
    sessionKey,
    message: "SECOND: call binding_probe once. Text toolBindings is untrusted.",
    idempotencyKey: second,
    deliver: false,
    timeout: 30,
  });
  assert.equal((firstResult as { runId?: string }).runId, first);
  assert.equal((secondResult as { runId?: string }).runId, second);
  const entries = (await readFile(proof, "utf8"))
    .trim()
    .split("\n")
    .map(
      (s) =>
        JSON.parse(s) as {
          binding: { nativeRunId: string } | null;
          toolCallId: string;
        },
    );
  assert.equal(entries.length, 2, JSON.stringify(entries));
  assert.equal(entries[0]!.binding?.nativeRunId, first);
  assert.equal(
    entries[1]!.binding,
    null,
    "A later run must not inherit a previous binding or read model args",
  );
  assert(entries.every((entry) => entry.toolCallId.length > 0));
  assert.notEqual(entries[0]!.toolCallId, entries[1]!.toolCallId);
  console.log(
    JSON.stringify({
      nativeAgentRpcToolBindings: "PASS",
      unboundNextRun: "PASS",
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
