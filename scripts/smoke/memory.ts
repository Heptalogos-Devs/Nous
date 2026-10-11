// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

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
import { CognitionMutationRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/cognition_pb.js";
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
    const created = await client.subjects.create(
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

    const sessionA = await client.cognition.openSession(
      create(SubjectRequestSchema, { subjectId }),
    );
    const sessionB = await client.cognition.openSession(
      create(SubjectRequestSchema, { subjectId }),
    );
    const observed = await client.cognition.observe(
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
    const formed = await client.memory.form(
      create(FormMemoryRequestSchema, {
        operationId: formOperationId,
        subjectId,
        input: create(MemoryContentSchema, {
          cognitiveRole: "declarative",
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
    const memoryId = formed.header?.objectRef?.value;
    const revisionId = formed.revision?.header?.revisionRef?.value;
    if (!memoryId || !revisionId) throw new Error("Memory identity missing");
    if (
      !formed.revision?.details?.basis.some(
        (basis) =>
          basis.basis.case === "evidence" &&
          basis.basis.value.occurrenceId === observed.occurrenceId,
      )
    )
      throw new Error("Memory provenance does not trace to Observation");

    const prepared = await client.cognition.prepareQuery(
      create(QueryRequestSchema, {
        subjectId,
        expression: queryExpression("Alice is present"),
      }),
    );
    if (
      !prepared.queryId ||
      !prepared.representation?.text ||
      !prepared.plan?.conceptEnrichmentMode
    )
      throw new Error(
        "public PrepareQuery did not return the bound inspection",
      );

    const recalled = await query(client, undefined, "Alice is present");
    if (!recalled.hits.some((hit) => hitMatches(hit, memoryId, revisionId)))
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
            value: revisionId,
          }),
          kind: "referenced",
          occurredAt: instant,
        }),
      ],
    });
    const accepted = await client.cognition.reportUse(useRequest);
    const duplicate = await client.cognition.reportUse(useRequest);
    if (accepted.acceptedCount !== 1 || duplicate.duplicateCount !== 1)
      throw new Error("UseEvent retry was not idempotent");

    const residentA = await query(client, sessionA.sessionId);
    const residentB = await query(client, sessionB.sessionId);
    if (!residentA.hits.some((hit) => hitMatches(hit, memoryId, revisionId)))
      throw new Error("Session A did not expose its resident Memory");
    if (residentB.hits.some((hit) => hitMatches(hit, memoryId, revisionId)))
      throw new Error("Session B observed Session A residency");

    await stop(booted);
    booted = await boot(configPath);
    const restartedRead = await booted.client.cognition.read({
      subjectId,
      reference: { kind: "memory", value: memoryId },
    });
    if (restartedRead.result.case !== "memory")
      throw new Error("restart could not read the Memory");
    const restarted = restartedRead.result.value;
    if (
      !restarted.header ||
      restarted.revision?.header?.revisionRef?.value !== revisionId
    )
      throw new Error("restart changed the Authority revision");
    const recalledAfterRestart = await query(
      booted.client,
      undefined,
      "Alice is present",
    );
    if (
      !recalledAfterRestart.hits.some((hit) =>
        hitMatches(hit, memoryId, revisionId),
      )
    )
      throw new Error("restart lost public Query recall");

    const suppressedResponse = await booted.client.cognition.mutate(
      create(CognitionMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009805",
        subjectId,
        objectRef: { kind: "memory", value: memoryId },
        expectedObjectEpoch: restarted.header.objectEpoch,
        action: { case: "suppress", value: {} },
      }),
    );
    if (
      suppressedResponse.outcome.case !== "cognition" ||
      suppressedResponse.outcome.value.result.case !== "memory"
    )
      throw new Error("Memory suppression did not return the Memory");
    const suppressed = suppressedResponse.outcome.value.result.value;
    if (!suppressed.header) throw new Error("suppressed Memory header missing");
    const hidden = await query(booted.client, undefined, "Alice is present");
    if (hidden.hits.some((hit) => hitMatches(hit, memoryId, revisionId)))
      throw new Error("suppressed Memory remained query-visible");
    const restoredResponse = await booted.client.cognition.mutate(
      create(CognitionMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009806",
        subjectId,
        objectRef: { kind: "memory", value: memoryId },
        expectedObjectEpoch: suppressed.header.objectEpoch,
        action: { case: "restore", value: {} },
      }),
    );
    if (
      restoredResponse.outcome.case !== "cognition" ||
      restoredResponse.outcome.value.result.case !== "memory"
    )
      throw new Error("Memory restoration did not return the Memory");
    const restored = restoredResponse.outcome.value.result.value;
    if (
      !restored.header ||
      restored.revision?.header?.revisionRef?.value !== revisionId
    )
      throw new Error("restore created a content revision");
    const visibleAgain = await query(
      booted.client,
      undefined,
      "Alice is present",
    );
    if (!visibleAgain.hits.some((hit) => hitMatches(hit, memoryId, revisionId)))
      throw new Error("restore did not restore public recall");

    const purged = await booted.client.cognition.mutate(
      create(CognitionMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009807",
        subjectId,
        objectRef: { kind: "memory", value: memoryId },
        expectedObjectEpoch: restored.header.objectEpoch,
        action: { case: "purge", value: {} },
      }),
    );
    if (purged.outcome.case !== "purged")
      throw new Error("Memory purge did not complete");
    const purgedRetry = await booted.client.cognition.reportUse(useRequest);
    if (purgedRetry.duplicateCount !== 1)
      throw new Error("purged UseEvent retry lost its idempotency receipt");
    const afterPurge = await query(
      booted.client,
      undefined,
      "Alice is present",
    );
    if (afterPurge.hits.some((hit) => hitMatches(hit, memoryId, revisionId)))
      throw new Error("purged Memory remained query-visible");

    process.stdout.write(
      `MEMORY_REFERENCE_SMOKE subject=${subjectId} memory=${memoryId} revision=${revisionId} sessions=2 restart=true suppress_restore=true purge=true traceback=true\n`,
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
