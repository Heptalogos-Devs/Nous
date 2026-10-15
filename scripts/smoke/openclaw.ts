// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { setTimeout as delay } from "node:timers/promises";
import {
  createServer,
  Agent,
  request,
  type IncomingMessage,
  type ServerResponse,
} from "node:http";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createWriteStream } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { z } from "zod";
import { connectNousInstance } from "@nous-wave/client/node";
import { prepare, boot, stop, type Boot } from "./support.js";
import { workspaceTemp } from "../workspace.js";

// Uses a real operator-configured provider. Does not create credentials or replace the operator instance.
const configSchema = z
  .object({
    models: z
      .object({
        providers: z.record(
          z.string(),
          z
            .object({
              baseUrl: z.url(),
              models: z.array(z.object({ id: z.string() }).passthrough()),
            })
            .passthrough(),
        ),
      })
      .passthrough(),
  })
  .passthrough();
const nativeResponse = z
  .object({
    result: z
      .object({
        acceptedSessionSpawns: z
          .array(z.object({ runId: z.string() }))
          .optional(),
      })
      .passthrough()
      .optional(),
  })
  .passthrough();
const nativeRoot = resolve(
  process.argv[3] ?? "data/runtime/openclaw/node_modules/openclaw",
);
const configSource = resolve(
  process.argv[2] ?? "data/config/apps/openclaw.json",
);
const root = await workspaceTemp("smoke", "openclaw-");
const configPath = join(root, "openclaw.json");
const workspace = join(root, "workspace");
const state = join(root, "state");
const env = {
  ...process.env,
  OPENCLAW_STATE_DIR: state,
  OPENCLAW_CONFIG_PATH: configPath,
};
await mkdir(workspace, { recursive: true });
await writeFile(
  join(workspace, "evidence.txt"),
  "OPENCLAW_REAL_TOOL_RESULT: cognition is shared; each worker owns its cursor.\n",
);
const captures: {
  model?: string;
  slice: boolean;
  v1: boolean;
  v2: boolean;
  worker: boolean;
  tool: boolean;
  memory: boolean;
}[] = [];
let groundedMemoryRevision: string | undefined;
const config = configSchema.parse(
  JSON.parse(await readFile(configSource, "utf8")),
);
const provider = Object.values(config.models.providers)[0];
assert(provider?.models[0], "Configure at least one real model provider");
const upstream = new URL(provider.baseUrl);
const forwardingAgent = new Agent({ keepAlive: false });
assert.equal(
  upstream.protocol,
  "http:",
  "This local smoke uses the existing HTTP model gateway",
);
assert(
  ["127.0.0.1", "localhost"].includes(upstream.hostname),
  "Use a loopback gateway for the smoke",
);
async function forward(incoming: IncomingMessage, outgoing: ServerResponse) {
  const chunks: Buffer[] = [];
  for await (const chunk of incoming)
    chunks.push(Buffer.from(chunk as Uint8Array));
  const body = Buffer.concat(chunks);
  if (incoming.method === "POST") {
    const input = z
      .object({
        model: z.string().optional(),
        messages: z.array(z.object({ role: z.string(), content: z.unknown() })),
      })
      .passthrough()
      .parse(JSON.parse(body.toString()));
    const system = input.messages
      .filter((message) => message.role === "system")
      .map((message) =>
        typeof message.content === "string"
          ? message.content
          : JSON.stringify(message.content),
      )
      .join("\n");
    captures.push({
      model: input.model,
      slice: system.includes("Nous cognition"),
      v1: system.includes("GUIDANCE_V1"),
      v2: system.includes("GUIDANCE_V2"),
      worker: system.includes('"purpose":"worker"'),
      tool: JSON.stringify(input.messages).includes(
        "OPENCLAW_REAL_TOOL_RESULT",
      ),
      memory:
        !!groundedMemoryRevision &&
        system.includes(`memory_revision:${groundedMemoryRevision}`),
    });
  }
  const forwarded = request(
    new URL(incoming.url ?? "/", upstream.origin),
    {
      method: incoming.method,
      headers: { ...incoming.headers, host: upstream.host },
      agent: forwardingAgent,
    },
    (response) => {
      outgoing.writeHead(response.statusCode ?? 502, response.headers);
      response.pipe(outgoing);
    },
  );
  forwarded.on("error", () => {
    outgoing.writeHead(502);
    outgoing.end();
  });
  outgoing.once("close", () => {
    if (!outgoing.writableFinished) forwarded.destroy();
  });
  forwarded.end(body);
}
const capture = createServer((incoming, outgoing) => {
  void forward(incoming, outgoing).catch(() => {
    outgoing.writeHead(502);
    outgoing.end();
  });
});
await new Promise<void>((done) => capture.listen(0, "127.0.0.1", done));
const address = capture.address();
assert(address && typeof address === "object");
provider.baseUrl = `http://127.0.0.1:${address.port}${upstream.pathname}`;

