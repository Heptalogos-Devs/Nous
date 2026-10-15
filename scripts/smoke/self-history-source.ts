// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import type { NousClient } from "@nous-wave/client";

/** Privacy must follow old immutable derivations, not just current heads. */
export async function selfHistorySourceSmoke(
  client: NousClient,
  subjectId: string,
) {
  await client.subjects.provisionCapabilities({
    subjectId,
    operationId: crypto.randomUUID(),
    capabilities: { memory: true },
  });
  const makeMemory = async (text: string) => {
    const source = await client.runtime.recordObservation({
      subjectId,
      requestId: crypto.randomUUID(),
      sourceClass: "generated",
      material: {
        case: "inlineText",
        value: { text, mediaType: "text/plain" },
      },
      admit: false,
    });
    return client.memory.formMemory({
      subjectId,
      operationId: crypto.randomUUID(),
      cognitiveRole: "declarative",
      input: {
        formationMode: "grounded",
        groundingOccurrenceId: source.occurrenceId,
        semanticRole: "self_choice_evidence",
        epistemicClass: "observed",
        text,
        validTime: {},
        basis: [
          {
            basis: {
              case: "evidence",
              value: {
                occurrenceId: source.occurrenceId,
                locator: { case: "wholeOccurrence", value: true },
                basisRole: "direct",
              },
            },
          },
        ],
      },
    });
  };
  const first = await makeMemory("我在这一次核验中选择先检查原始来源。");
  const second = await makeMemory(
    "我在后来的一次核验中选择先比较可获得的来源质量。",
  );
  const facetContent = (revisionId: string, statement: string) => ({
    content: {
      case: "facet" as const,
      value: {
        kind: "preference",
        statement,
        stance: "tentative",
        epistemicClass: "observed",
        scope: { description: "this verification task" },
        validTime: {},
        basis: [
          {
            basis: {
              case: "cognitionDependency" as const,
              value: {
                targetRevision: { kind: "memory_revision", value: revisionId },
                basisRole: "direct",
              },
            },
          },
        ],
      },
    },
  });
  const facet = await client.self.adopt({
    subjectId,
    operationId: crypto.randomUUID(),
    reason: "A source-backed contextual choice",
    content: facetContent(first.revisionId, first.text),
  });
  const narrativeContent = (
    reference: NonNullable<typeof facet.reference>,
    interpretation: string,
  ) => ({
    content: {
      case: "narrative" as const,
      value: {
        theme: "核验时怎样作选择",
        interpretation,
        epistemicClass: "derived",
        sourceRefs: [reference],
        basis: [
          {
            basis: {
              case: "cognitionDependency" as const,
              value: { targetRevision: reference, basisRole: "direct" },
            },
          },
        ],
        validTime: {},
      },
    },
  });
  const narrative = await client.self.adopt({
    subjectId,
    operationId: crypto.randomUUID(),
    reason: "Interpret the actual scoped choice",
    content: narrativeContent(facet.reference!, first.text),
  });
  const session = await client.runtime.openSession({ subjectId });
  await client.runtime.reportUse({
    subjectId,
    sessionId: session.sessionId,
    consumerRef: "default",
    modelInput: true,
    events: [
      {
        eventId: crypto.randomUUID(),
        reference: narrative.reference,
        kind: "presented",
        occurredAt: {
          seconds: BigInt(Math.floor(Date.now() / 1000)),
          nanos: 0,
        },
        context: { invocation_id: "old-narrative-history-contract" },
      },
    ],
  });
  const revised = await client.self.revise({
    subjectId,
    operationId: crypto.randomUUID(),
    targetRevision: facet.reference,
    expectedObjectEpoch: facet.objectEpoch,
    reason: "Actual later evidence changes the same scoped choice",
    content: facetContent(second.revisionId, second.text),
  });
  const invalid = await client.self.get({
    subjectId,
    id: narrative.objectRef!.value,
  });
  const current = await client.self.revise({
    subjectId,
    operationId: crypto.randomUUID(),
    targetRevision: narrative.reference,
    expectedObjectEpoch: invalid.objectEpoch,
    reason: "Reinterpret the same theme against the later exact source",
    content: narrativeContent(revised.reference!, second.text),
  });
  assert(
    !(await client.runtime.getSession({ subjectId, id: session.sessionId }))
      .historyInvalidated,
    "Ordinary revisions alone must not invalidate host history",
  );
  await client.memory.purgeMemory({
    subjectId,
    operationId: crypto.randomUUID(),
    memoryId: first.memoryId,
    expectedObjectEpoch: first.objectEpoch,
  });
  assert(
    (await client.runtime.getSession({ subjectId, id: session.sessionId }))
      .historyInvalidated,
    "Purging the old Memory must fence a history that used its old transitive narrative revision",
  );
  assert.equal(
    (await client.self.get({ subjectId, id: current.objectRef!.value }))
      .integrityState,
    "valid",
    "The independently reinterpreted current narrative retains its valid new source",
  );
}
