// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import type { NousClient } from "@nous-wave/client";
import type { SelfRecord } from "@nous-wave/protocol/nous/wave/v1alpha1/self_pb.js";

export async function selfTraceSmoke(
  client: NousClient,
  subjectId: string,
  modelRecord: SelfRecord,
) {
  const modelTrace = await client.cognition.trace({
    subjectId,
    reference: modelRecord.reference,
    maxNodes: 64,
    maxBytes: 65536,
  });
  assert(modelTrace.nodes.some((node) => node.content.case === "selfRecord"));
  assert(modelTrace.nodes.some((node) => node.content.case === "seed"));
  assert(
    modelTrace.producers.some(
      (producer) =>
        producer.operation === "self_formation_text" &&
        producer.modelIdentity === "self-contract" &&
        producer.signatureHash.length === 64,
    ),
  );
  await client.subjects.provisionCapabilities({
    subjectId,
    operationId: crypto.randomUUID(),
    capabilities: { memory: true },
  });
  const observation = await client.runtime.recordObservation({
    subjectId,
    requestId: crypto.randomUUID(),
    sourceClass: "generated",
    material: {
      case: "inlineText",
      value: {
        text: "这一次我选择先核对原始依据；该偏好限于事实核验。",
        mediaType: "text/plain",
      },
    },
    admit: false,
  });
  const evidence = {
    basis: {
      case: "evidence" as const,
      value: {
        occurrenceId: observation.occurrenceId,
        locator: { case: "wholeOccurrence" as const, value: true },
        basisRole: "direct",
      },
    },
  };
  const memory = await client.memory.formMemory({
    subjectId,
    operationId: crypto.randomUUID(),
    cognitiveRole: "declarative",
    input: {
      text: "这一次我选择先核对原始依据；该偏好限于事实核验。",
      semanticRole: "self_choice",
      epistemicClass: "observed",
      formationMode: "grounded",
      groundingOccurrenceId: observation.occurrenceId,
      basis: [evidence],
      validTime: { timeDomainRef: "time:real", extent: {} },
    },
  });
  const preference = await client.self.adopt({
    subjectId,
    operationId: crypto.randomUUID(),
    reason: "Actual bounded choice from source",
    content: {
      content: {
        case: "facet",
        value: {
          kind: "preference",
          statement: memory.text,
          epistemicClass: "observed",
          stance: "tentative",
          scope: { description: "事实核验" },
          validTime: { timeDomainRef: "time:real", extent: {} },
          basis: [
            {
              basis: {
                case: "cognitionDependency",
                value: {
                  targetRevision: {
                    kind: "memory_revision",
                    value: memory.revisionId,
                  },
                  basisRole: "direct",
                },
              },
            },
          ],
        },
      },
    },
  });
  const narrative = await client.self.adopt({
    subjectId,
    operationId: crypto.randomUUID(),
    reason:
      "Organize exact identity and scoped choice without rewriting either",
    content: {
      content: {
        case: "narrative",
        value: {
          theme: "身份与核验选择",
          interpretation: "当前采用的身份与事实核验中的一次选择。",
          epistemicClass: "derived",
          sourceRefs: [modelRecord.reference!, preference.reference!],
          basis: [evidence],
          validTime: { timeDomainRef: "time:real", extent: {} },
        },
      },
    },
  });
  const traced = await client.cognition.trace({
    subjectId,
    reference: narrative.reference,
    maxNodes: 64,
    maxBytes: 65536,
  });
  const ids = traced.nodes.map((node) => node.reference!.value);
  for (const id of [
    narrative.reference!.value,
    preference.reference!.value,
    modelRecord.reference!.value,
    memory.revisionId,
  ])
    assert(ids.includes(id));
  assert.equal(ids.length, new Set(ids).size);
  assert(
    traced.lineage?.nodes.some((node) => node.reference?.kind === "occurrence"),
  );
  assert(
    traced.lineage?.nodes.some((node) => node.reference?.kind === "artifact"),
  );
  assert(!traced.truncated);
  const tiny = await client.cognition.trace({
    subjectId,
    reference: narrative.reference,
    maxNodes: 1,
    maxBytes: 65536,
  });
  assert.equal(tiny.nodes.length, 1);
  assert(tiny.truncated);
  await assert.rejects(
    client.cognition.trace({
      subjectId,
      reference: narrative.reference,
      maxNodes: 64,
      maxBytes: 1,
    }),
  );
  // A separately initialized individual exercises exact old Seed reads after a new adoption.
  const seeded = await client.subjects.createSubject({
    operationId: crypto.randomUUID(),
    capabilities: { selfCognition: true },
    cognitiveSeed: {
      format: "text/markdown",
      text: "# 原始来源\n初始设定名：露白\n虚构背景，不是实际经历。\n",
    },
  });
  const original = await client.subjects.getCognitiveSeed({
    subjectId: seeded.subjectId,
  });
  const identity = await client.self.adopt({
    subjectId: seeded.subjectId,
    operationId: crypto.randomUUID(),
    reason: "Adopt the preserved initial designation as narrative",
    content: {
      content: {
        case: "facet",
        value: {
          kind: "identity",
          statement: "初始设定自称露白。",
          epistemicClass: "narrative",
          stance: "adopted",
          scope: { description: "general" },
          validTime: { timeDomainRef: "time:real", extent: {} },
          basis: [
            {
              basis: {
                case: "seed",
                value: {
                  seedVersion: {
                    kind: "cognitive_seed_version",
                    value: original.seedVersionId,
                  },
                  semanticPath: "line:2",
                },
              },
            },
          ],
        },
      },
    },
  });
  const newer = await client.subjects.adoptCognitiveSeed({
    subjectId: seeded.subjectId,
    operationId: crypto.randomUUID(),
    kind: "import",
    seed: {
      format: "text/markdown",
      text: "# 新增来源\n另一个采用文档，保留初始来源。\n",
    },
  });
  assert.notEqual(newer.seedVersionId, original.seedVersionId);
  const oldSource = await client.subjects.getCognitiveSeedVersion({
    subjectId: seeded.subjectId,
    id: original.seedVersionId,
  });
  assert.equal(oldSource.seed?.text, original.seed?.text);
  assert.equal(
    oldSource.adoptionId,
    "",
    "Exact source read must not relabel it with a newer adoption",
  );
  const exact = await client.cognition.trace({
    subjectId: seeded.subjectId,
    reference: identity.reference,
    maxNodes: 64,
    maxBytes: 65536,
  });
  const source = exact.nodes.find((node) => node.content.case === "seed")!;
  assert.equal(source.reference?.value, original.seedVersionId);
  assert(
    source.content.case === "seed" &&
      source.content.value.seed?.text === original.seed?.text,
  );
  await assert.rejects(
    client.subjects.getCognitiveSeedVersion({
      subjectId,
      id: original.seedVersionId,
    }),
  );
  await assert.rejects(
    client.cognition.trace({ subjectId, reference: identity.reference }),
  );
  const purged = await client.self.get({
    subjectId: seeded.subjectId,
    id: identity.objectRef!.value,
  });
  await client.self.purge({
    subjectId: seeded.subjectId,
    operationId: crypto.randomUUID(),
    objectRef: purged.objectRef,
    expectedObjectEpoch: purged.objectEpoch,
  });
  await assert.rejects(
    client.cognition.trace({
      subjectId: seeded.subjectId,
      reference: identity.reference,
    }),
  );
  console.log(
    "SELF_TRACE_SMOKE mixed_owners=true canonical_producer=true exact_seed=true original_text=true scope=true budget=true purge=true",
  );
}