async function cli(args: string[], expectedSuccess = true) {
  const child = spawn(
    process.execPath,
    [join(nativeRoot, "openclaw.mjs"), ...args],
    { env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] },
  );
  let output = "";
  let errors = "";
  child.stdout.on("data", (bytes: Buffer) => {
    output += bytes.toString();
  });
  // Native diagnostics remain local. Never echo provider auth, gateway config, or full input.
  child.stderr.on("data", (bytes: Buffer) => {
    errors += bytes.toString();
  });
  const code = await new Promise<number | null>((done) =>
    child.once("exit", done),
  );
  if ((code === 0) !== expectedSuccess) {
    const diagnostics = join(root, `${args[0]}-failure.log`);
    await writeFile(diagnostics, `${errors}\n${output}`, { mode: 0o600 });
    assert.fail(
      `Native ${args[0]} exit status ${code}; diagnostics=${diagnostics}`,
    );
  }
  return expectedSuccess ? nativeResponse.parse(JSON.parse(output)) : undefined;
}
const agent = (message: string, success = true) =>
  cli(
    [
      "agent",
      "--agent",
      "main",
      "--message",
      message,
      "--json",
      "--timeout",
      "120",
    ],
    success,
  );
const rpc = (method: string, params: object) =>
  cli([
    "gateway",
    "call",
    method,
    "--params",
    JSON.stringify(params),
    "--json",
    "--timeout",
    "120000",
  ]);
