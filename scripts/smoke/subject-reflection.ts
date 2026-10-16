// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { appendFile } from "node:fs/promises";
import { join } from "node:path";
import { create, toJson, clone } from "@bufbuild/protobuf";
import {
  CognitionUsageSchema,
  RevisionBasisSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { SelfContentSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/self_pb.js";
import { prepare, boot, stop, type Boot } from "./support.js";
import { workspaceTemp } from "../workspace.js";
import { runCli } from "../../apps/nous-cli/src/commands/index.js";
// Deterministic model contract over real isolated PostgreSQL. This is not QQ learning.
type Input = {
  actualSources: {
    key: string;
    scopeKey: string;
    source: { context: unknown; partial?: boolean };
  }[];
  currentSelf: { key: string; record: unknown }[];
  currentSocial: { key: string; record: unknown }[];
  entityCatalog: { key: string; entityRef: string }[];
  enabledDomains: string[];
  candidateScope: { completeSubjectSnapshot: boolean };
};
let calls = 0,
  input: Input | undefined,
  phase: "adopt" | "keep" | "revise" = "adopt";
let during: (() => Promise<void>) | undefined;
const selfContent = (text: string) => ({
  family: "self_facet",
  kind: "preference",
  statement: text,
  stance: "tentative",
  epistemicClass: "inferred",
  scope: {
    description: "deployment coordination",
    purposes: [],
    bindToInvocation: true,
  },
  exceptions: [],
  basisKeys: ["source:1"],
  validTime: { kind: "unknown" },
});
const provider = createServer((request, response) => {
  void (async () => {
    const chunks: Buffer[] = [];
    for await (const chunk of request)
      chunks.push(Buffer.from(chunk as Uint8Array));
    const body = JSON.parse(Buffer.concat(chunks).toString()) as {
      messages: { role: string; content: string }[];
    };
    input = JSON.parse(
      body.messages.find((m) => m.role === "user")!.content,
    ) as Input;
    calls++;
    await during?.();
    during = undefined;
    const actions =
      phase === "adopt"
        ? [
            {
              domain: "self",
              scopeKey: "source:1",
              proposal: {
                action: "adopt",
                content: selfContent(
                  "In this deployment I prefer checking original evidence first",
                ),
                reason:
                  "Actual observed bounded task choice; no global personality inferred",
              },
            },
            {
              domain: "social",
              scopeKey: "source:0",
              proposal: {
                action: "adopt",
                content: {
                  family: "language_convention",
                  expression: "LT",
                  variants: [],
                  caseInsensitive: false,
                  meaning: "the deployment dashboard",
                  referentKey: null,
                  pragmaticUse: "local work shorthand",
                  judgment: {
                    state: "hypothesis",
                    explanation:
                      "Alice's actual explanation in this conversation; not universal shared understanding",
                    estimate: null,
                    estimateMethod: null,
                  },
                  basisKeys: ["source:0"],
                  validTime: { kind: "unknown" },
                },
                reason: "Retain the attributed local explanation",
              },
            },
            {
              domain: "memory",
              scopeKey: "source:1",
              proposal: {
                action: "adopt",
                content: {
                  text: "The isolated deployment health check passed",
                  semanticRole: "task_outcome",
                  epistemicClass: "reported",
                  entityKeys: [],
                  basisKeys: ["source:1"],
                  validTime: { kind: "unknown" },
                },
                reason: "Actual Host-observed task outcome",
              },
            },
          ]
        : phase === "revise"
          ? [
              {
                domain: "self",
                scopeKey: "source:0",
                proposal: {
                  action: "revise",
                  targetKey: input!.currentSelf[0]!.key,
                  content: {
                    ...selfContent(
                      "For deployment I prefer checking two independent files",
                    ),
                    basisKeys: ["source:0"],
                  },
                  reason: "Actual later correction",
                },
              },
              {
                domain: "social",
                scopeKey: "source:0",
                proposal: {
                  action: "keep",
                  targetKey: null,
                  reason:
                    "The correction does not establish a new group convention",
                },
              },
            ]
          : [
              {
                domain: "self",
                scopeKey: "source:0",
                proposal: {
                  action: "keep",
                  targetKey: null,
                  reason: "No durable change supported by this actual batch",
                },
              },
            ];
    response.setHeader("content-type", "application/json");
    response.end(
      JSON.stringify({
        id: "reflection-contract",
        created: 1,
        model: "reflection-contract",
        choices: [
          {
            index: 0,
            message: {
              role: "assistant",
              content: JSON.stringify({ actions }),
            },
            finish_reason: "stop",
          },
        ],
        usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
      }),
    );
  })().catch((error) => {
    console.error(error);
    response.destroy();
  });
});
provider.listen(0, "127.0.0.1");
await once(provider, "listening");
const address = provider.address();
assert(address && typeof address !== "string");
const home = await workspaceTemp("smoke", "subject-reflection-");
let core: Boot | undefined;
const prior = process.env.NOUS_REFLECTION_CONTRACT;
process.env.NOUS_REFLECTION_CONTRACT = "local-deterministic-no-paid-provider";
try {
  const locator = await prepare(home);
  await appendFile(
    join(home, "config/nous.toml"),
    `\n[database]\nmax_connections=1\n[maintenance]\npoll_interval_seconds=3600\n[reflection]\nenabled_domains=["self","social","memory"]\ndebounce_seconds=1\nsource_limit=16\nsource_max_bytes=4096\ncandidate_limit=32\ncandidate_max_bytes=262144\nmax_actions=16\n[models.gateway_profiles.contract]\nbase_url="http://127.0.0.1:${address.port}/v1"\ncredential_env="NOUS_REFLECTION_CONTRACT"\n[models.model_profiles.contract]\ngateway="contract"\nprotocol="openai-chat"\nmodel="reflection-contract"\ncapabilities=["text","structured_output"]\n[models.execution_profiles.contract]\nmodel="contract"\n[models.roles.subject_reflection]\nroutes=["contract"]\n`,
  );
  core = await boot(locator);
  const c = core.client;
  const subject = await c.subjects.createSubject({
    operationId: crypto.randomUUID(),
    capabilities: { memory: true, selfCognition: true, socialCognition: true },
  });
  const subjectId = subject.subjectId;
  const session = await c.runtime.openSession({ subjectId });
  const work = await c.runtime.createWorkContext({
    subjectId,
    operationId: crypto.randomUUID(),
    purpose: "isolated actual deployment coordination",
    contextText: "Verify deployment source facts",
  });
  await c.runtime.setActiveWorkContext({
    operationId: crypto.randomUUID(),
    subjectId,
    sessionId: session.sessionId,
    expectedRuntimeRevision: session.runtimeRevision,
    workContextId: work.workContext!.workContextId,
  });
  const alice = "entity:reflection:alice",
    self = "entity:reflection:subject",
    group = "entity:reflection:team";
  let readable: { kind: string; value: string }[] = [];
  const usage = () =>
    toJson(
      CognitionUsageSchema,
      create(CognitionUsageSchema, {
        agentInstanceId: "reflection-contract",
        purpose: "reflection",
        intent: "deployment coordination",
        participants: [
          { entityRef: alice, role: "speaker" },
          { entityRef: self, role: "addressee" },
        ],
        audience: {
          conversationRef: "conversation:reflection-team",
          recipientEntityRefs: [self],
          groupEntityRef: group,
          visibility: "group",
        },
        sourceRead: { subjectId, sourceRefs: readable },
        socialDomain: "deployment",
      }),
    );
  const requestUsages = new Map<string, ReturnType<typeof usage>>();
  const frozenUsage = (request: string) => {
    let value = requestUsages.get(request);
    if (!value) {
      value = usage();
      requestUsages.set(request, value);
    }
    return value;
  };
  const observe = (
    text: string,
    actor = alice,
    sourceClass = "message",
    requestId = crypto.randomUUID(),
  ) =>
    c.runtime.recordObservation({
      subjectId,
      sessionId: session.sessionId,
      requestId,
      sourceClass,
      actorEntityRef: actor,
      conversationRef: "conversation:reflection-team",
      context: { cognition_usage: frozenUsage(requestId) },
      entities: [
        { surface: "Subject", entityRef: self },
        { surface: "Team", entityRef: group },
      ],
      material: {
        case: "inlineText",
        value: { text, mediaType: "text/plain" },
      },
      admit: false,
    });
  const firstRequest = crypto.randomUUID();
  const firstText =
    "Alice tells Subject in Team: LT means the deployment dashboard in this conversation.";
  const first = await observe(firstText, alice, "message", firstRequest);
  readable.push({ kind: "occurrence", value: first.occurrenceId });
  const second = await observe(
    "Subject and Alice in Team actually completed the isolated deployment health check; Subject chose to inspect original source evidence first.",
    self,
    "host_event",
  );
  readable.push({ kind: "occurrence", value: second.occurrenceId });
  const pending = await c.reflection.inspect({ subjectId });
  assert.equal(pending.pending.length, 1);
  assert.equal(calls, 0, "Receiver must never run the model");
  const revision = pending.pending[0]!.triggerRevision;
  await observe(firstText, alice, "message", firstRequest);
  assert.equal(
    (await c.reflection.inspect({ subjectId })).pending[0]!.triggerRevision,
    revision,
    "Observation replay cannot create a new trigger",
  );
  await new Promise((resolve) => setTimeout(resolve, 1100));
  const grant = () =>
    c.cognition.grantMaintenance({
      subjectId,
      maxOperations: 32,
      maxModelCalls: 1,
      maxElapsedMs: 30000,
    });
  const firstGrant = await grant();
  assert.equal(firstGrant.modelCalls, 1, JSON.stringify(firstGrant));
  assert.equal(calls, 1);
  const diagnostic = await c.reflection.inspect({
    subjectId,
    maxPayloadBytes: 8388608,
  });
  const firstOutcome = JSON.parse(
    diagnostic.recent[0]?.outcomeJson ?? "{}",
  ) as {
    result?: {
      actions?: {
        domain: string;
        status: string;
        reason: string;
        problemCode: string;
      }[];
    };
  };
  assert.ok(
    firstGrant.results.some(
      (r) => r.kind === "subject_reflection" && r.status === "applied",
    ),
    JSON.stringify({
      grant: firstGrant,
      actions: firstOutcome.result?.actions?.map((a) => ({
        domain: a.domain,
        status: a.status,
        reason: a.reason,
        problem: a.problemCode,
      })),
    }),
  );
  assert.equal(input?.actualSources.length, 2);
  assert.equal(input?.candidateScope.completeSubjectSnapshot, false);
  const adopted = (await c.self.list({ subjectId, page: { pageSize: 100 } }))
    .items[0]!;
  assert.equal(
    adopted.content?.content.value?.producer?.operation,
    "subject_reflection_text",
  );
  const learned = (await c.social.list({ subjectId, page: { pageSize: 100 } }))
    .items[0]!;
  assert.equal(learned.content?.content.case, "convention");
  const memories = await c.memory.listMemories({
    subjectId,
    page: { pageSize: 100 },
  });
  assert.equal(memories.items.length, 1);
  readable.push(adopted.reference!, learned.reference!, {
    kind: "memory_revision",
    value: memories.items[0]!.revisionId,
  });
  const after = await c.reflection.inspect({ subjectId });
  assert.equal(after.lastStatus, "applied");
  assert.equal(after.pending.length, 0);
  assert.ok(after.recent[0]?.outcomeJson?.includes("subject_reflection_text"));
  await grant();
  assert.equal(calls, 1, "Polling processed material cannot generate again");
  phase = "keep";
  await observe(
    "Alice and Subject in Team confirm the deployment outcome; no new durable preference.",
  );
  await new Promise((resolve) => setTimeout(resolve, 1100));
  during = async () => {
    await observe(
      "Alice and Subject in Team add a genuinely later contact while the old generation is running.",
    );
  };
  const secondGrant = await grant();
  assert.equal(secondGrant.modelCalls, 1);
  assert.equal(calls, 2);
  assert.equal(
    (await c.reflection.inspect({ subjectId })).pending.length,
    1,
    "Old acknowledgement must leave later trigger pending",
  );
  await grant();
  assert.equal(calls, 3);
  assert.equal(
    input?.actualSources.length,
    1,
    "Later batch advances from captured through-position",
  );
  phase = "revise";
  const correction = await observe(
    "Alice and Subject in Team explicitly correct the deployment practice: inspect two independent files.",
  );
  readable.push({ kind: "occurrence", value: correction.occurrenceId });
  await new Promise((resolve) => setTimeout(resolve, 1100));
  let competing: typeof adopted | undefined;
  during = async () => {
    const content = clone(SelfContentSchema, adopted.content!);
    if (content.content.case === "facet") {
      content.content.value.statement =
        "In this deployment I prefer a reviewed checklist";
      content.content.value.basis = [
        create(RevisionBasisSchema, {
          basis: {
            case: "evidence",
            value: {
              occurrenceId: correction.occurrenceId,
              locator: { case: "wholeOccurrence", value: true },
              basisRole: "direct",
            },
          },
        }),
      ];
    }
    competing = await c.self.revise({
      subjectId,
      operationId: crypto.randomUUID(),
      targetRevision: adopted.reference,
      expectedObjectEpoch: adopted.objectEpoch,
      content,
      reason: "Actual competing reviewed edit during model execution",
    });
  };
  const conflict = await grant();
  assert.ok(
    conflict.results.some(
      (r) => r.kind === "subject_reflection" && r.status === "partial",
    ),
    JSON.stringify(conflict),
  );
  assert.equal(calls, 4);
  assert.equal(
    (await c.self.get({ subjectId, id: adopted.objectRef!.value })).reference
      ?.value,
    competing!.reference?.value,
    "Stale paid proposal cannot overwrite the new head",
  );
  const partial = await c.reflection.inspect({
    subjectId,
    maxPayloadBytes: 8388608,
  });
  assert.equal(partial.lastStatus, "partial");
  const paid = partial.recent[0]!;
  const saved = JSON.parse(paid.outcomeJson!) as {
    result: { actions: { actionId: string; status: string }[] };
  };
  const stale = saved.result.actions.find((a) => a.status === "failed")!;
  const repaired = await c.reflection.repair({
    subjectId,
    operationId: paid.operationId,
    semanticDigest: paid.semanticDigest,
    repairId: crypto.randomUUID(),
    resolutions: [
      {
        actionId: stale.actionId,
        exactHead: competing!.reference,
        expectedObjectEpoch: competing!.objectEpoch,
      },
    ],
  });
  assert.equal(repaired.status, "applied");
  assert.equal(
    calls,
    4,
    "Explicit reviewed repair reuses retained paid result",
  );
  assert.equal((await c.reflection.inspect({ subjectId })).pending.length, 0);
  phase = "keep";
  await observe(
    `Subject and Team actual long source ${"bounded evidence ".repeat(1000)}`,
  );
  await new Promise((resolve) => setTimeout(resolve, 1100));
  await grant();
  assert.ok(input?.actualSources.some((s) => s.source.partial));
  assert.equal(calls, 5);
  const cli = (await runCli([
    "--run-root",
    join(home, "run"),
    "--instance-root",
    join(home, "reflection-cli"),
    "--subject",
    subjectId,
    "reflection",
    "status",
  ])) as { data: { lastStatus: string } };
  assert.equal(cli.data.lastStatus, "kept");
  const current = await c.self.get({ subjectId, id: adopted.objectRef!.value });
  await c.self.setLifecycle({
    mutation: {
      subjectId,
      operationId: crypto.randomUUID(),
      objectRef: current.objectRef,
      expectedObjectEpoch: current.objectEpoch,
    },
    action: "suppress",
  });
  await new Promise((resolve) => setTimeout(resolve, 1100));
  const changed = await grant();
  assert.equal(
    calls,
    5,
    "Unavailable source metadata cannot manufacture new external evidence or a model call",
  );
  assert.ok(
    changed.results.some(
      (r) =>
        r.kind === "subject_reflection" &&
        r.problemCode === "source_change_requires_actual_evidence",
    ),
  );
  assert.ok(
    (await c.reflection.inspect({ subjectId })).pending.some(
      (n) => n.state === "blocked",
    ),
  );
  console.log(
    "Shared reflection PostgreSQL smoke passed: actual-source batching, no receiver generation, request dedupe, three owners/one generation, keep/replay, trigger fencing, retained paid partial and exact-head repair, partial source and CLI status.",
  );
} finally {
  if (core) await stop(core);
  provider.closeAllConnections();
  provider.close();
  if (prior === undefined) delete process.env.NOUS_REFLECTION_CONTRACT;
  else process.env.NOUS_REFLECTION_CONTRACT = prior;
}
