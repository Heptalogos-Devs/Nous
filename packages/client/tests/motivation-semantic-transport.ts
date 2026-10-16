import { ValueSchema } from "@bufbuild/protobuf/wkt";
import { MaintenanceGrantRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/journal_pb.js";
import { encodeCognitionUsage } from "../src/host-bridge.js";
import { grantMaintenance } from "../../../apps/nous-core/src/maintenance/grants.js";
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Deterministic provider contract; this is not live-model semantic-quality evidence.
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { createServer } from "node:http";
import { once } from "node:events";
import { readFile, writeFile } from "node:fs/promises";
import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { createGrpcTransport } from "@connectrpc/connect-node";
import { createNousClient } from "../src/index.js";
import { EvaluateConditionRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/motivation_pb.js";
import { KernelClient } from "../../../apps/nous-core/src/kernel-client.js";
import { ModelRuntime } from "../../../apps/nous-core/src/model/runtime.js";
import { modelConfigurationSchema } from "../../../apps/nous-core/src/model/configuration.js";
import { evaluateMotivation } from "../../../apps/nous-core/src/model/motivation-evaluation.js";
const [phase, endpoint, subjectId, sourceId, statePath] = process.argv.slice(
  2,
) as [string, string, string, string, string];
const transport = () =>
  createGrpcTransport({
    baseUrl: endpoint,
    interceptors: [
      (next) => async (request) => {
        request.header.set("authorization", "Bearer motivation-fixture");
        return next(request);
      },
    ],
  });
const client = createNousClient(transport());
if (phase === "resume") {
  const request = fromJson(
    EvaluateConditionRequestSchema,
    JSON.parse(await readFile(statePath, "utf8")) as JsonValue,
  );
  const result = await evaluateMotivation(
    new KernelClient(transport()),
    new ModelRuntime(),
    request,
    {},
  );
  assert.equal(result.verdict, "satisfied");
  assert.equal(result.coverage, "limited");
  assert.equal(result.leaves[0]?.method.startsWith("recorded_model:"), true);
  assert.equal(
    result.leaves[0]?.producer?.operation,
    "subject_reflection_text",
  );
  assert(result.leaves[0]?.producer?.outputSchemaDigest);
  console.log(
    "Separate process resumed saved semantic result without a configured provider PASS",
  );
} else {
  let calls = 0,
    invalid = false;
  const provider = createServer((req, res) => {
    void (async () => {
      let raw = "";
      for await (const c of req) raw += String(c);
      calls++;
      const body = JSON.parse(raw) as {
        response_format: { json_schema: { strict: boolean; name: string } };
        messages: { role: string; content: string }[];
      };
      assert.equal(body.response_format.json_schema.strict, true);
      const input = JSON.parse(
        body.messages.find((m) => m.role === "user")!.content,
      ) as {
        operation?: string;
        criteria?: { key: string; materials: { key: string }[] }[];
        actualSources?: { key: string; scopeKey: string }[];
      };
      const semantic = input.operation === "motivation_criterion";
      assert.equal(
        body.response_format.json_schema.name,
        semantic ? "motivation_criterion" : "subject_reflection",
      );
      const source = input.actualSources?.[0];
      const output = semantic
        ? {
            results: input.criteria!.map((c) => ({
              key: c.key,
              truth: "true",
              reason: "Deterministic fixture selected actual provided material",
              evidenceKeys: [invalid ? "not-in-catalog" : c.materials[0]!.key],
            })),
          }
        : {
            actions: [
              {
                domain: "motivation",
                scopeKey: source!.scopeKey,
                proposal: {
                  action: "adopt",
                  reason:
                    "Actual expressed interest, not an external one-time commission",
                  content: {
                    family: "desire",
                    matterKey: "bounded-learning",
                    statement: "I would like to learn about this bounded topic",
                    basisKeys: [source!.key],
                    validTime: { kind: "unknown" },
                    links: [],
                    condition: null,
                  },
                },
              },
            ],
          };
      res.setHeader("content-type", "application/json");
      res.end(
        JSON.stringify({
          id: "criterion-fixture",
          created: 1,
          model: "criterion-fixture",
          choices: [
            {
              index: 0,
              message: { role: "assistant", content: JSON.stringify(output) },
              finish_reason: "stop",
            },
          ],
          usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
        }),
      );
    })().catch((e) => {
      res.statusCode = 500;
      res.end(String(e));
    });
  });
  provider.listen(0, "127.0.0.1");
  await once(provider, "listening");
  const address = provider.address();
  assert(address && typeof address !== "string");
  process.env.NOUS_CRITERION_FIXTURE = "synthetic-local-token";
  const models = await ModelRuntime.fromConfig(
    modelConfigurationSchema.parse({
      gateway_profiles: {
        fixture: {
          base_url: `http://127.0.0.1:${address.port}/v1`,
          credential_env: "NOUS_CRITERION_FIXTURE",
        },
      },
      model_profiles: {
        fixture: {
          gateway: "fixture",
          protocol: "openai-chat",
          model: "criterion-fixture",
          capabilities: ["text", "structured_output"],
        },
      },
      execution_profiles: { fixture: { model: "fixture" } },
      roles: { subject_reflection: { routes: ["fixture"] } },
    }),
  );
  try {
    const condition = await client.motivation.adopt({
      subjectId,
      operationId: randomUUID(),
      reason: "Actual semantic evaluator fixture",
      content: {
        family: "desired_condition",
        matterKey: `semantic:${randomUUID()}`,
        statement: "Understand this actual material",
        scope: { description: "actual selected materials" },
        basis: [
          {
            basis: {
              case: "evidence",
              value: {
                occurrenceId: sourceId,
                locator: { case: "wholeOccurrence", value: true },
                basisRole: "direct",
              },
            },
          },
        ],
        condition: {
          expression: {
            expression: {
              case: "semantic",
              value: {
                version: 1,
                question:
                  "Does this actual material establish a bounded study direction?",
                materialRefs: [{ kind: "occurrence", value: sourceId }],
              },
            },
          },
          temporalForm: "achieve",
          window: {
            timeDomainRef: "time:real",
            extent: {
              value: {
                case: "interval",
                value: {
                  start: timestampFromDate(new Date(Date.now() - 60000)),
                  end: timestampFromDate(new Date(Date.now() + 60000)),
                },
              },
            },
          },
          coverageRequirement: "Only selected source coverage",
        },
      },
    });
    const request = create(EvaluateConditionRequestSchema, {
      subjectId,
      operationId: randomUUID(),
      conditionRevision: condition.reference,
      sourceRead: {
        subjectId,
        sourceRefs: [{ kind: "occurrence", value: sourceId }],
      },
      evidenceRefs: [{ kind: "occurrence", value: sourceId }],
    });
    const first = new KernelClient(transport());
    first.motivationWorkflow.apply = async () => {
      throw new Error("Fixture host stopped after paid result save");
    };
    await assert.rejects(
      evaluateMotivation(first, models, request, {}),
      /stopped after paid/,
    );
    assert.equal(calls, 1);
    await writeFile(
      statePath,
      JSON.stringify(toJson(EvaluateConditionRequestSchema, request)),
    );
    // A wrong catalog reference remains a saved rejected paid proposal on retry.
    invalid = true;
    const bad = { ...request, operationId: randomUUID() };
    const real = new KernelClient(transport());
    await assert.rejects(
      evaluateMotivation(real, models, bad, {}),
      /source key/,
    );
    assert.equal(calls, 2);
    await assert.rejects(
      evaluateMotivation(
        new KernelClient(transport()),
        new ModelRuntime(),
        bad,
        {},
      ),
      /source key/,
    );
    assert.equal(calls, 2);
    const reflectionSubject = await client.subjects.createSubject({
      operationId: randomUUID(),
      capabilities: { memory: true, motivation: true },
    });
    const rs = reflectionSubject.subjectId;
    for (const [path, value] of [
      ["reflection.enabled_domains", ["motivation"]],
      ["reflection.debounce_seconds", 1],
    ] as const)
      await client.configuration.setSubjectOverride({
        subjectId: rs,
        operationId: randomUUID(),
        path,
        value: fromJson(ValueSchema, value as unknown as JsonValue),
      });
    const session = await client.runtime.openSession({ subjectId: rs });
    await client.runtime.recordObservation({
      subjectId: rs,
      sessionId: session.sessionId,
      requestId: randomUUID(),
      sourceClass: "message",
      actorEntityRef: "entity:fixture:interested-person",
      conversationRef: "conversation:actual-interest",
      context: {
        cognition_usage: encodeCognitionUsage({
          agentInstanceId: "reflection:actual",
          purpose: "deliberation",
          intent: "Actual bounded learning interest",
          audience: {
            conversationRef: "conversation:actual-interest",
            visibility: "private",
          },
          sourceRead: { subjectId: rs, sourceRefs: [] },
        }),
      },
      material: {
        case: "inlineText",
        value: {
          text: "I am interested in learning how this bounded topic works",
          mediaType: "text/plain",
        },
      },
      admit: false,
    });
    await new Promise((resolve)=>setTimeout(resolve,1100));
    const grant = create(MaintenanceGrantRequestSchema, {
      subjectId: rs,
      maxOperations: 32,
      maxModelCalls: 1,
      maxElapsedMs: 30000,
    });
    const reflected = await grantMaintenance(
      new KernelClient(transport()),
      models,
      grant,
    );
    assert.equal(reflected.modelCalls, 1, JSON.stringify(reflected));
    assert.equal(calls, 3);
    const wishes = await client.motivation.list({
      subjectId: rs,
      page: { pageSize: 100 },
    });
    assert.equal(wishes.items.length, 1);
    assert.equal(wishes.items[0]?.content?.family, "desire");
    assert.equal(
      wishes.items[0]?.content?.producer?.operation,
      "subject_reflection_text",
    );
    await grantMaintenance(new KernelClient(transport()), models, grant);
    assert.equal(calls, 3);
    console.log(
      "Actual provider schema, durable-before-apply, invalid catalog result retention PASS",
    );
  } finally {
    provider.close();
    await once(provider, "close");
    delete process.env.NOUS_CRITERION_FIXTURE;
  }
}
