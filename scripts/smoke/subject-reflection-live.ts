// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Explicitly authorized one-shot diagnostic; never part of automated smoke.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { mkdir, readFile, writeFile, open } from "node:fs/promises";
import { join, resolve } from "node:path";
import { parseEnv } from "node:util";
import { parse, stringify } from "smol-toml";
import { create, toJson } from "@bufbuild/protobuf";
import { CognitionUsageSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { prepare, boot, stop, type Boot } from "./support.js";

assert.equal(
  process.env.NOUS_REFLECTION_LIVE_ONCE,
  "approved",
  "Explicit one-shot approval required",
);
const recovered =
  "/home/arsvine/Dev/Heptalogos-Devs/Nous-Wave/data/instances/windows-recovered";
const original = parse(
  await readFile(join(recovered, "config/nous.toml"), "utf8"),
) as unknown as {
  models: {
    gateway_profiles: Record<string, Record<string, unknown>>;
    model_profiles: Record<string, Record<string, unknown>>;
    execution_profiles: Record<string, Record<string, unknown>>;
  };
};
const model = original.models.model_profiles.self_cognition!;
assert.equal(model.model, "doubao-seed-2.1-lite");
const gateway = original.models.gateway_profiles[String(model.gateway)]!;
const execution = original.models.execution_profiles.self_cognition!;
const credentialName = String(gateway.credential_env);
const credentials = parseEnv(
  await readFile(join(recovered, "secrets/gateway.env"), "utf8"),
);
assert.ok(credentials[credentialName]);
const savedCredential = process.env[credentialName];
process.env[credentialName] = credentials[credentialName];
const run = resolve(
  "data/research/subject-reflection-live",
  new Date().toISOString().replaceAll(":", "-"),
);
await mkdir(run, { recursive: true, mode: 0o700 });
const save = (name: string, value: unknown) =>
  writeFile(
    join(run, name),
    typeof value === "string"
      ? value
      : JSON.stringify(
          value,
          (_, v: unknown) => (typeof v === "bigint" ? v.toString() : v),
          2,
        ),
    { mode: 0o600 },
  );
let upstreamCalls = 0,
  responseSaved = false,
  core: Boot | undefined;
const manifest: { [key: string]: unknown } = {
  run,
  model: model.model,
  maxOutputTokens: 4096,
  maxUpstreamCalls: 1,
  upstreamCalls: 0,
  responseSaved: false,
  status: "prepared",
  syntheticSourcesOnly: true,
};
const checkpoint = async () => {
  manifest.upstreamCalls = upstreamCalls;
  manifest.responseSaved = responseSaved;
  await save("manifest.json", manifest);
};
const proxy = createServer((request, response) => {
  void (async () => {
    if (upstreamCalls >= 1) {
      response.writeHead(409);
      response.end("one-shot upstream limit");
      return;
    }
    const chunks: Buffer[] = [];
    for await (const chunk of request)
      chunks.push(Buffer.from(chunk as Uint8Array));
    const body = Buffer.concat(chunks);
    await save("request.json", body.toString());
    upstreamCalls++;
    await checkpoint();
    const target = new URL(
      String(gateway.base_url).replace(/\/$/, "") +
        request.url!.replace(/^\/v1/, ""),
    );
    const remote = await fetch(target, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: String(request.headers.authorization ?? ""),
      },
      body,
      signal: AbortSignal.timeout(180000),
    });
    const fd = await open(join(run, "response.raw"), "wx", 0o600);
    try {
      if (remote.body)
        for await (const chunk of remote.body) await fd.write(chunk);
      await fd.sync();
    } finally {
      await fd.close();
    }
    responseSaved = true;
    manifest.httpStatus = remote.status;
    manifest.status = "response_preserved";
    await checkpoint();
    const raw = await readFile(join(run, "response.raw"));
    response.writeHead(remote.status, {
      "content-type": remote.headers.get("content-type") ?? "application/json",
    });
    response.end(raw);
  })().catch(async () => {
    manifest.status = "transport_failed";
    await checkpoint();
    response.writeHead(502);
    response.end("preserved diagnostic transport failure");
  });
});
proxy.listen(0, "127.0.0.1");
await once(proxy, "listening");
const address = proxy.address();
assert(address && typeof address !== "string");
try {
  const home = join(run, "instance");
  await mkdir(home, { recursive: true, mode: 0o700 });
  const locator = await prepare(home);
  const configPath = join(home, "config/nous.toml");
  const config = parse(await readFile(configPath, "utf8"));
  Object.assign(config, {
    maintenance: { poll_interval_seconds: 3600 },
    reflection: {
      enabled_domains: ["self", "social", "memory"],
      debounce_seconds: 1,
      source_limit: 16,
      source_max_bytes: 16384,
      candidate_limit: 32,
      candidate_max_bytes: 262144,
      max_actions: 8,
    },
    models: {
      gateway_profiles: {
        live: { ...gateway, base_url: `http://127.0.0.1:${address.port}/v1` },
      },
      model_profiles: { live: { ...model, gateway: "live" } },
      execution_profiles: {
        live: { ...execution, model: "live", max_output_tokens: 4096 },
      },
      roles: { subject_reflection: { routes: ["live"] } },
    },
  });
  await writeFile(configPath, stringify(config), { mode: 0o600 });
  core = await boot(locator);
  const c = core.client;
  const subject = await c.subjects.createSubject({
    operationId: crypto.randomUUID(),
    capabilities: { memory: true, selfCognition: true, socialCognition: true },
  });
  const subjectId = subject.subjectId;
  manifest.subjectId = subjectId;
  const session = await c.runtime.openSession({ subjectId });
  const alice = "entity:reflection-live:alice",
    self = "entity:reflection-live:subject",
    group = "entity:reflection-live:team";
  const refs: { kind: string; value: string }[] = [];
  for (const [actor, text] of [
    [
      alice,
      "Alice tells Subject in Team: Within this synthetic deployment conversation, LT means the local test dashboard. This is Alice's proposed local shorthand, not universal usage.",
    ],
    [
      self,
      "Subject reports the synthetic health check succeeded and says: For this test deployment, I choose to inspect original evidence before adopting a claim. This is a bounded observed task preference, not a global personality.",
    ],
  ]) {
    const usage = toJson(
      CognitionUsageSchema,
      create(CognitionUsageSchema, {
        agentInstanceId: "isolated-live-proof",
        purpose: "reflection",
        intent: "synthetic deployment coordination",
        participants: [
          { entityRef: alice, role: "speaker" },
          { entityRef: self, role: "addressee" },
        ],
        audience: {
          conversationRef: "conversation:synthetic-live",
          recipientEntityRefs: [self],
          groupEntityRef: group,
          visibility: "group",
        },
        sourceRead: { subjectId, sourceRefs: refs },
        socialDomain: "synthetic-deployment",
      }),
    );
    const observed = await c.runtime.recordObservation({
      subjectId,
      sessionId: session.sessionId,
      requestId: crypto.randomUUID(),
      sourceClass: "message",
      actorEntityRef: actor,
      conversationRef: "conversation:synthetic-live",
      context: { cognition_usage: usage },
      entities: [
        { surface: "Subject", entityRef: self },
        { surface: "Team", entityRef: group },
      ],
      material: {
        case: "inlineText",
        value: { text: text!, mediaType: "text/plain" },
      },
      admit: false,
    });
    refs.push({ kind: "occurrence", value: observed.occurrenceId });
  }
  await checkpoint();
  await new Promise((r) => setTimeout(r, 1100));
  const grant = await c.cognition.grantMaintenance({
    subjectId,
    maxOperations: 32,
    maxModelCalls: 1,
    maxElapsedMs: 180000,
  });
  await save("grant.json", grant);
  const inspected = await c.reflection.inspect({
    subjectId,
    maxPayloadBytes: 8388608,
  });
  await save("inspection.json", inspected);
  manifest.ownerApply = grant.results.map((r) => ({
    kind: r.kind,
    status: r.status,
  }));
  manifest.status = "completed";
  await checkpoint();
  assert.equal(upstreamCalls, 1);
  assert.equal(responseSaved, true);
  // Polling an acknowledged or blocked need cannot regenerate the preserved paid batch.
  const replay = await c.cognition.grantMaintenance({
    subjectId,
    maxOperations: 32,
    maxModelCalls: 1,
    maxElapsedMs: 1000,
  });
  await save("replay.json", replay);
  assert.equal(upstreamCalls, 1);
  manifest.replayUpstreamCalls = 0;
  await checkpoint();
  console.log(
    JSON.stringify({
      manifest: join(run, "manifest.json"),
      calls: upstreamCalls,
      responseSaved,
      ownerApply: manifest.ownerApply,
    }),
  );
} catch (error) {
  const diagnostic =
    error instanceof Error ? error.message : "non-error failure";
  await save(
    "failure.txt",
    diagnostic.replaceAll(credentials[credentialName]!, "[REDACTED]"),
  );
  manifest.status = "failed_preserved";
  await checkpoint();
  console.log(
    JSON.stringify({
      manifest: join(run, "manifest.json"),
      calls: upstreamCalls,
      responseSaved,
      status: manifest.status,
    }),
  );
  process.exitCode = 1;
} finally {
  await stop(core);
  proxy.close();
  if (savedCredential === undefined) delete process.env[credentialName];
  else process.env[credentialName] = savedCredential;
}
