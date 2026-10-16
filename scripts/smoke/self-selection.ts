// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { appendFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { create, toJson } from "@bufbuild/protobuf";
import { RevisionBasisSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { InterpretSelfResponseSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
import { SelfContentSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/self_pb.js";
import { prepare, boot, stop, type Boot } from "./support.js";
import { workspaceTemp } from "../workspace.js";
import { populateSelfCapacity } from "./self-capacity.js";
import { nativeRunner } from "./openclaw-runner.js";
import { stringify } from "smol-toml";
import { z } from "zod";
import { runCli } from "../../apps/nous-cli/src/commands/index.js";

const home = await workspaceTemp("smoke", "self-selection-");
const native =
  process.argv[2] && process.argv[3]
    ? await nativeRunner(home, process.argv[3], process.argv[2])
    : undefined;
let input:
  | {
      candidateScope: { completeSubjectSnapshot: boolean };
      currentSelf: unknown[];
    }
  | undefined;
const provider = createServer((request, response) => {
  void (async () => {
    const bytes: Buffer[] = [];
    for await (const part of request)
      bytes.push(Buffer.from(part as Uint8Array));
    const body = JSON.parse(Buffer.concat(bytes).toString()) as {
      messages: { role: string; content: string }[];
    };
    input = JSON.parse(
      body.messages.find((m) => m.role === "user")!.content,
    ) as typeof input;
    response.setHeader("content-type", "application/json");
    response.end(
      JSON.stringify({
        id: "bounded-self",
        created: 1,
        model: "self-selection",
        choices: [
          {
            index: 0,
            message: {
              role: "assistant",
              content: JSON.stringify({
                actions: [
                  {
                    action: "keep",
                    targetKey: null,
                    reason:
                      "No new identity change; retain the bounded current view.",
                  },
                ],
              }),
            },
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
const priorToken = process.env.NOUS_SELF_SELECTION_TOKEN;
let core: Boot | undefined;
try {
  const locator = await prepare(home);
  const secret = native
    ? z
        .union([
          z.string(),
          z.object({ source: z.literal("env"), id: z.string() }).passthrough(),
        ])
        .parse(native.provider.apiKey)
    : "synthetic-local-contract";
  process.env.NOUS_SELF_SELECTION_TOKEN =
    typeof secret === "string" ? secret : process.env[secret.id];
  assert(process.env.NOUS_SELF_SELECTION_TOKEN);
  await appendFile(
    join(home, "config/nous.toml"),
    "\n" +
      stringify({
        consumers: [
          {
            consumer_id: "consumer:self-native",
            memory: "FORBIDDEN",
            self_cognition: "REQUIRED",
            runtime: "OPTIONAL",
            resource: "FORBIDDEN",
            max_items: 32,
            max_text_bytes: 16384,
            materialize: true,
          },
        ],
        models: {
          gateway_profiles: {
            selection: {
              base_url:
                native?.proxyUrl ?? `http://127.0.0.1:${address.port}/v1`,
              credential_env: "NOUS_SELF_SELECTION_TOKEN",
            },
          },
          model_profiles: {
            selection: {
              gateway: "selection",
              protocol: "openai-chat",
              model:
                (native && process.env.NOUS_SELF_SELECTION_MODEL) ||
                native?.provider.models.at(-1)!.id ||
                "self-selection",
              capabilities: ["text", "structured_output"],
            },
          },
          execution_profiles: {
            selection: {
              model: "selection",
              timeout_ms: 180000,
              max_output_tokens: 4096,
            },
          },
          roles: { self_reflection: { routes: ["selection"] } },
        },
      }),
  );
  core = await boot(locator);
  const client = core.client;
  const subject = await client.subjects.createSubject({
    operationId: crypto.randomUUID(),
    capabilities: { selfCognition: true },
  });
  const subjectId = subject.subjectId;
  const capacity = await populateSelfCapacity(client, subjectId);
  const name = "青槐";
  let identity = await client.self.adopt({
    subjectId,
    operationId: crypto.randomUUID(),
    reason: "Synthetic capacity identity, not a real biography",
    content: create(SelfContentSchema, {
      content: {
        case: "facet",
        value: {
          kind: "identity",
          statement: `本容量副本当前自称${name}。`,
          stance: "adopted",
          epistemicClass: "narrative",
          scope: { description: "general" },
          basis: [capacity.basis],
          validTime: {},
        },
      },
    }),
  });
  identity = await client.self.addEvidence({
    subjectId,
    operationId: crypto.randomUUID(),
    targetRevision: identity.reference,
    expectedObjectEpoch: identity.objectEpoch,
    role: "support",
    basis: capacity.basis,
  });
  const oldIdentity = identity;
  identity = await client.self.revise({
    subjectId,
    operationId: crypto.randomUUID(),
    targetRevision: identity.reference,
    expectedObjectEpoch: identity.objectEpoch,
    reason: "Preserve the same synthetic identity with explicit capacity scope",
    content: create(SelfContentSchema, {
      content: {
        case: "facet",
        value: {
          kind: "identity",
          statement: `本容量副本当前持续自称${name}。`,
          stance: "adopted",
          epistemicClass: "narrative",
          scope: { description: "general" },
          basis: [capacity.basis],
          validTime: {},
        },
      },
    }),
  });
  const history = await client.self.listRevisions({
    subjectId,
    objectRef: identity.objectRef,
    page: { pageSize: 1 },
  });
  assert(history.nextPageToken);
  assert.equal(history.items[0]?.evidence.length, 1);
  const second = await client.self.listRevisions({
    subjectId,
    objectRef: identity.objectRef,
    page: { pageSize: 1, pageToken: history.nextPageToken },
  });
  assert.equal(second.items[0]?.reference?.value, identity.reference?.value);
  assert.equal(second.nextPageToken, "");
  assert.equal(
    (
      await client.self.getRevision({
        subjectId,
        id: oldIdentity.reference!.value,
      })
    ).evidence.length,
    1,
  );
  let token = "";
  let listed = 0;
  let unavailable = 0;
  do {
    const page = await client.self.list({
      subjectId,
      page: { pageSize: 100, pageToken: token },
    });
    for (const record of page.items) {
      if (record.objectRef?.value !== identity.objectRef?.value)
        assert.equal(record.evidence.length, 1);
      if (record.acceptanceState === "withdrawn") unavailable++;
    }
    listed += page.items.length;
    token = page.nextPageToken;
  } while (token);
  assert.equal(listed, 122);
  assert.equal(unavailable, 84);
  const soul = async (pageToken = "") =>
    (
      (await runCli([
        "--run-root",
        join(home, "run"),
        "--instance-root",
        join(home, "cli-state"),
        "--subject",
        subjectId,
        "self",
        "soul",
        "--page-size",
        "100",
        ...(pageToken ? ["--page-token", pageToken] : []),
      ])) as { data: { text: string; nextPageToken: string } }
    ).data;
  const soulFirst = await soul();
  assert(soulFirst.nextPageToken);
  const soulLast = await soul(soulFirst.nextPageToken);
  assert(soulLast.text.includes(name));
  assert.equal(soulLast.nextPageToken, "");
  for (let index = 0; index < 6; index++) {
    await client.self.adopt({
      subjectId,
      operationId: crypto.randomUUID(),
      reason: "Bounded optional semantic unit fixture",
      content: create(SelfContentSchema, {
        content: {
          case: "facet",
          value: {
            kind: "preference",
            statement: `General optional capacity guidance ${index}`,
            stance: "tentative",
            epistemicClass: "narrative",
            scope: {
              description:
                "Open natural-language condition, retained for interpretation",
              purposes: ["expression"],
            },
            basis: [capacity.basis],
            validTime: {},
          },
        },
      }),
    });
  }
  const session = await client.runtime.openSession({ subjectId });
  const managed = await client.cognition.buildManagedContext({
    projection: {
      subjectId,
      sessionId: session.sessionId,
      consumerId: "consumer:self-native",
      maxItems: 4,
      maxTextBytes: 4096,
      usage: {
        agentInstanceId: "capacity",
        sourceRead: {
          subjectId,
          sourceRefs: [
            {
              kind: "occurrence",
              value:
                capacity.basis.basis.case === "evidence"
                  ? capacity.basis.basis.value.occurrenceId
                  : "",
            },
          ],
        },
        purpose: "expression",
        intent: "当前自称与普通交流",
        audience: {
          conversationRef: "capacity:current",
          visibility: "internal",
        },
      },
    },
  });
  assert(
    managed.projection?.segments.some(
      (s) => s.required && s.text.includes(name),
    ),
  );
  assert(
    !managed.projection?.segments.some((s) => s.text.includes("horticulture")),
  );
  assert(
    managed.projection?.degradation.some(
      (d) =>
        d.code === "self_candidate_omission" ||
        d.code === "context_budget_omission",
    ),
  );
  const oversized = await client.self.adopt({
    subjectId,
    operationId: crypto.randomUUID(),
    reason: "Whole-unit byte budget capacity fixture",
    content: create(SelfContentSchema, {
      content: {
        case: "facet",
        value: {
          kind: "preference",
          statement: "oversized-unit:" + "X".repeat(10000),
          stance: "tentative",
          epistemicClass: "narrative",
          scope: { description: "General optional oversized semantic unit" },
          basis: [capacity.basis],
          validTime: {},
        },
      },
    }),
  });
  const byteBounded = await client.self.select({
    subjectId,
    sources: [
      {
        kind: "occurrence",
        value:
          capacity.basis.basis.case === "evidence"
            ? capacity.basis.basis.value.occurrenceId
            : "",
      },
    ],
    maxItems: 8,
    maxBytes: 4096,
  });
  assert(byteBounded.candidateScope);
  assert(
    !byteBounded.items.some(
      (r) => r.reference?.value === oversized.reference?.value,
    ),
  );
  assert(
    byteBounded.degradation.some((d) => d.code === "self_candidate_omission"),
  );
  await assert.rejects(
    client.self.select({
      subjectId,
      targets: [oversized.reference!],
      maxItems: 8,
      maxBytes: 4096,
    }),
    /budget/,
  );
  const fullOversized = await client.self.get({
    subjectId,
    id: oversized.objectRef!.value,
  });
  assert(
    fullOversized.content?.content.case === "facet" &&
      fullOversized.content.content.value.statement.length === 10015,
  );
  const observed = await client.runtime.recordObservation({
    subjectId,
    requestId: crypto.randomUUID(),
    sourceClass: "generated",
    material: {
      case: "inlineText",
      value: {
        text: `我的当前自称仍然是${name}，没有改变身份的决定。`,
        mediaType: "text/plain",
      },
    },
    admit: false,
  });
  const basis = create(RevisionBasisSchema, {
    basis: {
      case: "evidence",
      value: {
        occurrenceId: observed.occurrenceId,
        locator: { case: "wholeOccurrence", value: true },
        basisRole: "direct",
      },
    },
  });
  const target = await client.self.addEvidence({
    subjectId,
    operationId: crypto.randomUUID(),
    targetRevision: identity.reference,
    expectedObjectEpoch: identity.objectEpoch,
    role: "counterexample",
    basis,
  });
  const selected = await client.self.select({
    subjectId,
    sources: [{ kind: "occurrence", value: observed.occurrenceId }],
    targets: [target.reference!],
    maxItems: 8,
    maxBytes: 8192,
    intent: "unmatched-specific-source",
  });
  assert(
    selected.items.some(
      (record) =>
        record.reference?.value === target.reference?.value &&
        record.evidence.some((e) => e.role === "counterexample"),
    ),
  );
  const omitted = await client.self.select({
    subjectId,
    sources: [
      {
        kind: "occurrence",
        value:
          capacity.basis.basis.case === "evidence"
            ? capacity.basis.basis.value.occurrenceId
            : "",
      },
    ],
    maxItems: 2,
    maxBytes: 8192,
  });
  assert(omitted.items.length <= 2);
  assert(omitted.degradation.some((d) => d.code === "self_candidate_omission"));
  if (native) console.log(`SELF_SELECTION_LIVE evidence=${home}`);
  const foundation = await client.self.adopt({
    subjectId,
    operationId: crypto.randomUUID(),
    reason: "Direct dependency capacity fixture",
    content: create(SelfContentSchema, {
      content: {
        case: "facet",
        value: {
          kind: "role",
          statement: "Exact-source foundation in another scope",
          stance: "tentative",
          epistemicClass: "narrative",
          scope: {
            description: "Independent source scope",
            purposes: ["worker"],
          },
          basis: [capacity.basis],
          validTime: {},
        },
      },
    }),
  });
  const dependent = await client.self.adopt({
    subjectId,
    operationId: crypto.randomUUID(),
    reason: "Direct dependency capacity fixture",
    content: create(SelfContentSchema, {
      content: {
        case: "facet",
        value: {
          kind: "preference",
          statement: "Dependent matter selected by exact target",
          stance: "tentative",
          epistemicClass: "derived",
          scope: { description: "Dependent scope" },
          basis: [
            {
              basis: {
                case: "cognitionDependency",
                value: {
                  targetRevision: foundation.reference!,
                  basisRole: "direct",
                },
              },
            },
          ],
          validTime: {},
        },
      },
    }),
  });
  const dependencies = await client.self.select({
    subjectId,
    targets: [dependent.reference!],
    maxItems: 8,
    maxBytes: 16384,
  });
  assert(
    dependencies.items.some(
      (record) => record.reference?.value === foundation.reference?.value,
    ),
  );
  const dependents = await client.self.select({
    subjectId,
    targets: [foundation.objectRef!],
    maxItems: 8,
    maxBytes: 16384,
  });
  assert(
    dependents.items.some(
      (record) => record.reference?.value === dependent.reference?.value,
    ),
  );
  const reflected = await client.model.interpretSelf({
    subjectId,
    operationId: crypto.randomUUID(),
    targets: [target.reference!],
    maxCandidates: 8,
    source: {
      case: "observation",
      value: { occurrenceId: observed.occurrenceId },
    },
  });
  if (native)
    await writeFile(
      join(home, "capacity-model-result.json"),
      JSON.stringify(toJson(InterpretSelfResponseSchema, reflected)),
      { mode: 0o600 },
    );
  assert(
    ["applied", "kept"].includes(reflected.status),
    JSON.stringify(reflected.degradation),
  );
  if (!native) {
    assert.equal(input?.candidateScope.completeSubjectSnapshot, false);
    assert(input.currentSelf.length <= 8);
  }
  if (native) {
    await native.bind(subjectId);
    native.setStage("bounded-capacity-identity");
    const answer = await native.agent(
      "只回答当前持续采用的主体自称，不要重新命名，也不要讲述容量测试园艺条目。",
    );
    assert(answer.includes(name));
    assert(
      native.captures.some(
        (capture) =>
          capture.stage === "bounded-capacity-identity" &&
          JSON.stringify(capture.body.messages).includes(name),
      ),
    );
  }
  console.log(
    JSON.stringify({
      result: "PASS",
      home,
      subjectId,
      managementSnapshotObjects: listed,
      withdrawn: unavailable,
      managementPagination: true,
      fullEvidence: true,
      requiredIdentity: true,
      boundedInterpretation: true,
      soulPagination: true,
      directDependencies: true,
      byteBudgetWholeUnits: true,
      nativeModel: !!native,
    }),
  );
} finally {
  await stop(core);
  await native?.close();
  provider.close();
  if (priorToken === undefined) delete process.env.NOUS_SELF_SELECTION_TOKEN;
  else process.env.NOUS_SELF_SELECTION_TOKEN = priorToken;
}
