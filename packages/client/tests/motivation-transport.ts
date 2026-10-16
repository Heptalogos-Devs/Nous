import { DomainTemporalExtentSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { criterionFactContext } from "../src/motivation.js";
import { create, type MessageInitShape } from "@bufbuild/protobuf";
import {
  MotivationContentSchema,
  CriterionExpressionSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/motivation_pb.js";
import { KernelClient } from "../../../apps/nous-core/src/kernel-client.js";
import { createCore } from "../../../apps/nous-core/src/server.js";
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import {
  createConnectTransport,
  createGrpcTransport,
} from "@connectrpc/connect-node";
import { createNousClient } from "../src/index.js";
import { mcpConnection } from "../../../apps/nous-cli/src/mcp-scope.js";
import type { connectNousInstance } from "../src/node.js";
const [endpoint, subjectId, conditionId, sourceId, eventId] =
  process.argv.slice(2) as [string, string, string, string, string];
const connect = (token: string) =>
  createNousClient(
    createGrpcTransport({
      baseUrl: endpoint,
      interceptors: [
        (next) => async (request) => {
          request.header.set("authorization", `Bearer ${token}`);
          return next(request);
        },
      ],
    }),
  );
const kernel = new KernelClient(
  createGrpcTransport({
    baseUrl: endpoint,
    interceptors: [
      (next) => async (request) => {
        request.header.set("authorization", "Bearer motivation-fixture");
        return next(request);
      },
    ],
  }),
);
const token = "motivation-core-fixture-credential-32-bytes";
const core = await createCore({
  kernel,
  token,
  maxUploadBytes: 1024 * 1024,
  consumers: [
    {
      consumerId: "motivation-fixture",
      revision: "1",
      memory: "OPTIONAL",
      selfCognition: "FORBIDDEN",
      socialCognition: "OPTIONAL",
      motivation: "PREFERRED",
      runtime: "OPTIONAL",
      resource: "FORBIDDEN",
      maxItems: 256,
      maxTextBytes: 262144,
      materialize: true,
    },
  ],
});
await core.listen({ host: "127.0.0.1", port: 0 });
const address = core.server.address();
assert(address && typeof address !== "string");
const client = createNousClient(
  createConnectTransport({
    baseUrl: `http://127.0.0.1:${address.port}`,
    httpVersion: "1.1",
    interceptors: [
      (next) => async (request) => {
        request.header.set("authorization", `Bearer ${token}`);
        return next(request);
      },
    ],
  }),
);
try {
  await assert.rejects(
    connect("wrong").motivation.getRevision({ subjectId, id: conditionId }),
  );
  const original = await client.motivation.getRevision({
    subjectId,
    id: conditionId,
  });
  assert.equal(original.content?.family, "desired_condition");
  const input = {
    subjectId,
    operationId: randomUUID(),
    conditionRevision: original.reference,
    sourceRead: {
      subjectId,
      sourceRefs: [
        { kind: "occurrence", value: sourceId },
        { kind: "occurrence", value: eventId },
      ],
    },
    evidenceRefs: [{ kind: "occurrence", value: eventId }],
  };
  const result = await client.motivation.evaluate(input);
  assert.equal(result.verdict, "satisfied");
  assert.deepEqual(await client.motivation.evaluate(input), result);
  const native = mcpConnection(
    { ...client } as Awaited<ReturnType<typeof connectNousInstance>>,
    false,
  );
  assert.throws(
    () => native.motivation.getRevision({ subjectId, id: conditionId }),
    /does not grant/,
  );
  for (const name of [
    "adopt",
    "revise",
    "addEvidence",
    "setLifecycle",
    "purge",
    "evaluate",
  ] as const)
    assert.throws(
      () => (native.motivation[name] as (...args: unknown[]) => unknown)(input),
      /does not grant/,
    );
  assert.throws(
    () => native.runtime.recordObservation({ subjectId, sourceClass: "tool" }),
    /does not grant/,
  );
  const sourceRead = input.sourceRead;
  const usage = {
    agentInstanceId: "fixture:main",
    purpose: "deliberation",
    intent: "bounded study condition",
    audience: {
      conversationRef: "conversation:fixture",
      visibility: "internal",
    },
    sourceRead,
  };
  const scoped = mcpConnection(
    { ...client } as Awaited<ReturnType<typeof connectNousInstance>>,
    false,
    usage,
  );
  assert.throws(
    () => native.cognition.query({ subjectId, usage }),
    /trusted startup source scope/,
  );
  const query = await scoped.cognition.query({
    subjectId,
    usage,
    expression: {
      operation: "atom",
      cues: [
        { cue: { case: "text", value: "bounded study condition" } },
        { cue: { case: "reference", value: original.reference! } },
      ],
      modifiers: {
        projection: { domains: ["desired_condition"] },
        materialize: true,
      },
    },
  });
  assert(query.hits.some((h) => h.revision?.value === conditionId));
  const session = await client.runtime.openSession({ subjectId });
  const projection = await client.cognition.buildProjection({
    subjectId,
    sessionId: session.sessionId,
    consumerId: "motivation-fixture",
    usage,
    maxItems: 256,
    maxTextBytes: 262144,
    materialize: true,
  });
  assert(
    projection.segments.some(
      (segment) =>
        segment.domain === "motivation" &&
        segment.sourceRefs.some((r) => r.value === conditionId),
    ),
  );
  const trace = await client.cognition.trace({
    subjectId,
    reference: original.reference,
    maxNodes: 64,
    maxBytes: 262144,
  });
  assert(
    trace.nodes.some(
      (node) =>
        node.reference?.value === conditionId &&
        node.content.case === "motivationRecord",
    ),
  );
  const base = original.content!;
  const actualAt = new Date();
  const stamp = {
    timeDomainRef: "time:real",
    at: timestampFromDate(actualAt),
    recordedAt: timestampFromDate(new Date()),
  };
  const measured = await client.runtime.recordObservation({
    subjectId,
    requestId: randomUUID(),
    sourceClass: "tool",
    observedAt: timestampFromDate(new Date()),
    context: criterionFactContext({
      fact: {
        case: "measurement",
        value: {
          version: 1,
          name: "score",
          unit: "points",
          value: 8,
          error: 0,
          at: stamp,
        },
      },
    }),
    material: {
      case: "inlineText",
      value: {
        text: "Actual bounded score measurement",
        mediaType: "text/plain",
      },
    },
  });
  const measurement = {
    expression: {
      case: "measurement" as const,
      value: {
        version: 1,
        name: "score",
        unit: "points",
        comparison: "ge",
        value: 5,
      },
    },
  };
  async function evaluateLeaf(
    expression: MessageInitShape<typeof CriterionExpressionSchema>,
    evidenceIds: string[],
    windowEnd = new Date(Date.now() + 60000),
    form = "achieve",
  ) {
    const content = create(MotivationContentSchema, base);
    content.matterKey = `criterion:${randomUUID()}`;
    content.condition!.expression = create(
      CriterionExpressionSchema,
      expression,
    );
    content.condition!.temporalForm = form;
    content.condition!.window = create(DomainTemporalExtentSchema, {
      timeDomainRef: "time:real",
      extent: {
        value: {
          case: "interval",
          value: {
            start: timestampFromDate(new Date(Date.now() - 120000)),
            end: timestampFromDate(windowEnd),
          },
        },
      },
    });
    const adopted = await client.motivation.adopt({
      subjectId,
      operationId: randomUUID(),
      reason: "Actual criterion integration",
      content,
    });
    return client.motivation.evaluate({
      subjectId,
      operationId: randomUUID(),
      conditionRevision: adopted.reference,
      sourceRead: {
        subjectId,
        sourceRefs: [
          { kind: "occurrence", value: sourceId },
          ...evidenceIds.map((value) => ({ kind: "occurrence", value })),
        ],
      },
      evidenceRefs: evidenceIds.map((value) => ({ kind: "occurrence", value })),
    });
  }
  assert.equal(
    (await evaluateLeaf(measurement, [measured.occurrenceId])).verdict,
    "satisfied",
  );
  assert.equal(
    (
      await evaluateLeaf(
        {
          expression: {
            case: "measurement",
            value: { ...measurement.expression.value, unit: "seconds" },
          },
        },
        [measured.occurrenceId],
      )
    ).verdict,
    "unknown",
  );
  const missing = {
    expression: {
      case: "eventWitness" as const,
      value: { version: 1, eventType: "unobserved_event" },
    },
  };
  assert.equal(
    (
      await evaluateLeaf(
        {
          expression: { case: "all", value: { items: [measurement, missing] } },
        },
        [measured.occurrenceId],
      )
    ).verdict,
    "unknown",
  );
  assert.equal(
    (
      await evaluateLeaf(
        {
          expression: { case: "any", value: { items: [measurement, missing] } },
        },
        [measured.occurrenceId],
      )
    ).verdict,
    "satisfied",
  );
  assert.equal(
    (await evaluateLeaf({ expression: { case: "not", value: missing } }, []))
      .verdict,
    "unknown",
  );
  const uncertain = await client.runtime.recordObservation({
    subjectId,
    requestId: randomUUID(),
    sourceClass: "tool",
    context: criterionFactContext({
      fact: {
        case: "measurement",
        value: {
          version: 1,
          name: "score",
          unit: "points",
          value: 8,
          error: 4,
          at: stamp,
        },
      },
    }),
    material: {
      case: "inlineText",
      value: {
        text: "Measurement uncertainty interval crosses threshold",
        mediaType: "text/plain",
      },
    },
  });
  assert.equal(
    (
      await evaluateLeaf(
        {
          expression: {
            case: "measurement",
            value: { ...measurement.expression.value, value: 10 },
          },
        },
        [uncertain.occurrenceId],
      )
    ).verdict,
    "unknown",
  );
  const generated = await client.runtime.recordObservation({
    subjectId,
    requestId: randomUUID(),
    sourceClass: "generated",
    context: criterionFactContext({
      fact: {
        case: "event",
        value: { version: 1, eventType: "claimed_effect", at: stamp },
      },
    }),
    material: {
      case: "inlineText",
      value: { text: "I claim the task is complete", mediaType: "text/plain" },
    },
  });
  assert.equal(
    (
      await evaluateLeaf(
        {
          expression: {
            case: "eventWitness",
            value: { version: 1, eventType: "claimed_effect" },
          },
        },
        [generated.occurrenceId],
      )
    ).verdict,
    "unknown",
  );
  const boundary = new Date(Date.now() - 1000);
  const atBoundary = await client.runtime.recordObservation({
    subjectId,
    requestId: randomUUID(),
    sourceClass: "tool",
    context: criterionFactContext({
      fact: {
        case: "event",
        value: {
          version: 1,
          eventType: "boundary",
          at: { ...stamp, at: timestampFromDate(boundary) },
        },
      },
    }),
    material: {
      case: "inlineText",
      value: {
        text: "Actual event exactly on excluded deadline boundary",
        mediaType: "text/plain",
      },
    },
  });
  assert.equal(
    (
      await evaluateLeaf(
        {
          expression: {
            case: "eventWitness",
            value: { version: 1, eventType: "boundary" },
          },
        },
        [atBoundary.occurrenceId],
        boundary,
      )
    ).verdict,
    "unknown",
  );
  const otherWorld = await client.runtime.recordObservation({
    subjectId,
    requestId: randomUUID(),
    sourceClass: "host_event",
    context: criterionFactContext({
      fact: {
        case: "event",
        value: {
          version: 1,
          eventType: "world_event",
          at: { ...stamp, timeDomainRef: "time:other" },
        },
      },
    }),
    material: {
      case: "inlineText",
      value: { text: "A different world's event", mediaType: "text/plain" },
    },
  });
  assert.equal(
    (
      await evaluateLeaf(
        {
          expression: {
            case: "eventWitness",
            value: { version: 1, eventType: "world_event" },
          },
        },
        [otherWorld.occurrenceId],
      )
    ).verdict,
    "unknown",
  );
  await client.subjects.provisionCapabilities({
    subjectId,
    operationId: randomUUID(),
    capabilities: { socialCognition: true },
  });
  const people = await client.runtime.recordObservation({
    subjectId,
    requestId: randomUUID(),
    sourceClass: "message",
    actorEntityRef: "entity:fixture:alice",
    entities: [{ surface: "Bob", entityRef: "entity:fixture:bob" }],
    material: {
      case: "inlineText",
      value: {
        text: "Alice and Bob collaborate in learning",
        mediaType: "text/plain",
      },
    },
  });
  const relationship = await client.social.adopt({
    subjectId,
    operationId: randomUUID(),
    reason: "Actual actor observation",
    content: {
      content: {
        case: "relation",
        value: {
          from: "entity:fixture:alice",
          to: "entity:fixture:bob",
          relationType: "collaboration",
          degree: { degree: { case: "state", value: "active" } },
          scope: { description: "Learning together" },
          statement: "A bounded actual collaboration",
          validTime: {
            value: {
              case: "interval",
              value: { start: timestampFromDate(new Date(Date.now() - 60000)) },
            },
          },
          judgment: {
            state: "supported",
            explanation: "Actual attributed interaction",
          },
          basis: [
            {
              basis: {
                case: "evidence",
                value: {
                  occurrenceId: people.occurrenceId,
                  locator: { case: "wholeOccurrence", value: true },
                  basisRole: "direct",
                },
              },
            },
          ],
        },
      },
    },
  });
  assert.equal(
    (
      await evaluateLeaf(
        {
          expression: {
            case: "relation",
            value: {
              version: 1,
              reference: relationship.reference,
              relationType: "collaboration",
              degree: "active",
            },
          },
        },
        [people.occurrenceId],
      )
    ).verdict,
    "satisfied",
  );

  for (let index = 0; index < 105; index++)
    await client.motivation.adopt({
      subjectId,
      operationId: randomUUID(),
      reason: "Independent stable capacity matters",
      content: {
        ...base,
        family: "desire",
        condition: undefined,
        matterKey: `capacity:${index}`,
        statement: `actual distinct wish ${index}`,
      },
    });
  const page = await client.motivation.list({
    subjectId,
    page: { pageSize: 100 },
  });
  assert.equal(page.items.length, 100);
  assert(page.nextPageToken);
  const selected = await client.motivation.select({
    subjectId,
    scope: { description: "actual source scope" },
    sourceRead,
    maxItems: 256,
    maxBytes: 1048576,
  });
  assert(selected.items.length > 100);
  assert.equal(selected.omitted, false);
  console.log(
    "Motivation public Core Query/Projection/trace + authenticated SDK/MCP + capacity PASS",
  );
} finally {
  await core.close();
}