async function nativeReadResult(
  client: Awaited<ReturnType<typeof connectNousInstance>>,
  subjectId: string,
) {
  const artifacts = await client.material.listArtifacts({
    subjectId,
    page: { pageSize: 200 },
  });
  for (const artifact of artifacts.items) {
    const occurrences = await client.material.listOccurrences({
      subjectId,
      artifactId: artifact.artifactId,
      limit: 100,
    });
    const occurrence = occurrences.items.find(
      (item) =>
        item.sourceClass === "tool" && item.context?.tool_name === "read",
    );
    if (!occurrence) continue;
    const download = await client.artifacts.download(
      subjectId,
      artifact.artifactId,
    );
    const chunks: Uint8Array[] = [];
    for await (const bytes of download.content as AsyncIterable<Uint8Array>)
      chunks.push(bytes);
    if (Buffer.concat(chunks).toString().includes("OPENCLAW_REAL_TOOL_RESULT"))
      return occurrence;
  }
  throw new Error("Public Material must contain actual native read output");
}
let core: Boot | undefined;
let gateway: ChildProcess | undefined;
try {
  const locator = await prepare(root);
  core = await boot(locator);
  const client = core.client;
  const subject = await client.subjects.createSubject({
    operationId: crypto.randomUUID(),
    capabilities: { memory: true },
  });
  const subjectId = subject.subjectId; // Deliberately no Seed.
  const created = await client.runtime.createWorkContext({
    subjectId,
    operationId: crypto.randomUUID(),
    purpose: "Native OpenClaw cognition smoke",
    contextText:
      "GUIDANCE_V1: Use actual tool results; ordinary remember/forget is conversation, not management authorization.",
  });
  const context = created.workContext!;
  const gatewayToken = crypto.randomUUID();
  await writeFile(
    configPath,
    JSON.stringify(
      {
        ...config,
        gateway: {
          mode: "local",
          bind: "loopback",
          port: 0,
          auth: { mode: "token", token: gatewayToken },
        },
        agents: {
          defaults: {
            workspace,
            skipBootstrap: true,
            model: {
              primary: `${Object.keys(config.models.providers)[0]}/${provider.models[0]!.id}`,
            },
            compaction: { memoryFlush: { enabled: false } },
          },
        },
        tools: { profile: "full" },
        plugins: {
          allow: ["nous-cognition"],
          load: { paths: [join(dirname(nativeRoot), "@nous-wave/openclaw")] },
          slots: { contextEngine: "nous-cognition", memory: "none" },
          entries: {
            "nous-cognition": {
              enabled: true,
              config: {
                runRoot: join(root, "run"),
                subjectId,
                consumerId: "consumer:smoke:cognitive-runtime-episode",
                workContextId: context.workContextId,
                cognitionTokenCap: 4096,
              },
            },
          },
        },
      },
      null,
      2,
    ),
  );
  // Reserve an ephemeral native Gateway port before writing its configuration.
  const portProbe = createServer();
  await new Promise<void>((done) => portProbe.listen(0, "127.0.0.1", done));
  const port = portProbe.address();
  assert(port && typeof port === "object");
  await new Promise<void>((done) => portProbe.close(() => done()));
  const gatewayConfig = JSON.parse(await readFile(configPath, "utf8")) as {
    gateway: { port: number };
  };
  gatewayConfig.gateway.port = port.port;
  await writeFile(configPath, JSON.stringify(gatewayConfig, null, 2));
  gateway = spawn(
    process.execPath,
    [join(nativeRoot, "openclaw.mjs"), "gateway", "run"],
    { env, windowsHide: true, stdio: ["pipe", "pipe", "pipe"] },
  );
  const gatewayLog = createWriteStream(join(root, "gateway.log"), {
    mode: 0o600,
  });
  gateway.stdout!.pipe(gatewayLog, { end: false });
  gateway.stderr!.pipe(gatewayLog, { end: false });
  gateway.once("exit", () => gatewayLog.end());
  // Native readiness is an HTTP contract, independent of diagnostic wording.
  gateway.stdout!.resume();
  gateway.stderr!.resume();
  const readinessDeadline = Date.now() + 60000;
  for (;;) {
    if (gateway.exitCode !== null)
      throw new Error("Native Gateway exited before ready");
    const ready = await fetch(`http://127.0.0.1:${port.port}/readyz`, {
      signal: AbortSignal.timeout(1000),
    }).catch(() => undefined);
    if (ready?.status === 200) break;
    if (Date.now() >= readinessDeadline)
      throw new Error("Native Gateway readiness timeout");
    await delay(250);
  }
  await agent(
    "Read evidence.txt using the read tool, then quote its actual contents.",
  );
  assert(
    captures.some((input) => input.v1 && input.tool),
    "Tool continuation must contain applied cognition and real result",
  );
  const actualRead = await nativeReadResult(client, subjectId);
  // Explicit operator adoption from the actual native observation. Chat itself
  // does not acquire this management operation.
  const memory = await client.memory.formMemory({
    subjectId,
    operationId: crypto.randomUUID(),
    cognitiveRole: "declarative",
    input: {
      formationMode: "grounded",
      groundingOccurrenceId: actualRead.occurrenceId,
      semanticRole: "fact",
      text: "Cognition is shared; each worker owns its cursor.",
      epistemicClass: "observed",
      validTime: {},
      basis: [
        {
          basis: {
            case: "evidence",
            value: {
              occurrenceId: actualRead.occurrenceId,
              locator: { case: "wholeOccurrence", value: true },
              basisRole: "direct",
            },
          },
        },
      ],
    },
  });
  groundedMemoryRevision = memory.revisionId;
  const revised = await client.runtime.updateWorkContext({
    subjectId,
    workContextId: context.workContextId,
    expectedRevision: context.revision,
    operationId: crypto.randomUUID(),
    intent: {
      case: "fields",
      value: {
        contextText:
          "GUIDANCE_V2: Current guidance replaces V1. Read real files before reporting.",
      },
    },
  });
  await client.runtime.updateWorkContext({
    subjectId,
    workContextId: context.workContextId,
    expectedRevision: revised.workContext!.revision,
    operationId: crypto.randomUUID(),
    intent: {
      case: "pin",
      value: {
        cognitionAnchors: [
          { kind: "memory_revision", value: memory.revisionId },
        ],
      },
    },
  });
  const resetAt = captures.length;
  await agent("Read evidence.txt again and summarize it in one sentence.");
  assert(
    captures
      .slice(resetAt)
      .every((input) => input.v2 && !input.v1 && input.memory),
    "RESET must remove old cognition from system input",
  );
  const compactAt = captures.length;
  await rpc("sessions.compact", { key: "agent:main:main" });
  assert(
    captures.slice(compactAt).every((input) => !input.slice && !input.v2),
    "Native compactor must not absorb Nous-owned slice",
  );
  const spawned = await agent(
    'Use sessions_spawn exactly once with runtime=subagent, context=isolated, mode=run, task="Read evidence.txt with read and quote it". Then sessions_yield until its real result arrives.',
  );
  const child = spawned?.result?.acceptedSessionSpawns?.[0];
  assert(child, "Native sessions_spawn must accept an actual child run");
  await rpc("agent.wait", { runId: child.runId, timeoutMs: 120000 });
  assert(
    captures.some((input) => input.worker && input.v2 && input.tool),
    "Child tool continuation must apply same Subject cognition independently",
  );
  await agent("Give one sentence based on the completed child result.");
  const beforeFailure = captures.length;
  await stop(core);
  core = undefined;
  await agent("Read evidence.txt before replying.", false);
  assert.equal(
    captures.length,
    beforeFailure,
    "Unavailable required cognition must stop before provider transmission",
  );
  core = await boot(locator);
  await agent("Read evidence.txt after recovery and quote the real result.");
  const switchedModel = provider.models[1];
  if (switchedModel)
    await rpc("sessions.patch", {
      key: "agent:main:main",
      model: `${Object.keys(config.models.providers)[0]}/${switchedModel.id}`,
    });
  await rpc("sessions.reset", { key: "agent:main:main", reason: "new" });
  const newSessionAt = captures.length;
  await agent(
    "Read evidence.txt and describe how independent workers share cognition.",
  );
  assert(
    captures
      .slice(newSessionAt)
      .every(
        (input) =>
          input.v2 &&
          !input.v1 &&
          input.memory &&
          (!switchedModel || input.model === switchedModel.id),
      ),
    "Native reset/model switch must retain current Subject cognition",
  );
  const reconnected = await connectNousInstance({ runRoot: join(root, "run") });
  const sessions = await reconnected.runtime.listSessions({
    subjectId,
    page: { pageSize: 100 },
  });
  assert(
    sessions.items.length >= 3,
    "Child and reset native session must have independent Nous Sessions",
  );
  await nativeReadResult(reconnected, subjectId);
  assert.equal(
    (await reconnected.memory.getMemory({ subjectId, id: memory.memoryId }))
      .text,
    memory.text,
  );
  await writeFile(
    join(root, "result.json"),
    JSON.stringify({
      noSeed: true,
      reset: true,
      compact: true,
      subagent: true,
      outcome: true,
      unavailableStopsProvider: true,
      coreRestart: true,
      nativeSessionReset: true,
      memoryApplied: true,
      modelSwitch: !!switchedModel,
      providerInputs: captures.length,
    }),
  );
  process.stdout.write(
    `OPENCLAW_SMOKE_OK no_seed=true reset=true compact=true subagent=true outcome=true unavailable=true restart=true evidence=${root}\n`,
  );
} finally {
  await writeFile(
    join(root, "provider-captures.json"),
    JSON.stringify(captures),
    { mode: 0o600 },
  );
  if (gateway?.exitCode === null && gateway.signalCode === null) {
    const exited = once(gateway, "exit");
    gateway.kill();
    await exited;
  }
  await stop(core);
  forwardingAgent.destroy();
  capture.closeAllConnections();
  await new Promise<void>((done) => capture.close(() => done()));
}
