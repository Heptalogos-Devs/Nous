// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { boot, stop, prepare, type Boot } from "./support.js";
import { create } from "@bufbuild/protobuf";
import { TimestampSchema } from "@bufbuild/protobuf/wkt";
import {
  CognitiveRefSchema,
  CueSchema,
  CognitiveSeedSchema,
  CreateEpisodeRequestSchema,
  CreateSubjectRequestSchema,
  CreateWorkContextRequestSchema,
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
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { CognitionMutationRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/cognition_pb.js";
import { rm } from "node:fs/promises";
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
    await client.subjects.create(
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
    const sessionA = await client.cognition.openSession(
      create(SubjectRequestSchema, { subjectId }),
    );
    const observed = await client.cognition.observe(
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
    const memory = await client.memory.form(
      create(FormMemoryRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009904",
        subjectId,
        input: create(MemoryContentSchema, {
          cognitiveRole: "declarative",
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
    const memoryRevisionId = memory.revision?.header?.revisionRef?.value;
    if (!memoryRevisionId) throw new Error("Memory revision identity missing");
    let context = await client.cognition.createWorkContext(
      create(CreateWorkContextRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009905",
        subjectId,
        purpose: "continue the runtime task",
        cognitionAnchors: [
          create(CognitiveRefSchema, {
            kind: "memory_revision",
            value: memoryRevisionId,
          }),
        ],
      }),
    );
    const contextId = context.workContext?.workContextId;
    if (!contextId) throw new Error("WorkContext identity missing");
    const foregroundRequest = create(SetActiveWorkContextRequestSchema, {
      operationId: "00000000-0000-0000-0000-000000009906",
      subjectId,
      sessionId: sessionA.sessionId,
      expectedRuntimeRevision: sessionA.runtimeRevision,
      workContextId: contextId,
    });
    const foreground =
      await client.cognition.setActiveWorkContext(foregroundRequest);
    const foregroundReplay =
      await client.cognition.setActiveWorkContext(foregroundRequest);
    if (foreground.runtimeRevision !== foregroundReplay.runtimeRevision)
      throw new Error("foreground replay changed Runtime revision");
    const activeQuery = await client.cognition.query(
      contextQuery(sessionA.sessionId),
    );
    if (
      !activeQuery.hits.some((hit) => hit.revision?.value === memoryRevisionId)
    )
      throw new Error(
        "active WorkContext did not surface exact MemoryRevision",
      );
    context = await client.cognition.pauseWorkContext(
      create(WorkContextMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009907",
        subjectId,
        workContextId: contextId,
        expectedRevision: context.workContext?.revision,
      }),
    );
    const pausedSession = await client.cognition.getSession({
      subjectId,
      id: sessionA.sessionId,
    });
    if (pausedSession.activeWorkContextId)
      throw new Error("pause did not clear foreground binding");
    await stop(current);
    current = await boot(configPath);
    const sessionB = await current.client.cognition.openSession(
      create(SubjectRequestSchema, { subjectId }),
    );
    context = await current.client.cognition.resumeWorkContext(
      create(WorkContextMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009908",
        subjectId,
        workContextId: contextId,
        expectedRevision: context.workContext?.revision,
      }),
    );
    await current.client.cognition.setActiveWorkContext(
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
    if (!continued.hits.some((hit) => hit.revision?.value === memoryRevisionId))
      throw new Error(
        "cross-session WorkContext did not surface exact MemoryRevision",
      );

    const episode = await current.client.memory.createEpisode(
      create(CreateEpisodeRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009910",
        subjectId,
        trackKey: "interaction",
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
      }),
    );
    const episodeValue = episode.episode;
    const episodeId = episodeValue?.header?.objectRef?.value;
    const firstRevision = episodeValue?.revision?.header?.revisionRef?.value;
    if (!episodeValue?.header || !episodeId || !firstRevision)
      throw new Error("Episode identity missing");
    const revised = await current.client.memory.reviseEpisode({
      operationId: "00000000-0000-0000-0000-000000009911",
      subjectId,
      episodeId,
      expectedObjectEpoch: episodeValue.header.objectEpoch,
      intent: "resegment",
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
    });
    const history = await current.client.cognition.history({
      subjectId,
      objectRef: { kind: "episode", value: episodeId },
    });
    if (
      history.items.length !== 2 ||
      revised.episode?.header?.currentRevisionRef?.value === firstRevision
    )
      throw new Error(
        "Episode revision history did not preserve the first revision",
      );
    const exact = await current.client.cognition.read({
      subjectId,
      reference: { kind: "episode_revision", value: firstRevision },
    });
    if (
      exact.result.case !== "episode" ||
      exact.result.value.header?.objectRef?.value !== episodeId ||
      exact.result.value.revision?.header?.revisionRef?.value !== firstRevision
    )
      throw new Error("historical Episode revision read mismatch");
    const currentRevision = revised.episode?.header?.currentRevisionRef?.value;
    const revisedEpisode = revised.episode;
    if (!revisedEpisode || !currentRevision)
      throw new Error("current Episode revision missing");

    context = await current.client.cognition.updateWorkContext(
      create(UpdateWorkContextRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009912",
        subjectId,
        workContextId: contextId,
        expectedRevision: context.workContext?.revision,
        purpose: "continue the runtime task with an Episode",
        cognitionAnchors: [
          create(CognitiveRefSchema, {
            kind: "memory_revision",
            value: memoryRevisionId,
          }),
          create(CognitiveRefSchema, {
            kind: "episode_revision",
            value: currentRevision,
          }),
        ],
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
    const restartedEpisodeRead = await current.client.cognition.read({
      subjectId,
      reference: { kind: "episode", value: episodeId },
    });
    if (restartedEpisodeRead.result.case !== "episode")
      throw new Error("restart could not read the Episode");
    const restartedEpisode = restartedEpisodeRead.result.value;
    if (
      !restartedEpisode.header ||
      restartedEpisode.revision?.header?.revisionRef?.value !== currentRevision
    )
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

    await current.client.cognition.endWorkContext(
      create(WorkContextMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009918",
        subjectId,
        workContextId: contextId,
        expectedRevision: context.workContext?.revision,
      }),
    );
    const endedSession = await current.client.cognition.getSession({
      subjectId,
      id: sessionB.sessionId,
    });
    if (endedSession.activeWorkContextId)
      throw new Error("end did not clear foreground binding");

    const suppressedResponse = await current.client.cognition.mutate(
      create(CognitionMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009920",
        subjectId,
        objectRef: { kind: "episode", value: episodeId },
        expectedObjectEpoch: restartedEpisode.header.objectEpoch,
        action: { case: "suppress", value: {} },
      }),
    );
    if (
      suppressedResponse.outcome.case !== "cognition" ||
      suppressedResponse.outcome.value.result.case !== "episode"
    )
      throw new Error("Episode suppression did not return the Episode");
    const suppressedEpisode = suppressedResponse.outcome.value.result.value;
    if (!suppressedEpisode.header)
      throw new Error("suppressed Episode header missing");
    const restoredResponse = await current.client.cognition.mutate(
      create(CognitionMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009921",
        subjectId,
        objectRef: { kind: "episode", value: episodeId },
        expectedObjectEpoch: suppressedEpisode.header.objectEpoch,
        action: { case: "restore", value: {} },
      }),
    );
    if (
      restoredResponse.outcome.case !== "cognition" ||
      restoredResponse.outcome.value.result.case !== "episode"
    )
      throw new Error("Episode restoration did not return the Episode");
    const restoredEpisode = restoredResponse.outcome.value.result.value;
    if (
      !restoredEpisode.header ||
      restoredEpisode.revision?.header?.revisionRef?.value !== currentRevision
    )
      throw new Error("Episode restore created a content revision");
    const purged = await current.client.cognition.mutate(
      create(CognitionMutationRequestSchema, {
        operationId: "00000000-0000-0000-0000-000000009922",
        subjectId,
        objectRef: { kind: "episode", value: episodeId },
        expectedObjectEpoch: restoredEpisode.header.objectEpoch,
        action: { case: "purge", value: {} },
      }),
    );
    if (purged.outcome.case !== "purged")
      throw new Error("Episode purge did not complete");
    let purgeReadRejected = false;
    try {
      await current.client.cognition.read({
        subjectId,
        reference: { kind: "episode", value: episodeId },
      });
    } catch {
      purgeReadRejected = true;
    }
    if (!purgeReadRejected)
      throw new Error("purged Episode remained ordinarily readable");

    process.stdout.write(
      "COGNITIVE_RUNTIME_EPISODE_SMOKE subject=" +
        subjectId +
        " memory=" +
        memoryRevisionId +
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
