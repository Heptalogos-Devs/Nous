// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { selfTraceSmoke } from "./self-trace.js";
import { selfCliSmoke } from "./self-cli.js";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { appendFile } from "node:fs/promises";
import { join } from "node:path";
import { once } from "node:events";
import { prepare, boot, stop, type Boot } from "./support.js";
import { workspaceTemp } from "../workspace.js";

// Deterministic provider contract, not evidence of live-model judgment or behavior.
let proposal: unknown;
let calls = 0;
let fail = false;
let mutateDuringGeneration: (() => Promise<void>) | undefined;
let lastInput:
  | {
      basisCatalog: { key: string }[];
      currentSelf: { key: string; record: { reference: { value: string } } }[];
    }
  | undefined;
const provider = createServer((request, response) => {
  void (async () => {
    const bytes: Buffer[] = [];
    for await (const chunk of request)
      bytes.push(Buffer.from(chunk as Uint8Array));
    calls++;
    const body = JSON.parse(Buffer.concat(bytes).toString()) as {
      messages: { role: string; content: string }[];
      response_format: { type: string; json_schema: { strict: boolean } };
    };
    assert.equal(body.response_format.type, "json_schema");
    assert.equal(body.response_format.json_schema.strict, true);
    lastInput = JSON.parse(
      body.messages.find((message) => message.role === "user")!.content,
    ) as typeof lastInput;
    await mutateDuringGeneration?.();
    response.setHeader("Content-Type", "application/json");
    if (fail) {
      response.writeHead(503);
      response.end('{"error":"synthetic-provider-secret"}');
      return;
    }
    response.end(
      JSON.stringify({
        id: "self-contract",
        created: 1,
        model: "self-contract",
        choices: [
          {
            index: 0,
            message: { role: "assistant", content: JSON.stringify(proposal) },
            finish_reason: "stop",
          },
        ],
        usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
      }),
    );
  })().catch(() => response.destroy());
});
provider.listen(0, "127.0.0.1");
await once(provider, "listening");
const address = provider.address();
assert(address && typeof address !== "string");
const home = await workspaceTemp("smoke", "self-model-");
let core: Boot | undefined;
const oldCredential = process.env.NOUS_SELF_MODEL_CONTRACT;
process.env.NOUS_SELF_MODEL_CONTRACT = "synthetic-local-contract";
try {
  const locator = await prepare(home);
  await appendFile(
    join(home, "config/nous.toml"),
    `
[models.gateway_profiles.contract]
base_url="http://127.0.0.1:${address.port}/v1"
credential_env="NOUS_SELF_MODEL_CONTRACT"
[models.model_profiles.contract]
gateway="contract"
protocol="openai-chat"
model="self-contract"
capabilities=["text","structured_output"]
[models.execution_profiles.contract]
model="contract"
[models.roles.self_formation]
routes=["contract"]
[models.roles.self_reflection]
routes=["contract"]
`,
  );
  core = await boot(locator);
  let c = core.client;
  const empty = await c.subjects.createSubject({
    operationId: crypto.randomUUID(),
    capabilities: { selfCognition: true },
  });
  assert.equal(
    (await c.self.list({ subjectId: empty.subjectId, page: { pageSize: 100 } }))
      .items.length,
    0,
  );
  assert.equal(calls, 0);
  const subject = await c.subjects.createSubject({
    operationId: crypto.randomUUID(),
    capabilities: { selfCognition: true },
    cognitiveSeed: {
      format: "text/markdown",
      text: "# 初始选择\n名字：青砚\n背景故事是虚构设定，未提供真实个人经历。",
    },
  });
  const subjectId = subject.subjectId;
  const seed = await c.subjects.getCognitiveSeed({ subjectId });
  const source = { case: "seedVersionId" as const, value: seed.seedVersionId };
  const facet = (statement: string) => ({
    family: "self_facet",
    kind: "identity",
    statement,
    stance: "adopted",
    epistemicClass: "narrative",
    scope: { description: "general", purposes: [], bindToInvocation: false },
    exceptions: [],
    basisKeys: ["source:line:2"],
    validTime: { kind: "unknown" },
  });
  const request = { subjectId, operationId: crypto.randomUUID(), source };
  proposal = {
    actions: [
      {
        action: "adopt",
        content: facet("当前自称青砚。"),
        reason: "采用 Seed 中的自称，未补充背景经历。",
      },
    ],
  };
  const [first, concurrent] = await Promise.all([
    c.model.interpretSelf(request),
    c.model.interpretSelf(request),
  ]);
  assert.equal(calls, 1);
  assert.deepEqual(first, concurrent);
  assert.equal(first.domain, "self");
  assert.equal(first.status, "applied");
  const original = first.actions[0]!.record!;
  assert.equal(original.content?.content.case, "facet");
  if (original.content?.content.case !== "facet")
    throw new Error("Missing facet");
  assert.equal(
    original.content.content.value.producer?.operation,
    "self_formation_text",
  );
  assert.equal(
    original.content.content.value.producer?.modelIdentity,
    "self-contract",
  );
  assert.equal(original.content.content.value.basis[0]?.basis.case, "seed");
  assert(
    lastInput?.basisCatalog.some((entry) => entry.key === "source:line:2"),
  );
  await assert.rejects(
    c.model.interpretSelf({
      ...request,
      source: { case: "seedVersionId", value: crypto.randomUUID() },
    }),
  );
  assert.equal(calls, 1);
  await stop(core);
  core = await boot(locator);
  c = core.client;
  assert.deepEqual(await c.model.interpretSelf(request), first);
  assert.equal(calls, 1);
  const observation = await c.runtime.recordObservation({
    subjectId,
    requestId: crypto.randomUUID(),
    sourceClass: "generated",
    material: {
      case: "inlineText",
      value: {
        text: "我自主决定把自称改为青岚；青砚保留为旧称。",
        mediaType: "text/plain",
      },
    },
    admit: false,
  });
  const reflection = {
    subjectId,
    operationId: crypto.randomUUID(),
    source: {
      case: "observation" as const,
      value: { occurrenceId: observation.occurrenceId },
    },
  };
  proposal = {
    actions: [
      {
        action: "revise",
        targetKey: "self:0",
        content: {
          ...facet("当前自称青岚，过去自称青砚。"),
          epistemicClass: "observed",
          basisKeys: ["source"],
        },
        reason: "实际自我选择改变同一身份事项。",
      },
    ],
  };
  const changed = await c.model.interpretSelf(reflection);
  assert.equal(changed.status, "applied");
  const revised = changed.actions[0]!.record!;
  assert.equal(revised.objectRef?.value, original.objectRef?.value);
  assert.notEqual(revised.reference?.value, original.reference?.value);
  assert.equal(revised.parentRevisionId, original.reference?.value);
  assert.equal(
    lastInput?.currentSelf[0]?.record.reference.value,
    original.reference?.value,
  );
  const old = await c.self.getRevision({
    subjectId,
    id: original.reference!.value,
  });
  assert.deepEqual(old.content, original.content);
  const invoke = () =>
    c.model.interpretSelf({ ...reflection, operationId: crypto.randomUUID() });
  proposal = {
    actions: [
      {
        action: "keep",
        targetKey: "self:0",
        reason: "来源没有进一步改变已经采纳的自称。",
      },
    ],
  };
  assert.equal((await invoke()).status, "kept");
  assert.equal(
    (
      await c.self.listRevisions({
        subjectId,
        objectRef: original.objectRef,
        page: { pageSize: 100 },
      })
    ).items.length,
    2,
  );
  proposal = {
    actions: [
      {
        action: "evidence",
        targetKey: "self:0",
        role: "support",
        basisKey: "source",
        reason: "补充相同实际选择的证据。",
      },
    ],
  };
  const evidence = await invoke();
  assert.equal(evidence.status, "applied");
  assert.equal(
    evidence.actions[0]?.record?.reference?.value,
    revised.reference?.value,
  );
  assert.equal(evidence.actions[0]?.record?.evidence.length, 1);
  proposal = {
    actions: [
      { action: "keep", targetKey: null, reason: "没有新变化。" },
      {
        action: "adopt",
        content: { ...facet("非法来源不允许发布。"), basisKeys: ["invented"] },
        reason: "必须拒绝虚构引用。",
      },
    ],
  };
  const invalidRequest = { ...reflection, operationId: crypto.randomUUID() };
  const invalid = await c.model.interpretSelf(invalidRequest);
  assert.equal(invalid.status, "failed");
  assert.equal(invalid.degradation[0]?.code, "self_proposal_rejected");
  const beforeInvalidReplay = calls;
  assert.deepEqual(await c.model.interpretSelf(invalidRequest), invalid);
  assert.equal(calls, beforeInvalidReplay);
  assert.equal(
    (await c.self.list({ subjectId, page: { pageSize: 100 } })).items.length,
    1,
  );
  proposal = {
    actions: [
      {
        action: "adopt",
        content: {
          family: "narrative_thread",
          theme: "自主选择自称",
          interpretation: "当前认识来自实际自我选择，未制造过往经历。",
          epistemicClass: "derived",
          sourceKeys: ["self:0"],
          basisKeys: ["source"],
          validTime: { kind: "unknown" },
        },
        reason: "将当前身份的来源组织为主题。",
      },
    ],
  };
  assert.equal((await invoke()).status, "applied");
  // Change the target after the paid input snapshot: CAS rejects rather than overwriting.
  mutateDuringGeneration = async () => {
    const target = await c.self.get({
      subjectId,
      id: original.objectRef!.value,
    });
    await c.self.addEvidence({
      subjectId,
      operationId: crypto.randomUUID(),
      targetRevision: target.reference,
      expectedObjectEpoch: target.objectEpoch,
      role: "counterexample",
      basis: {
        basis: {
          case: "evidence",
          value: {
            occurrenceId: observation.occurrenceId,
            locator: { case: "wholeOccurrence", value: true },
            basisRole: "direct",
          },
        },
      },
    });
  };
  proposal = {
    actions: [
      { action: "keep", targetKey: null, reason: "其他事项无需变化。" },
      {
        action: "revise",
        targetKey: "self:0",
        content: { ...facet("不允许覆盖已变化版本。"), basisKeys: ["source"] },
        reason: "测试真实 CAS。",
      },
    ],
  };
  const conflict = await invoke();
  assert.equal(conflict.status, "partial");
  assert.equal(conflict.actions[0]?.status, "kept");
  assert.equal(conflict.actions[1]?.status, "failed");
  mutateDuringGeneration = undefined;
  fail = true;
  const failed = await c.model.interpretSelf({
    ...request,
    operationId: crypto.randomUUID(),
  });
  assert.equal(failed.status, "failed");
  assert(failed.degradation.length > 0);
  assert(!JSON.stringify(failed).includes("synthetic-provider-secret"));
  assert.equal(
    (await c.subjects.getCognitiveSeed({ subjectId })).seed?.text,
    seed.seed?.text,
  );
  assert.equal(
    (await c.subjects.getSubject({ subjectId })).subjectId,
    subjectId,
  );
  assert.equal(
    (await c.self.getRevision({ subjectId, id: original.reference!.value }))
      .reference?.value,
    original.reference!.value,
  );
  await selfTraceSmoke(c, subjectId, original);
  const beforeCli = calls;
  await selfCliSmoke(home, subjectId, request.operationId);
  assert.equal(calls, beforeCli);
  const purgeTarget = await c.self.get({
    subjectId,
    id: original.objectRef!.value,
  });
  await c.self.purge({
    subjectId,
    operationId: crypto.randomUUID(),
    objectRef: purgeTarget.objectRef,
    expectedObjectEpoch: purgeTarget.objectEpoch,
  });
  await assert.rejects(c.model.interpretSelf(request), /purged/);
  await assert.rejects(c.model.interpretSelf(reflection), /purged/);
  const beforePurgeReplay = calls;
  await stop(core);
  core = await boot(locator);
  c = core.client;
  await assert.rejects(c.model.interpretSelf(request), /purged/);
  assert.equal(calls, beforePurgeReplay);
  console.log(
    "Self model contracts passed: paid replay/restart, source-bound Seed/reflection, revise/keep/evidence/narrative, CAS and explicit domain failure with preserved source.",
  );
} finally {
  await stop(core);
  if (oldCredential === undefined) delete process.env.NOUS_SELF_MODEL_CONTRACT;
  else process.env.NOUS_SELF_MODEL_CONTRACT = oldCredential;
  await new Promise<void>((done) => provider.close(() => done()));
}
