// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { boot, stop, prepare, type Boot } from "./support.js";
import type { NousClient } from "@nous-wave/client";
import { create } from "@bufbuild/protobuf";
import { TimestampSchema } from "@bufbuild/protobuf/wkt";
import {
  CognitiveRefSchema,
  CognitiveSeedSchema,
  CreateSubjectRequestSchema,
  CueSchema,
  EvidenceRefSchema,
  FormMemoryRequestSchema,
  InlineTextSchema,
  MemoryContentSchema,
  ReviseMemoryRequestSchema,
  MemoryMutationRequestSchema,
  ObservationInputSchema,
  QueryExprSchema,
  QueryRequestSchema,
  ReportUseRequestSchema,
  RevisionBasisSchema,
  SubjectCapabilitiesSchema,
  SubjectRequestSchema,
  TemporalExtentSchema,
  UseEventSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { CreateTagRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { rm } from "node:fs/promises";
import { workspaceTemp } from "../workspace.js";

const subjectId = "00000000-0000-0000-0000-000000009801";
const occurrenceRequestId = "00000000-0000-0000-0000-000000009802";
const formOperationId = "00000000-0000-0000-0000-000000009803";
const useEventId = "00000000-0000-0000-0000-000000009804";
const consumerRef = "consumer:smoke:memory-reference";

const instant = create(TimestampSchema, {
  seconds: BigInt(Math.floor(Date.now() / 1000)),
  nanos: 0,
});

function queryExpression(text?: string) {
  return create(QueryExprSchema, {
    operation: "atom",
    cues: text
      ? [
          create(CueSchema, {
            cue: { case: "text", value: text },
          }),
        ]
      : [
          create(CueSchema, {
            cue: { case: "text", value: "Recall current cognition" },
          }),
        ],
  });
}

async function query(
  client: NousClient,
  sessionId: string | undefined,
  text?: string,
) {
  return client.cognition.query(
    create(QueryRequestSchema, {
      subjectId,
      sessionId,
      expression: queryExpression(text),
    }),
  );
}

function hitMatches(
  hit: { reference?: { value: string }; revision?: { value: string } },
  memoryId: string,
  revisionId: string,
) {
  return (
    hit.reference?.value === memoryId ||
    hit.reference?.value === revisionId ||
    hit.revision?.value === revisionId
  );
}

async function main() {
  const root = await workspaceTemp("smoke", "memory-");
  const configPath = await prepare(root);

  let booted: Boot | undefined;
  try {
    booted = await boot(configPath);
    const client = booted.client;
    const created = await client.subjects.createSubject(
      create(CreateSubjectRequestSchema, {
        subjectId,
        operationId: "00000000-0000-0000-0000-000000009801",
        cognitiveSeed: create(CognitiveSeedSchema, {
          text: "schema_version = 1",
          format: "application/vnd.nous-wave.cognitive-seed+toml;version=1",
        }),
        capabilities: create(SubjectCapabilitiesSchema, { memory: true }),
      }),
    );
    if (created.subjectId !== subjectId)
      throw new Error("Subject identity mismatch");

    const sessionA = await client.runtime.openSession(
      create(SubjectRequestSchema, { subjectId }),
    );
    const sessionB = await client.runtime.openSession(
      create(SubjectRequestSchema, { subjectId }),
    );
    const observed = await client.runtime.recordObservation(
      create(ObservationInputSchema, {
        subjectId,
        sessionId: sessionA.sessionId,
        requestId: occurrenceRequestId,
        sourceClass: "message",
        observedAt: instant,
        admit: true,
        material: {
          case: "inlineText",
          value: create(InlineTextSchema, {
            text: "Alice is present",
            mediaType: "text/plain",
          }),
        },
      }),
    );
    let formed = await client.memory.formMemory(
      create(FormMemoryRequestSchema, {
        operationId: formOperationId,
        subjectId,
        cognitiveRole: "declarative",
        input: create(MemoryContentSchema, {
          formationMode: "grounded",
          groundingOccurrenceId: observed.occurrenceId,
          semanticRole: "fact",
          text: "Alice is present",
          basis: [
            create(RevisionBasisSchema, {
              basis: {
                case: "evidence",
                value: create(EvidenceRefSchema, {
                  occurrenceId: observed.occurrenceId,
                  locator: { case: "wholeOccurrence", value: true },
                  basisRole: "direct",
                }),
              },
            }),
          ],
          validTime: create(TemporalExtentSchema, {}),

          epistemicClass: "observed",
        }),
      }),
    );
    if (!formed.memoryId || !formed.revisionId)
      throw new Error("Memory identity missing");
    if (
      !formed.basis.some(
        (basis) =>
          basis.basis.case === "evidence" &&
          basis.basis.value.occurrenceId === observed.occurrenceId,
      )
    )
      throw new Error("Memory provenance does not trace to Observation");

    const recalled = await query(client, undefined, "Alice is present");
    if (
      !recalled.hits.some((hit) =>
        hitMatches(hit, formed.memoryId, formed.revisionId),
      )
    )
      throw new Error("public Query did not recall the formed Memory");

    const useRequest = create(ReportUseRequestSchema, {
      subjectId,
      sessionId: sessionA.sessionId,
      consumerRef,
      events: [
        create(UseEventSchema, {
          eventId: useEventId,
          reference: create(CognitiveRefSchema, {
            kind: "memory_revision",
            value: formed.revisionId,
          }),
          kind: "referenced",
          occurredAt: instant,
        }),
      ],
    });
    const accepted = await client.runtime.reportUse(useRequest);
    const duplicate = await client.runtime.reportUse(useRequest);
    if (accepted.acceptedCount !== 1 || duplicate.duplicateCount !== 1)
      throw new Error("UseEvent retry was not idempotent");

    const residentA = await query(client, sessionA.sessionId);
    const residentB = await query(client, sessionB.sessionId);
    if (
      !residentA.hits.some((hit) =>
        hitMatches(hit, formed.memoryId, formed.revisionId),
      )
    )
      throw new Error("Session A did not expose its resident Memory");
    if (
      residentB.hits.some((hit) =>
        hitMatches(hit, formed.memoryId, formed.revisionId),
      )
    )
      throw new Error("Session B observed Session A residency");

    const original = formed;
    const tags = await Promise.all(
      ["attendance", "calibration"].map((label) =>
        client.concepts.createTag(
          create(CreateTagRequestSchema, {
            subjectId,
            operationId: randomUUID(),
            content: { label },
          }),
        ),
      ),
    );
    const revisionRequest = create(ReviseMemoryRequestSchema, {
      subjectId,
      memoryId: formed.memoryId,
      operationId: randomUUID(),
      expectedObjectEpoch: formed.objectEpoch,
      intent: "rephrase",
      input: {
        formationMode: formed.formationMode,
        groundingOccurrenceId: formed.groundingOccurrenceId,
        semanticRole: formed.semanticRole,
        text: "Alice is present at calibration",
        title: "Attendance",
        basis: formed.basis,
        aboutness: formed.aboutness,
        tags: [tags[1]!.tagId, tags[0]!.tagId, tags[1]!.tagId],
        validTime: formed.validTime,
        epistemicClass: formed.epistemicClass,
      },
    });
    formed = await client.memory.reviseMemory(revisionRequest);
    assert.deepEqual(
      [...formed.tags].sort(),
      tags.map((tag) => tag.tagId).sort(),
    );
    revisionRequest.input!.tags = tags.map((tag) => tag.tagId);
    assert.equal(
      (await client.memory.reviseMemory(revisionRequest)).revisionId,
      formed.revisionId,
    );
    revisionRequest.input!.tags = [tags[0]!.tagId];
    await assert.rejects(client.memory.reviseMemory(revisionRequest));
    const historyFirst = await client.memory.listMemoryRevisions({
      subjectId,
      memoryId: formed.memoryId,
      page: { pageSize: 1 },
    });
    assert.equal(historyFirst.items.length, 1);
    assert(historyFirst.nextPageToken);
    assert.equal(historyFirst.items[0]!.revisionId, original.revisionId);
    assert.deepEqual(historyFirst.items[0]!.basis, original.basis);
    assert.equal(historyFirst.items[0]!.cognitiveRole, original.cognitiveRole);
    assert.equal(historyFirst.items[0]!.objectEpoch, formed.objectEpoch);
    assert(
      historyFirst.items[0]!.temporalEvidence &&
        historyFirst.items[0]!.createdAt,
    );
    const historyLast = await client.memory.listMemoryRevisions({
      subjectId,
      memoryId: formed.memoryId,
      page: { pageSize: 1, pageToken: historyFirst.nextPageToken },
    });
    assert.equal(historyLast.items.length, 1);
    assert.equal(historyLast.items[0]!.revisionId, formed.revisionId);
    assert.deepEqual(historyLast.items[0]!.tags, formed.tags);
    assert.deepEqual(historyLast.items[0]!.basis, formed.basis);
    assert.equal(historyLast.nextPageToken, "");

    await stop(booted);
    booted = await boot(configPath);
    const restarted = await booted.client.memory.getMemory({
      subjectId,
      id: formed.memoryId,
    });
    if (restarted.revisionId !== formed.revisionId)
      throw new Error("restart changed the Authority revision");
    const recalledAfterRestart = await query(
      booted.client,
      undefined,
      "Alice is present",
    );
    if (
      !recalledAfterRestart.hits.some((hit) =>
        hitMatches(hit, formed.memoryId, formed.revisionId),
      )
    )
      throw new Error("restart lost public Query recall");

    const suppressed = await booted.client.memory.suppressMemory(
      create(MemoryMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009805",
        subjectId,
        memoryId: formed.memoryId,
        expectedObjectEpoch: restarted.objectEpoch,
      }),
    );
    const hidden = await query(booted.client, undefined, "Alice is present");
    if (
      hidden.hits.some((hit) =>
        hitMatches(hit, formed.memoryId, formed.revisionId),
      )
    )
      throw new Error("suppressed Memory remained query-visible");
    const restored = await booted.client.memory.restoreMemory(
      create(MemoryMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009806",
        subjectId,
        memoryId: formed.memoryId,
        expectedObjectEpoch: suppressed.objectEpoch,
      }),
    );
    if (restored.revisionId !== formed.revisionId)
      throw new Error("restore created a content revision");
    const visibleAgain = await query(
      booted.client,
      undefined,
      "Alice is present",
    );
    if (
      !visibleAgain.hits.some((hit) =>
        hitMatches(hit, formed.memoryId, formed.revisionId),
      )
    )
      throw new Error("restore did not restore public recall");

    await booted.client.memory.purgeMemory(
      create(MemoryMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009807",
        subjectId,
        memoryId: formed.memoryId,
        expectedObjectEpoch: restored.objectEpoch,
      }),
    );
    const purgedRetry = await booted.client.runtime.reportUse(useRequest);
    if (purgedRetry.duplicateCount !== 1)
      throw new Error("purged UseEvent retry lost its idempotency receipt");
    const afterPurge = await query(
      booted.client,
      undefined,
      "Alice is present",
    );
    if (
      afterPurge.hits.some((hit) =>
        hitMatches(hit, formed.memoryId, formed.revisionId),
      )
    )
      throw new Error("purged Memory remained query-visible");

    process.stdout.write(
      `MEMORY_REFERENCE_SMOKE subject=${subjectId} memory=${formed.memoryId} revision=${formed.revisionId} sessions=2 restart=true suppress_restore=true purge=true traceback=true\n`,
    );
  } finally {
    await stop(booted);
    await rm(root, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(
    error instanceof Error ? (error.stack ?? error.message) : String(error),
  );
  process.exitCode = 1;
});
