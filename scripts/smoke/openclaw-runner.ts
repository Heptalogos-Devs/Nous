// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { setTimeout as delay } from "node:timers/promises";
import { createServer, Agent, request } from "node:http";
import { createWriteStream } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { z } from "zod";
import type { NousClient } from "@nous-wave/client";

const nativeProviderConfiguration = z.object({
  models: z
    .object({
      providers: z.record(
        z.string(),
        z
          .object({
            baseUrl: z.url(),
            apiKey: z.unknown(),
            models: z.array(z.object({ id: z.string() }).passthrough()),
          })
          .passthrough(),
      ),
    })
    .passthrough(),
});
type NativeCapture = {
  stage: string;
  body: { model?: string; messages: { role: string; content: unknown }[] };
};

/** Controls the installed native CLI/Gateway and observes its real HTTP sends. */
export async function nativeRunner(
  home: string,
  nativeRoot: string,
  configurationFile: string,
) {
  const configuration = nativeProviderConfiguration.parse(
    JSON.parse(await readFile(configurationFile, "utf8")),
  );
  const providerName = Object.keys(configuration.models.providers)[0]!;
  const provider = configuration.models.providers[providerName]!;
  assert(provider?.models[0]);
  const upstream = new URL(provider.baseUrl);
  assert(
    upstream.protocol === "http:" &&
      ["127.0.0.1", "localhost"].includes(upstream.hostname),
    "The real provider must use the existing loopback HTTP gateway",
  );
  const transport = new Agent({ keepAlive: false });
  const captureFile = join(home, "actual-provider-inputs.json");
  const captures: NativeCapture[] = await readFile(captureFile, "utf8")
    .then((text) =>
      z
        .array(
          z.object({
            stage: z.string(),
            body: z
              .object({
                model: z.string().optional(),
                messages: z.array(
                  z
                    .object({ role: z.string(), content: z.unknown() })
                    .passthrough(),
                ),
              })
              .passthrough(),
          }),
        )
        .parse(JSON.parse(text)),
    )
    .catch(() => []);
  let stage = "setup";
  const proxy = createServer((incoming, outgoing) => {
    void (async () => {
      const chunks: Buffer[] = [];
      for await (const chunk of incoming)
        chunks.push(Buffer.from(chunk as Uint8Array));
      const bytes = Buffer.concat(chunks);
      if (incoming.method === "POST")
        captures.push({
          stage,
          body: z
            .object({
              model: z.string().optional(),
              messages: z.array(
                z
                  .object({ role: z.string(), content: z.unknown() })
                  .passthrough(),
              ),
            })
            .passthrough()
            .parse(JSON.parse(bytes.toString())),
        });
      const forwarded = request(
        new URL(incoming.url ?? "/", upstream.origin),
        {
          method: incoming.method,
          headers: { ...incoming.headers, host: upstream.host },
          agent: transport,
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
      forwarded.end(bytes);
    })().catch(() => {
      outgoing.writeHead(502);
      outgoing.end();
    });
  });
  proxy.listen(0, "127.0.0.1");
  await once(proxy, "listening");
  const address = proxy.address();
  assert(address && typeof address !== "string");
  const proxyUrl = `http://127.0.0.1:${address.port}${upstream.pathname}`;
  provider.baseUrl = proxyUrl;
  const workspace = join(home, "workspace");
  await mkdir(workspace, { recursive: true });
  const configPath = join(home, "openclaw.json");
  const environment = {
    ...process.env,
    OPENCLAW_STATE_DIR: join(home, "state"),
    OPENCLAW_CONFIG_PATH: configPath,
  };
  let gateway: ChildProcess | undefined;
  let commandIndex = 0;
  const commandGroup = crypto.randomUUID();
  const probe = createServer();
  probe.listen(0, "127.0.0.1");
  await once(probe, "listening");
  const port = probe.address();
  assert(port && typeof port !== "string");
  const gatewayPort = port.port;
  await new Promise<void>((done) => probe.close(() => done()));
  async function command(args: string[]) {
    const child = spawn(
      process.execPath,
      [join(nativeRoot, "openclaw.mjs"), ...args],
      {
        env: environment,
        windowsHide: true,
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    let output = "",
      diagnostic = "";
    child.stdout!.on("data", (bytes: Buffer) => {
      output += bytes.toString();
    });
    child.stderr!.on("data", (bytes: Buffer) => {
      diagnostic += bytes.toString();
    });
    const exited: unknown[] = await once(child, "exit");
    const code = exited[0];
    const path = join(home, `native-${commandGroup}-${++commandIndex}.json`);
    await writeFile(path, output, { mode: 0o600 });
    if (code !== 0) {
      await writeFile(path + ".log", diagnostic, { mode: 0o600 });
      throw new Error(
        `Native ${args[0]} failed (${String(code)}); diagnostics=${path}`,
      );
    }
    return JSON.parse(output) as unknown;
  }
  const rpc = (method: string, params: object) =>
    command([
      "gateway",
      "call",
      method,
      "--params",
      JSON.stringify(params),
      "--json",
      "--timeout",
      "240000",
    ]);
  async function stopGateway() {
    if (gateway?.exitCode === null && gateway.signalCode === null) {
      const ended = once(gateway, "exit");
      gateway.kill();
      await ended;
    }
    gateway = undefined;
  }
  async function startGateway() {
    gateway = spawn(
      process.execPath,
      [join(nativeRoot, "openclaw.mjs"), "gateway", "run"],
      { env: environment, windowsHide: true, stdio: ["pipe", "pipe", "pipe"] },
    );
    const log = createWriteStream(join(home, "gateway.log"), {
      flags: "a",
      mode: 0o600,
    });
    gateway.stdout!.pipe(log, { end: false });
    gateway.stderr!.pipe(log, { end: false });
    gateway.once("exit", () => log.end());
    const deadline = Date.now() + 60000;
    for (;;) {
      if (gateway.exitCode !== null)
        throw new Error("Native Gateway exited before readiness");
      const response = await fetch(`http://127.0.0.1:${gatewayPort}/readyz`, {
        signal: AbortSignal.timeout(1000),
      }).catch(() => undefined);
      if (response?.status === 200) return;
      if (Date.now() >= deadline) throw new Error("Native readiness deadline");
      await delay(250);
    }
  }
  return {
    workspace,
    captures,
    providerName,
    provider,
    proxyUrl,
    setStage(value: string) {
      stage = value;
    },
    async bind(subjectId: string, workContextId?: string, reset = true) {
      await stopGateway();
      await writeFile(
        configPath,
        JSON.stringify(
          {
            models: configuration.models,
            gateway: {
              mode: "local",
              bind: "loopback",
              port: gatewayPort,
              auth: { mode: "token", token: crypto.randomUUID() },
            },
            agents: {
              defaults: {
                workspace,
                skipBootstrap: true,
                model: { primary: `${providerName}/${provider.models[0]!.id}` },
                compaction: { memoryFlush: { enabled: false } },
              },
            },
            tools: { profile: "full" },
            plugins: {
              allow: ["nous-cognition"],
              load: {
                paths: [join(dirname(nativeRoot), "@nous-wave/openclaw")],
              },
              slots: { contextEngine: "nous-cognition", memory: "none" },
              entries: {
                "nous-cognition": {
                  enabled: true,
                  config: {
                    runRoot: join(home, "run"),
                    subjectId,
                    consumerId: "consumer:self-native",
                    workContextId,
                    cognitionTokenCap: 8192,
                  },
                },
              },
            },
          },
          null,
          2,
        ),
        { mode: 0o600 },
      );
      await startGateway();
      if (reset)
        await rpc("sessions.reset", { key: "agent:main:main", reason: "new" });
    },
    async restart() {
      await stopGateway();
      await startGateway();
    },
    rpc,
    async agent(message: string) {
      const response = await command([
        "agent",
        "--agent",
        "main",
        "--message",
        message,
        "--json",
        "--timeout",
        "180",
      ]);
      const result = z
        .object({
          result: z
            .object({
              payloads: z.array(
                z
                  .object({
                    text: z.string().optional(),
                    isError: z.boolean().optional(),
                    isReasoning: z.boolean().optional(),
                  })
                  .passthrough(),
              ),
            })
            .passthrough(),
        })
        .parse(response);
      const text = result.result.payloads
        .filter((payload) => !payload.isError && !payload.isReasoning)
        .map((payload) => payload.text ?? "")
        .join("\n")
        .trim();
      assert(text, "Actual native final expression must be nonempty");
      await writeFile(join(home, `${stage}.txt`), text, { mode: 0o600 });
      return text;
    },
    async close() {
      await stopGateway();
      await writeFile(
        join(home, "actual-provider-inputs.json"),
        JSON.stringify(captures),
        { mode: 0o600 },
      );
      transport.destroy();
      proxy.closeAllConnections();
      await new Promise<void>((done) => proxy.close(() => done()));
    },
  };
}

export async function nativeGeneratedSource(
  client: NousClient & {
    artifacts: {
      download(
        subject: string,
        id: string,
      ): Promise<{ content: AsyncIterable<Uint8Array> }>;
    };
  },
  subjectId: string,
  text: string,
) {
  const artifacts = await client.material.listArtifacts({
    subjectId,
    page: { pageSize: 200 },
  });
  for (const artifact of artifacts.items.toReversed()) {
    const occurrences = await client.material.listOccurrences({
      subjectId,
      artifactId: artifact.artifactId,
      limit: 100,
    });
    const source = occurrences.items.find(
      (item) =>
        item.sourceClass === "generated" && item.context?.role === "assistant",
    );
    if (!source) continue;
    const content = await client.artifacts.download(
      subjectId,
      artifact.artifactId,
    );
    const bytes: Uint8Array[] = [];
    for await (const chunk of content.content) bytes.push(chunk);
    const raw = Buffer.concat(bytes).toString();
    if (raw.includes(text) || text.includes(raw)) return source;
  }
  throw new Error(
    "Actual native generation must exist in public Material before Self interpretation",
  );
}
