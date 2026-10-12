// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { boot, stop, prepare, type Boot } from "./support.js";
import { create } from "@bufbuild/protobuf";
import { NousError } from "@nous-wave/client";
import { runCli } from "../../apps/nous-cli/src/commands/index.js";
import { TimestampSchema } from "@bufbuild/protobuf/wkt";
import {
  CognitiveRefSchema,
  CueSchema,
  CognitiveSeedSchema,
  CreateEpisodeRequestSchema,
  CreateSubjectRequestSchema,
  CreateWorkContextRequestSchema,
  EpisodeMutationRequestSchema,
  EpisodeMemberSchema,
  EvidenceRefSchema,
  FormMemoryRequestSchema,
  InlineTextSchema,
  MemoryContentSchema,
  ObservationInputSchema,
  QueryExprSchema,
  QueryRequestSchema,
  RevisionBasisSchema,
  SetActiveWorkContextRequestSchema,
  SubjectCapabilitiesSchema,
  SubjectRequestSchema,
  TemporalExtentSchema,
  UpdateWorkContextRequestSchema,
  WorkContextMutationRequestSchema,
  WorkContextClearScope,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { rm, writeFile } from "node:fs/promises";
import { workspaceTemp } from "../workspace.js";

const subjectId = "00000000-0000-0000-0000-000000009901";
const instant = create(TimestampSchema, {
  seconds: BigInt(Math.floor(Date.now() / 1000)),
  nanos: 0,
});

function contextQuery(sessionId: string) {
  return create(QueryRequestSchema, {
    subjectId,
    sessionId,
    expression: create(QueryExprSchema, {
      operation: "atom",
      cues: [
        create(CueSchema, {
          cue: { case: "text", value: "runtime continuity" },
        }),
      ],
    }),
  });
}

function evidence(occurrenceId: string) {
  return create(RevisionBasisSchema, {
    basis: {
      case: "evidence",
      value: create(EvidenceRefSchema, {
        occurrenceId,
        locator: { case: "wholeOccurrence", value: true },
        basisRole: "direct",
      }),
    },
  });
}

async function main() {
  const root = await workspaceTemp("smoke", "runtime-");
  const configPath = await prepare(root);
  let current: Boot | undefined;
  try {
    current = await boot(configPath);
    const client = current.client;
    await client.subjects.createSubject(
      create(CreateSubjectRequestSchema, {
        subjectId,
        operationId: "00000000-0000-0000-0000-000000009902",
        cognitiveSeed: create(CognitiveSeedSchema, {
          text: "schema_version = 1",
          format: "application/vnd.nous-wave.cognitive-seed+toml;version=1",
        }),
        capabilities: create(SubjectCapabilitiesSchema, { memory: true }),
      }),
    );
    const sessionA = await client.runtime.openSession(
      create(SubjectRequestSchema, { subjectId }),
    );
    const observed = await client.runtime.recordObservation(
      create(ObservationInputSchema, {
        subjectId,
        sessionId: sessionA.sessionId,
        requestId: "00000000-0000-0000-0000-000000009903",
        sourceClass: "message",
        observedAt: instant,
        admit: false,
        material: {
          case: "inlineText",
          value: create(InlineTextSchema, {
            text: "runtime continuity",
            mediaType: "text/plain",
          }),
        },
      }),
    );
    const memory = await client.memory.formMemory(
      create(FormMemoryRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009904",
        subjectId,
        cognitiveRole: "declarative",
        input: create(MemoryContentSchema, {
          formationMode: "grounded",
          groundingOccurrenceId: observed.occurrenceId,
          semanticRole: "fact",
          text: "runtime continuity",
          basis: [evidence(observed.occurrenceId)],
          validTime: create(TemporalExtentSchema, {}),

          epistemicClass: "observed",
        }),
      }),
    );
    let context = await client.runtime.createWorkContext(
      create(CreateWorkContextRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009905",
        subjectId,
        purpose: "continue the runtime task",
        cognitionAnchors: [
          create(CognitiveRefSchema, {
            kind: "memory_revision",
            value: memory.revisionId,
          }),
        ],
      }),
    );
    const contextId = context.workContext?.workContextId;
    if (!contextId) throw new Error("WorkContext identity missing");
    const createdRevision = context.workContext!.revision;
    context = await client.runtime.updateWorkContext({
      subjectId,
      workContextId: contextId,
      operationId: crypto.randomUUID(),
      expectedRevision: createdRevision,
      intent: {
        case: "fields",
        value: {
          contextText: "continuity background",
          constraints: { mode: "bounded" },
          unresolvedQuestions: { values: ["next step"] },
        },
      },
    });
    if (
      context.workContext?.purpose !== "continue the runtime task" ||
      context.workContext.contextText !== "continuity background"
    )
      throw new Error("WorkContext fields did not preserve omitted purpose");
    let staleRejected = false;
    try {
      await client.runtime.updateWorkContext({
        subjectId,
        workContextId: contextId,
        operationId: crypto.randomUUID(),
        expectedRevision: createdRevision,
        intent: { case: "fields", value: { purpose: "stale update" } },
      });
    } catch (error) {
      staleRejected =
        error instanceof NousError && error.domainCode === "STALE_REVISION";
    }
    if (!staleRejected)
      throw new Error("WorkContext accepted a stale mutation");
    context = await client.runtime.updateWorkContext({
      subjectId,
      workContextId: contextId,
      operationId: crypto.randomUUID(),
      expectedRevision: context.workContext!.revision,
      intent: {
        case: "fields",
        value: {
          contextText: "",
          unresolvedQuestions: { values: [] },
          constraints: {},
        },
      },
    });
    if (
      context.workContext?.contextText !== "" ||
      context.workContext.unresolvedQuestions.length ||
      Object.keys(context.workContext.constraints ?? {}).length
    )
      throw new Error("WorkContext did not apply explicit empty fields");
    context = await client.runtime.updateWorkContext({
      subjectId,
      workContextId: contextId,
      operationId: crypto.randomUUID(),
      expectedRevision: context.workContext!.revision,
      intent: {
        case: "unpin",
        value: {
          cognitionAnchors: [
            { kind: "memory_revision", value: memory.revisionId },
          ],
        },
      },
    });
    if (context.workContext?.cognitionAnchors.length)
      throw new Error("WorkContext unpin failed");
    context = await client.runtime.updateWorkContext({
      subjectId,
      workContextId: contextId,
      operationId: crypto.randomUUID(),
      expectedRevision: context.workContext!.revision,
      intent: {
        case: "pin",
        value: {
          cognitionAnchors: [
            { kind: "memory_revision", value: memory.revisionId },
          ],
        },
      },
    });
    if (context.workContext?.cognitionAnchors.length !== 1)
      throw new Error("WorkContext pin failed");
    const disposable = await client.runtime.createWorkContext({
      subjectId,
      operationId: crypto.randomUUID(),
      purpose: "clear and pagination",
      contextText: "to clear",
      cognitionAnchors: [{ kind: "memory_revision", value: memory.revisionId }],
    });
    const cleared = await client.runtime.updateWorkContext({
      subjectId,
      workContextId: disposable.workContext!.workContextId,
      operationId: crypto.randomUUID(),
      expectedRevision: disposable.workContext!.revision,
      intent: { case: "clear", value: WorkContextClearScope.ALL },
    });
    if (
      cleared.workContext?.contextText !== "" ||
      cleared.workContext.cognitionAnchors.length ||
      cleared.workContext.purpose !== "clear and pagination"
    )
      throw new Error("WorkContext clear did not preserve purpose");
    const firstContexts = await client.runtime.listWorkContexts({
      subjectId,
      page: { pageSize: 1 },
    });
    if (firstContexts.items.length !== 1 || !firstContexts.nextPageToken)
      throw new Error("WorkContext pagination did not emit cursor");
    const nextContexts = await client.runtime.listWorkContexts({
      subjectId,
      page: { pageSize: 1, pageToken: firstContexts.nextPageToken },
    });
    if (
      nextContexts.items.length !== 1 ||
      nextContexts.items[0]!.workContextId ===
        firstContexts.items[0]!.workContextId ||
      nextContexts.nextPageToken
    )
      throw new Error("WorkContext pagination repeated or lost a context");
    const cliFlags = [
      "--run-root",
      `${root}/run`,
      "--instance-root",
      `${root}/instance`,
      "--subject",
      subjectId,
      "--session",
      sessionA.sessionId,
      "--consumer",
      "consumer:smoke:cognitive-runtime-episode",
    ];
    const createOperation = crypto.randomUUID();
    const createdSubject = (await runCli([
      "subject",
      "create",
      "--operation-id",
      createOperation,
      ...cliFlags,
    ])) as { data: { subjectId: string } };
    const replayedSubject = (await runCli([
      "subject",
      "create",
      "--operation-id",
      createOperation,
      ...cliFlags,
    ])) as { data: { subjectId: string } };
    if (replayedSubject.data.subjectId !== createdSubject.data.subjectId)
      throw new Error("CLI Subject create replay allocated another Subject");
    const changedCreation = `${root}/changed-subject.toml`;
    await writeFile(
      changedCreation,
      '[cognitive_seed]\ntext = "schema_version = 1"\nformat = "application/vnd.nous-wave.cognitive-seed+toml;version=1"\n[capabilities]\nmemory = true\n[metadata]\nnote = "changed input"\n',
    );
    let createConflict = false;
    try {
      await runCli([
        "subject",
        "create",
        "--operation-id",
        createOperation,
        "--request-file",
        changedCreation,
        ...cliFlags,
      ]);
    } catch (error) {
      createConflict =
        error instanceof NousError &&
        error.domainCode === "OPERATION_ID_CONFLICT";
    }
    if (!createConflict)
      throw new Error(
        "CLI Subject create changed-input retry did not conflict",
      );
    const createdSeed = await client.subjects.getCognitiveSeed({
      subjectId: createdSubject.data.subjectId,
    });
    if (
      !createdSubject.data.subjectId ||
      createdSeed.operationId !== createOperation
    )
      throw new Error("CLI Subject create ignored operation identity");
    const cliCreated = (await runCli([
      "context",
      "create",
      "--purpose",
      "local selection only",
      ...cliFlags,
    ])) as { data: { workContextId: string } };
    await runCli(["projection", "build", ...cliFlags]);
    await runCli(["context", "select", contextId, ...cliFlags]);
    await runCli(["projection", "managed", ...cliFlags]);
    const unforegrounded = await client.runtime.getSession({
      subjectId,
      id: sessionA.sessionId,
    });
    if (unforegrounded.activeWorkContextId || !cliCreated.data.workContextId)
      throw new Error("CLI local selection foregrounded a WorkContext");
    const acceptedMemories = (await runCli([
      "memory",
      "list",
      "--status",
      "accepted",
      ...cliFlags,
    ])) as { data: { items: { memoryId: string }[] } };
    const withdrawnMemories = (await runCli([
      "memory",
      "list",
      "--status",
      "withdrawn",
      ...cliFlags,
    ])) as { data: { items: unknown[] } };
    if (
      !acceptedMemories.data.items.some(
        (item) => item.memoryId === memory.memoryId,
      ) ||
      withdrawnMemories.data.items.length
    )
      throw new Error("CLI Memory list did not apply status");
    const fencedMemory = (await runCli([
      "memory",
      "accessibility",
      `memory:${memory.memoryId}`,
      "--kind",
      "normal",
      "--expected-object-epoch",
      String(memory.objectEpoch),
      ...cliFlags,
    ])) as { data: { objectEpoch: bigint } };
    if (fencedMemory.data.objectEpoch <= memory.objectEpoch)
      throw new Error("CLI mutation did not advance the object epoch");
    let staleObjectEpoch = false;
    try {
      await runCli([
        "memory",
        "accessibility",
        `memory:${memory.memoryId}`,
        "--kind",
        "auto",
        "--expected-object-epoch",
        String(memory.objectEpoch),
        ...cliFlags,
      ]);
    } catch (error) {
      staleObjectEpoch =
        error instanceof NousError && error.domainCode === "STALE_REVISION";
    }
    if (!staleObjectEpoch)
      throw new Error(
        "CLI replaced the caller's stale object epoch with current state",
      );
    const closedSessions = (await runCli([
      "session",
      "list",
      "--status",
      "closed",
      ...cliFlags,
    ])) as { data: { items: unknown[] } };
    if (closedSessions.data.items.length)
      throw new Error("CLI Session list did not apply status");
    await runCli(["episode", "list", "--status", "withdrawn", ...cliFlags]);
    await runCli(["journal", "list", "--status", "withdrawn", ...cliFlags]);
    await runCli(["resource", "list", "--status", "unavailable", ...cliFlags]);
    const cleanedMaterial = (await runCli([
      "material",
      "clean",
      "--run-root",
      `${root}/run`,
      "--instance-root",
      `${root}/cleanup-consumer`,
    ])) as { data: { objectsDeleted: bigint; stagingDeleted: bigint } };
    if (
      typeof cleanedMaterial.data.objectsDeleted !== "bigint" ||
      typeof cleanedMaterial.data.stagingDeleted !== "bigint" ||
      cleanedMaterial.data.objectsDeleted < 0n ||
      cleanedMaterial.data.stagingDeleted < 0n
    )
      throw new Error("Material cleanup returned invalid counts");
    const seedAfterCleanup = await client.subjects.getCognitiveSeed({
      subjectId: createdSubject.data.subjectId,
    });
    if (seedAfterCleanup.artifactId !== createdSeed.artifactId)
      throw new Error("Material cleanup lost another Subject's Artifact");
    const foregroundRequest = create(SetActiveWorkContextRequestSchema, {
      operationId: "00000000-0000-0000-0000-000000009906",
      subjectId,
      sessionId: sessionA.sessionId,
      expectedRuntimeRevision: sessionA.runtimeRevision,
      workContextId: contextId,
    });
    const foreground =
      await client.runtime.setActiveWorkContext(foregroundRequest);
    const foregroundReplay =
      await client.runtime.setActiveWorkContext(foregroundRequest);
    if (foreground.runtimeRevision !== foregroundReplay.runtimeRevision)
      throw new Error("foreground replay changed Runtime revision");
    const activeQuery = await client.cognition.query(
      contextQuery(sessionA.sessionId),
    );
    if (
      !activeQuery.hits.some((hit) => hit.revision?.value === memory.revisionId)
    )
      throw new Error(
        "active WorkContext did not surface exact MemoryRevision",
      );
    context = await client.runtime.pauseWorkContext(
      create(WorkContextMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009907",
        subjectId,
        workContextId: contextId,
        expectedRevision: context.workContext?.revision,
      }),
    );
    const pausedSession = await client.runtime.getSession({
      subjectId,
      id: sessionA.sessionId,
    });
    if (pausedSession.activeWorkContextId)
      throw new Error("pause did not clear foreground binding");
    await stop(current);
    current = await boot(configPath);
    const sessionB = await current.client.runtime.openSession(
      create(SubjectRequestSchema, { subjectId }),
    );
    context = await current.client.runtime.resumeWorkContext(
      create(WorkContextMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009908",
        subjectId,
        workContextId: contextId,
        expectedRevision: context.workContext?.revision,
      }),
    );
    await current.client.runtime.setActiveWorkContext(
      create(SetActiveWorkContextRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009909",
        subjectId,
        sessionId: sessionB.sessionId,
        expectedRuntimeRevision: sessionB.runtimeRevision,
        workContextId: contextId,
      }),
    );
    const continued = await current.client.cognition.query(
      contextQuery(sessionB.sessionId),
    );
    if (
      !continued.hits.some((hit) => hit.revision?.value === memory.revisionId)
    )
      throw new Error(
        "cross-session WorkContext did not surface exact MemoryRevision",
      );

    const episode = await current.client.memory.createEpisode(
      create(CreateEpisodeRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009910",
        subjectId,
        trackKey: "interaction",
        content: {
          boundaryExplanation: "one bounded interaction",
          experienceTime: create(TemporalExtentSchema, {}),

          members: [
            create(EpisodeMemberSchema, {
              reference: create(CognitiveRefSchema, {
                kind: "occurrence",
                value: observed.occurrenceId,
              }),
              role: "member",
            }),
          ],
          basis: [evidence(observed.occurrenceId)],
        },
      }),
    );
    const episodeValue = episode.episode;
    const episodeId = episodeValue?.episodeId;
    const firstRevision = episodeValue?.currentRevisionId;
    if (!episodeValue || !episodeId || !firstRevision)
      throw new Error("Episode identity missing");
    const revised = await current.client.memory.reviseEpisode({
      operationId: "00000000-0000-0000-0000-000000009911",
      subjectId,
      episodeId,
      expectedObjectEpoch: episodeValue.objectEpoch,
      intent: "resegment",
      content: {
        boundaryExplanation: "resegmented interaction",
        experienceTime: create(TemporalExtentSchema, {}),

        members: [
          create(EpisodeMemberSchema, {
            reference: create(CognitiveRefSchema, {
              kind: "occurrence",
              value: observed.occurrenceId,
            }),
            role: "member",
          }),
        ],
        basis: [evidence(observed.occurrenceId)],
      },
    });
    const history = await current.client.memory.listEpisodeRevisions({
      subjectId,
      episodeId,
    });
    if (
      history.items.length !== 2 ||
      revised.episode?.currentRevisionId === firstRevision
    )
      throw new Error(
        "Episode revision history did not preserve the first revision",
      );
    const exact = await current.client.memory.getEpisodeRevision({
      subjectId,
      id: firstRevision,
    });
    if (exact.episodeId !== episodeId)
      throw new Error("historical Episode revision read mismatch");
    const currentRevision = revised.episode?.currentRevisionId;
    const revisedEpisode = revised.episode;
    if (!revisedEpisode || !currentRevision)
      throw new Error("current Episode revision missing");

    context = await current.client.runtime.updateWorkContext(
      create(UpdateWorkContextRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009912",
        subjectId,
        workContextId: contextId,
        expectedRevision: context.workContext?.revision,
        intent: {
          case: "pin",
          value: {
            cognitionAnchors: [
              create(CognitiveRefSchema, {
                kind: "episode_revision",
                value: currentRevision,
              }),
            ],
          },
        },
      }),
    );
    const episodeRuntimeQuery = await current.client.cognition.query(
      contextQuery(sessionB.sessionId),
    );
    if (
      !episodeRuntimeQuery.hits.some(
        (hit) => hit.reference?.value === currentRevision,
      )
    )
      throw new Error("WorkContext did not surface exact EpisodeRevision");

    await stop(current);
    current = await boot(configPath);
    const restartedEpisode = await current.client.memory.getEpisode({
      subjectId,
      id: episodeId,
    });
    if (restartedEpisode.episode?.currentRevisionId !== currentRevision)
      throw new Error("restart changed the current Episode revision");
    const restartedQuery = await current.client.cognition.query(
      contextQuery(sessionB.sessionId),
    );
    if (
      !restartedQuery.hits.some(
        (hit) => hit.reference?.value === currentRevision,
      )
    )
      throw new Error("restart lost WorkContext EpisodeRevision recall");

    await current.client.runtime.endWorkContext(
      create(WorkContextMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009918",
        subjectId,
        workContextId: contextId,
        expectedRevision: context.workContext?.revision,
      }),
    );
    const endedSession = await current.client.runtime.getSession({
      subjectId,
      id: sessionB.sessionId,
    });
    if (endedSession.activeWorkContextId)
      throw new Error("end did not clear foreground binding");

    const suppressedEpisode = await current.client.memory.suppressEpisode(
      create(EpisodeMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009920",
        subjectId,
        episodeId,
        expectedObjectEpoch: restartedEpisode.episode?.objectEpoch,
      }),
    );
    const restoredEpisode = await current.client.memory.restoreEpisode(
      create(EpisodeMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009921",
        subjectId,
        episodeId,
        expectedObjectEpoch: suppressedEpisode.episode?.objectEpoch,
      }),
    );
    if (restoredEpisode.episode?.currentRevisionId !== currentRevision)
      throw new Error("Episode restore created a content revision");
    await current.client.memory.purgeEpisode(
      create(EpisodeMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009922",
        subjectId,
        episodeId,
        expectedObjectEpoch: restoredEpisode.episode?.objectEpoch,
      }),
    );
    let purgeReadRejected = false;
    try {
      await current.client.memory.getEpisode({ subjectId, id: episodeId });
    } catch {
      purgeReadRejected = true;
    }
    if (!purgeReadRejected)
      throw new Error("purged Episode remained ordinarily readable");

    process.stdout.write(
      "COGNITIVE_RUNTIME_EPISODE_SMOKE subject=" +
        subjectId +
        " memory=" +
        memory.revisionId +
        " work_context=" +
        contextId +
        " episode=" +
        episodeId +
        " historical_revision=" +
        firstRevision +
        " restart=true cross_session=true episode_revision=true lifecycle=true purge=true\n",
    );
  } finally {
    await stop(current);
    await rm(root, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(
    error instanceof Error ? (error.stack ?? error.message) : String(error),
  );
  process.exitCode = 1;
});
