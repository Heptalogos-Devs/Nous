// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create } from "@bufbuild/protobuf";
import { CognitionUsageSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { selfHistorySourceSmoke } from "./self-history-source.js";
import assert from "node:assert/strict";
import { appendFile } from "node:fs/promises";
import { join } from "node:path";
import { createManagedBinding } from "@nous-wave/client/managed";
import { prepare, boot, stop, type Boot } from "./support.js";
import { workspaceTemp } from "../workspace.js";

// Public owner/SDK contract. Admission is exercised without claiming a provider executed.
const home = await workspaceTemp("smoke", "self-history-");
let core: Boot | undefined;
try {
  const locator = await prepare(home);
  await appendFile(
    join(home, "config/nous.toml"),
    '\n[[consumers]]\nconsumer_id="default"\n[[consumers]]\nconsumer_id="consumer:no-self"\nmemory="FORBIDDEN"\nself_cognition="FORBIDDEN"\nruntime="OPTIONAL"\nresource="FORBIDDEN"\n',
  );
  core = await boot(locator);
  const subject = await core.client.subjects.createSubject({
    operationId: crypto.randomUUID(),
    capabilities: { selfCognition: true },
    cognitiveSeed: {
      format: "text/markdown",
      text: "# Choice\nPRIVATE_SELF_HISTORY_MARKER_青砚\n",
    },
  });
  const subjectId = subject.subjectId;
  const seed = await core.client.subjects.getCognitiveSeed({ subjectId });
  const adopt = (statement: string) =>
    core!.client.self.adopt({
      subjectId,
      operationId: crypto.randomUUID(),
      reason: "Explicit sourced Self for history contract",
      content: {
        content: {
          case: "facet",
          value: {
            kind: "identity",
            statement,
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
                      value: seed.seedVersionId,
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
  const facet = await adopt("PRIVATE_SELF_HISTORY_MARKER_青砚");
  const session = await core.client.runtime.openSession({ subjectId });
  const binding = (sessionId: string, consumerId = "default") =>
    createManagedBinding({
      subjectId,
      sessionId,
      consumerId,
      client: () => core!.client,
    });
  const prepareInput = async (managed: ReturnType<typeof binding>) => {
    const prepared = await managed.prepare({
      cognitionTokenBudget: 4096,
      signal: new AbortController().signal,
      usage: create(CognitionUsageSchema, {
        agentInstanceId: "history-contract",
        sourceRead: {
          subjectId,
          sourceRefs: [
            { kind: "cognitive_seed_version", value: seed.seedVersionId },
          ],
        },
        purpose: "subject_deliberation",
        intent: "Continue actual task",
        audience: {
          conversationRef: "history-contract",
          visibility: "internal",
        },
      }),
    });
    return managed.apply("Clean native base", prepared, {
      render: (base, segments) =>
        base + segments.map((segment) => segment.text).join("\n"),
    });
  };
  const managed = binding(session.sessionId);
  const first = await prepareInput(managed);
  assert(first.input.includes("PRIVATE_SELF_HISTORY_MARKER"));
  const input = {
    invocationId: crypto.randomUUID(),
    applied: first.applied,
    occurredAt: new Date(),
  };
  await managed.admit(input);
  // The host cancels before network transmission; the recorded outcome is exactly that.
  await managed.report({ ...input, outcome: "cancelled" });
  const used = await core.client.runtime.getSession({
    subjectId,
    id: session.sessionId,
  });
  assert.deepEqual(used.historyDomains, ["self"]);
  const child = await core.client.runtime.openSession({
    subjectId,
    parentSessionId: session.sessionId,
  });
  assert.equal(child.parentSessionId, session.sessionId);
  const unrelated = await core.client.runtime.openSession({ subjectId });
  const pending = binding(unrelated.sessionId);
  const staged = await prepareInput(pending);
  await core.client.self.purge({
    subjectId,
    operationId: crypto.randomUUID(),
    objectRef: facet.objectRef,
    expectedObjectEpoch: facet.objectEpoch,
  });
  assert(
    (await core.client.runtime.getSession({ subjectId, id: session.sessionId }))
      .historyInvalidated,
  );
  assert(
    (await core.client.runtime.getSession({ subjectId, id: child.sessionId }))
      .historyInvalidated,
  );
  assert(
    !(
      await core.client.runtime.getSession({
        subjectId,
        id: unrelated.sessionId,
      })
    ).historyInvalidated,
  );
  await assert.rejects(prepareInput(managed), /reset/);
  await assert.rejects(prepareInput(binding(child.sessionId)), /reset/);
  await assert.rejects(
    core.client.runtime.openSession({
      subjectId,
      parentSessionId: session.sessionId,
    }),
    /reset/,
  );
  // A source purged between preparation and admission cannot reach the provider,
  // including a Session with no earlier model use to invalidate.
  await assert.rejects(
    pending.admit({
      invocationId: crypto.randomUUID(),
      applied: staged.applied,
      occurredAt: new Date(),
    }),
  );
  await stop(core);
  core = await boot(locator);
  await assert.rejects(prepareInput(binding(session.sessionId)), /reset/);
  await assert.rejects(
    core.client.runtime.reportUse({
      subjectId,
      sessionId: child.sessionId,
      consumerRef: "default",
      modelInput: true,
      events: [],
    }),
    /reset/,
  );
  const fresh = await core.client.runtime.openSession({ subjectId });
  const clean = binding(fresh.sessionId);
  const cleanInput = await prepareInput(clean);
  assert(!cleanInput.input.includes("PRIVATE_SELF_HISTORY_MARKER"));
  assert.equal(cleanInput.applied.sourceRefs.length, 0);
  await clean.admit({
    invocationId: crypto.randomUUID(),
    applied: cleanInput.applied,
    occurredAt: new Date(),
  });
  const restoredFacet = await adopt("当前自称青岚");
  const beforeSuppress = await prepareInput(clean);
  await clean.admit({
    invocationId: crypto.randomUUID(),
    applied: beforeSuppress.applied,
    occurredAt: new Date(),
  });
  const suppressed = await core.client.self.setLifecycle({
    mutation: {
      subjectId,
      operationId: crypto.randomUUID(),
      objectRef: restoredFacet.objectRef,
      expectedObjectEpoch: restoredFacet.objectEpoch,
    },
    action: "suppress",
  });
  await assert.rejects(prepareInput(clean), /reset/);
  await core.client.self.setLifecycle({
    mutation: {
      subjectId,
      operationId: crypto.randomUUID(),
      objectRef: restoredFacet.objectRef,
      expectedObjectEpoch: suppressed.objectEpoch,
    },
    action: "restore",
  });
  await assert.rejects(prepareInput(clean), /reset/);
  const narrowerSession = await core.client.runtime.openSession({ subjectId });
  const broad = binding(narrowerSession.sessionId);
  const broadInput = await prepareInput(broad);
  await broad.admit({
    invocationId: crypto.randomUUID(),
    applied: broadInput.applied,
    occurredAt: new Date(),
  });
  assert(
    !(
      await core.client.runtime.getSession({
        subjectId,
        id: narrowerSession.sessionId,
      })
    ).historyInvalidated,
  );
  await assert.rejects(
    prepareInput(binding(narrowerSession.sessionId, "consumer:no-self")),
    /reset/,
  );
  const emptyNarrow = await core.client.runtime.openSession({ subjectId });
  const narrow = binding(emptyNarrow.sessionId, "consumer:no-self");
  const narrowInput = await prepareInput(narrow);
  assert(!narrowInput.input.includes("当前自称青岚"));
  await narrow.admit({
    invocationId: crypto.randomUUID(),
    applied: narrowInput.applied,
    occurredAt: new Date(),
  });
  const otherSubject = await core.client.subjects.createSubject({
    operationId: crypto.randomUUID(),
    capabilities: { selfCognition: true },
  });
  await assert.rejects(
    core.client.runtime.openSession({
      subjectId: otherSubject.subjectId,
      parentSessionId: narrowerSession.sessionId,
    }),
  );
  const relayRoot = await core.client.runtime.openSession({ subjectId });
  const relayBinding = binding(relayRoot.sessionId);
  const rootInput = await prepareInput(relayBinding);
  await relayBinding.admit({
    invocationId: crypto.randomUUID(),
    applied: rootInput.applied,
    occurredAt: new Date(),
  });
  const relayChild = await core.client.runtime.openSession({
    subjectId,
    parentSessionId: relayRoot.sessionId,
  });
  const relaySibling = await core.client.runtime.openSession({
    subjectId,
    parentSessionId: relayRoot.sessionId,
  });
  const workerChoice = await core.client.self.adopt({
    subjectId,
    operationId: crypto.randomUUID(),
    reason:
      "A scoped variant of the preserved Seed designation for a relay contract",
    content: {
      content: {
        case: "facet",
        value: {
          kind: "identity",
          statement: "Worker designation: PRIVATE_SELF_HISTORY_MARKER_青砚",
          stance: "adopted",
          epistemicClass: "narrative",
          scope: {
            description: "worker task only",
            purposes: ["professional_execution"],
          },
          validTime: { timeDomainRef: "time:real", extent: {} },
          basis: [
            {
              basis: {
                case: "seed",
                value: {
                  seedVersion: {
                    kind: "cognitive_seed_version",
                    value: seed.seedVersionId,
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
  await core.client.runtime.reportUse({
    subjectId,
    sessionId: relayChild.sessionId,
    consumerRef: "default",
    modelInput: true,
    events: [
      {
        eventId: crypto.randomUUID(),
        reference: workerChoice.reference,
        kind: "presented",
        occurredAt: {
          seconds: BigInt(Math.floor(Date.now() / 1000)),
          nanos: 0,
        },
        context: { invocation_id: "relay-contract" },
      },
    ],
  });
  await core.client.self.purge({
    subjectId,
    operationId: crypto.randomUUID(),
    objectRef: workerChoice.objectRef,
    expectedObjectEpoch: workerChoice.objectEpoch,
  });
  for (const affected of [relayRoot, relayChild, relaySibling])
    assert(
      (
        await core.client.runtime.getSession({
          subjectId,
          id: affected.sessionId,
        })
      ).historyInvalidated,
      "Parent and sibling histories can contain relayed worker results",
    );
  await selfHistorySourceSmoke(core.client, subjectId);
  console.log(
    "SELF_HISTORY_SMOKE source_admission=true descendant_fence=true restart=true empty_input_guard=true scope_restriction=true native_reset_required=true unrelated_session=true",
  );
} finally {
  await stop(core);
}
