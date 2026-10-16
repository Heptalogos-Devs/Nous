// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { appendFile } from "node:fs/promises";
import { join } from "node:path";
import { create } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import {
  CognitionUsageSchema,
  EvidenceRefSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { createManagedBinding, eventIdentity } from "@nous-wave/client/managed";
import { prepare, boot, stop, type Boot } from "./support.js";
import { workspaceTemp } from "../workspace.js";
const home = await workspaceTemp("smoke", "current-materials-");
let core: Boot | undefined;
try {
  const locator = await prepare(home);
  await appendFile(
    join(home, "config/nous.toml"),
    '\n[[consumers]]\nconsumer_id="consumer:current-media"\nmemory="OPTIONAL"\nself_cognition="OPTIONAL"\nsocial_cognition="OPTIONAL"\nruntime="OPTIONAL"\nresource="FORBIDDEN"\nmax_items=32\nmax_text_bytes=65536\nmaterialize=true\n',
  );
  core = await boot(locator);
  const c = core.client;
  const { subjectId } = await c.subjects.createSubject({
    operationId: crypto.randomUUID(),
  });
  const observe = (conversationRef: string, text: string) =>
    c.runtime.recordObservation({
      subjectId,
      requestId: crypto.randomUUID(),
      sourceClass: "message",
      conversationRef,
      material: {
        case: "inlineText",
        value: { text, mediaType: "text/plain" },
      },
      admit: false,
    });
  const pub = await observe(
    "conversation:synthetic-public",
    "Current public media description marker",
  );
  const priv = await observe(
    "conversation:synthetic-private",
    "Private media must not appear",
  );
  // A second private attribution of identical artifact bytes must not poison
  // the actual public Occurrence's independently verifiable path.
  await observe(
    "conversation:synthetic-private",
    "Current public media description marker",
  );
  const derived = await c.model.deriveMaterial({
    subjectId,
    sourceRegionId: pub.sourceRegionId,
    strategy: "description_only",
  });
  const privateDerived = await c.model.deriveMaterial({
    subjectId,
    sourceRegionId: priv.sourceRegionId,
    strategy: "description_only",
  });
  assert(derived.selectedRepresentationId);
  assert(privateDerived.selectedRepresentationId);
  const source = create(EvidenceRefSchema, {
    occurrenceId: pub.occurrenceId,
    locator: {
      case: "derivedRepresentationId" as const,
      value: derived.selectedRepresentationId,
    },
    basisRole: "interpretation",
  });
  const privateSource = create(EvidenceRefSchema, {
    occurrenceId: priv.occurrenceId,
    locator: {
      case: "derivedRepresentationId" as const,
      value: privateDerived.selectedRepresentationId,
    },
    basisRole: "interpretation",
  });
  const session = await c.runtime.openSession({ subjectId });
  const usage = create(CognitionUsageSchema, {
    agentInstanceId: "native:synthetic-current-media",
    purpose: "subject_deliberation",
    intent: "Read current admitted media",
    audience: {
      conversationRef: "conversation:synthetic-public",
      visibility: "public",
    },
    sourceRead: {
      subjectId,
      originConversationRefs: ["conversation:synthetic-public"],
    },
  });
  const read = await c.cognition.readSources({
    subjectId,
    usage,
    materialSources: [source, privateSource],
    maxTextBytes: 65536,
  });
  assert.equal(read.items.length, 1);
  assert(
    read.items[0]!.text.includes("Current public media description marker"),
  );
  assert(!JSON.stringify(read).includes("Private media must not appear"));
  const binding = createManagedBinding({
    client: () => c,
    subjectId,
    sessionId: session.sessionId,
    consumerId: "consumer:current-media",
  });
  const prepared = await binding.prepare({
    usage,
    currentMaterials: [source],
    cognitionTokenBudget: 16384,
    signal: new AbortController().signal,
  });
  const applied = binding.apply({ text: "native base" }, prepared, {
    render: (base, slice) => ({
      ...base,
      text: base.text + JSON.stringify(slice),
    }),
  });
  assert(
    JSON.stringify(applied).includes("Current public media description marker"),
  );
  const admittedAt = new Date();
  await binding.admit({
    invocationId: "native-current-media:1",
    applied: applied.applied,
    occurredAt: admittedAt,
  });
  const exactRefs = [
    { kind: "derived_representation", value: derived.selectedRepresentationId },
    { kind: "occurrence", value: pub.occurrenceId },
  ];
  const replay = await c.runtime.reportUse({
    subjectId,
    sessionId: session.sessionId,
    consumerRef: "consumer:current-media",
    modelInput: true,
    events: await Promise.all(
      exactRefs.map(async (reference) => ({
        eventId: await eventIdentity(
          `native-current-media:1:presented:${reference.kind}:${reference.value}`,
        ),
        reference,
        kind: "presented",
        occurredAt: timestampFromDate(admittedAt),
        context: {
          invocation_id: "native-current-media:1",
          projection_id: applied.applied.projectionId,
          stage: "model_input",
          ...(reference.kind === "derived_representation"
            ? { actual_occurrence_id: pub.occurrenceId }
            : {}),
        },
      })),
    ),
  });
  assert.equal(replay.acceptedCount, 0);
  assert.equal(
    replay.duplicateCount,
    2,
    "both exact Material and actual Occurrence must be in the real use ledger",
  );
  await assert.rejects(
    () =>
      c.runtime.reportUse({
        subjectId,
        sessionId: session.sessionId,
        consumerRef: "consumer:current-media",
        modelInput: true,
        events: [
          {
            eventId: crypto.randomUUID(),
            reference: exactRefs[0],
            kind: "presented",
            occurredAt: timestampFromDate(new Date()),
            context: { actual_occurrence_id: priv.occurrenceId },
          },
        ],
      }),
    /actual Occurrence|unavailable/i,
  );

  await assert.rejects(
    () =>
      binding.prepare({
        usage,
        currentMaterials: [privateSource],
        cognitionTokenBudget: 16384,
        signal: new AbortController().signal,
      }),
    /source|permission|available|scope/i,
  );
  console.log(
    JSON.stringify({
      home,
      currentMaterial: "PASS",
      publicOccurrencePath: "PASS",
      privateDenied: "PASS",
      presentedBeforeInput: "PASS",
      modelCalls: 0,
    }),
  );
} finally {
  await stop(core);
}
