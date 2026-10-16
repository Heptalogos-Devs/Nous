// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { createServer } from "node:http";
import { createInterface } from "node:readline";
import {
  timestampFromDate,
  TimestampSchema,
  ValueSchema,
} from "@bufbuild/protobuf/wkt";
import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { connectNous } from "@nous-wave/client/node";
import { MaintenancePlanSchema } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { CognitiveRefSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { KernelClient } from "../../apps/nous-core/src/kernel-client.js";
import { createCore } from "../../apps/nous-core/src/server.js";
import { parseEffectiveConfiguration } from "../../apps/nous-core/src/configuration/schema.js";
import { ConfigExposure } from "@nous-wave/client";
import { SubjectMaintenanceScheduler } from "../../apps/nous-core/src/maintenance/grants.js";
import { DomainErrorCode } from "@nous-wave/client/errors";
import { modelConfigurationSchema } from "../../apps/nous-core/src/model/configuration.js";
import { GenerationFailure } from "../../apps/nous-core/src/model/execution/routes.js";
import { ModelRuntime } from "../../apps/nous-core/src/model/runtime.js";
import {
  episodePartitionSchema,
  journalSynthesisSchema,
} from "../../apps/nous-core/src/model/schemas/longitudinal.js";
import { consolidationSchema } from "../../apps/nous-core/src/model/schemas/consolidation.js";

function proposalModels() {
  const models = new ModelRuntime();
  Object.defineProperty(models.invocations, "capabilities", {
    get: () =>
      ["episode_segmentation", "journal_synthesis", "memory_consolidation"].map(
        (role) => ({
          name: `model.${role}`,
          state: "READY",
          detail: "Executable smoke proposal adapter",
        }),
      ),
  });
  const calls = { episode: 0, journal: 0, consolidation: 0 };
  models.invocations.profile = (role) =>
    [
      "episode_segmentation",
      "journal_synthesis",
      "memory_consolidation",
    ].includes(role)
      ? ({} as NonNullable<ReturnType<typeof models.invocations.profile>>)
      : undefined;
  models.invocations.prepare = (role) => {
    if (!models.invocations.profile(role))
      throw new GenerationFailure(role, "role_unavailable");
    return { role, routes: [], configDigest: "b".repeat(64) };
  };
  const producerMetadata = {
    modelRole: "journal_synthesis" as const,
    modelProfile: "stub",
    executionProfile: "stub",
    inferenceControlsDigest: "e".repeat(64),
    rolePolicyDigest: "f".repeat(64),
    implementation: "longitudinal-proposal-stub",
    protocol: "openai-chat",
    model: "deterministic",
    profileDigest: "a".repeat(64),
    configDigest: "b".repeat(64),
    outputSchemaDigest: "c".repeat(64),
    promptId: "smoke/longitudinal",
    promptDigest: "d".repeat(64),
  };
  const plan = (input: string) =>
    fromJson(MaintenancePlanSchema, JSON.parse(input) as JsonValue);
  const segmentEpisode = async (input: string, beforeAttempt?: () => void) => {
    beforeAttempt?.();
    calls.episode++;
    const source = plan(input);
    return {
      value: episodePartitionSchema.parse({
        action: "partition",
        segments: [
          {
            memberKeys: source.members.map((member) => member.key),
            title: "Calibration sequence",
            boundaryExplanation: "Sequential calibration observations.",
          },
        ],
      }),
      producerMetadata,
      execution: {
        attempts: [
          {
            executionProfile: "stub",
            modelProfile: "stub",
            status: "succeeded" as const,
            latencyMs: 0,
          },
        ],
        successfulExecutionProfile: "stub",
      },
    };
  };
  const synthesizeJournal = async (
    input: string,
    beforeAttempt?: () => void,
  ) => {
    beforeAttempt?.();
    calls.journal++;
    const source = plan(input);
    const basisKeys = source.basis
      .filter(
        (entry) =>
          entry.basis?.basis.case === "cognitionDependency" &&
          entry.basis.basis.value.targetRevision?.kind === "episode_revision",
      )
      .map((entry) => entry.key);
    return {
      value: journalSynthesisSchema.parse({
        action: "commit",
        title: "Calibration journal",
        narrative: "Calibration plan confirmed.",
        points: [
          {
            role: "decision",
            text: "Calibration plan confirmed.",
            basisKeys,
          },
        ],
      }),
      producerMetadata,
      execution: {
        attempts: [
          {
            executionProfile: "stub",
            modelProfile: "stub",
            status: "succeeded" as const,
            latencyMs: 0,
          },
        ],
        successfulExecutionProfile: "stub",
      },
    };
  };
  const consolidate = async (input: string, beforeAttempt?: () => void) => {
    beforeAttempt?.();
    calls.consolidation++;
    const source = plan(input);
    const member = source.members[0];
    assert(member);
    const evidence = source.basis.find(
      (entry) =>
        entry.basis?.basis.case === "evidence" &&
        entry.basis.basis.value.occurrenceId === member.occurrenceId,
    );
    assert(evidence);
    const actions = source.candidates.some(
      (candidate) => candidate.target?.reference?.kind === "memory_revision",
    )
      ? [
          {
            action: "skip",
            reason: "The supported calibration Memory is available.",
          },
        ]
      : [
          {
            action: "create_memory",
            content: {
              cognitiveRole: "experiential",
              formationMode: "grounded",
              groundingMemberKey: member.key,
              semanticRole: "calibration",
              text: "Calibration plan confirmed.",
              title: "Calibration",
              basisKeys: [evidence.key],
              entityKeys: [],
              validTime: { kind: "unknown" },
              epistemicClass: "observed",
            },
          },
        ];
    return {
      value: consolidationSchema.parse({ actions }),
      producerMetadata,
      execution: {
        attempts: [
          {
            executionProfile: "stub",
            modelProfile: "stub",
            status: "succeeded" as const,
            latencyMs: 0,
          },
        ],
        successfulExecutionProfile: "stub",
      },
    };
  };
  models.invocations.generate = (async (
    role,
    input,
    options?: Parameters<typeof models.invocations.generate>[2],
  ) => {
    const content = input.content as string;
    if (role === "episode_segmentation")
      return segmentEpisode(content, options?.beforeAttempt);
    if (role === "journal_synthesis")
      return synthesizeJournal(content, options?.beforeAttempt);
    if (role === "memory_consolidation")
      return consolidate(content, options?.beforeAttempt);
    throw new Error(`Unexpected smoke role ${role}`);
  }) as typeof models.invocations.generate;
  return { models, calls };
}

async function scenario(endpoint: string, token: string) {
  const channel = createInterface({ input: process.stdin });
  const replies = channel[Symbol.asyncIterator]();
  const control = async (request: Record<string, unknown>) => {
    process.stdout.write(JSON.stringify(request) + "\n");
    const response = await replies.next();
    assert(!response.done, "harness control closed");
    return JSON.parse(response.value) as { endpoint?: string };
  };
  let { models, calls } = proposalModels();
  let recoveryProvider: ReturnType<typeof createServer> | undefined;
  async function openCore(kernelEndpoint: string) {
    let kernel = KernelClient.connect(kernelEndpoint, token);
    const snapshot = await kernel.configuration.getConfiguration({
      exposureCeiling: ConfigExposure.DEVELOPER,
    });
    const effective = parseEffectiveConfiguration(
      Object.fromEntries(
        snapshot.entries.map((entry) => [
          entry.path,
          entry.value ? toJson(ValueSchema, entry.value) : null,
        ]),
      ),
    );
    kernel = KernelClient.connect(kernelEndpoint, token, effective.execution);
    return createCore({
      kernel,
      token,
      consumers: [],
      models,
      execution: effective.execution,
      maxUploadBytes: effective.maxUploadBytes,
    });
  }
  let app = await openCore(endpoint);
  let publicEndpoint = await app.listen({ host: "127.0.0.1", port: 0 });
  let client = connectNous(publicEndpoint, token);
  const subjectId = randomUUID();
  const advance = async (seconds: number) =>
    control({ command: "advance", subject: subjectId, seconds });
  const maintain = async (maxModelCalls = 4, maxOperations = 4) => {
    const result = await client.cognition.grantMaintenance({
      subjectId,
      maxOperations,
      maxModelCalls,
      maxElapsedMs: 10000,
    });
    assert(
      !result.results.some((item) => item.status === "internal_failure"),
      "maintenance returned an internal failure",
    );
    for (const operation of result.results)
      assert(
        operation.status !== "rejected_invalid" &&
          (operation.status !== "blocked_dependency" ||
            operation.problemCode === "source_not_settled"),
        JSON.stringify(operation),
      );
    return result;
  };
  try {
    await client.subjects.createSubject({
      subjectId,
      operationId: randomUUID(),
      cognitiveSeed: {
        text: "schema_version = 1",
        format: "application/vnd.nous-wave.cognitive-seed+toml;version=1",
        provenance: {},
      },
    });
    const session = await client.runtime.openSession({ subjectId });
    assert(session.openedAt);
    assert(session.lastActivityAt);
    assert.equal(session.closedAt, undefined);
    const observationIds: string[] = [];
    for (const text of [
      "Calibration plan confirmed.",
      "Calibration measurements collected.",
      "Calibration review completed.",
    ]) {
      const observed = await client.runtime.recordObservation({
        subjectId,
        sessionId: session.sessionId,
        sourceClass: "message",
        material: {
          case: "inlineText",
          value: { text, mediaType: "text/plain" },
        },
        context: {},
      });
      observationIds.push(observed.occurrenceId);
      await advance(2);
    }
    assert.deepEqual(calls, { episode: 0, journal: 0, consolidation: 0 });
    const nearline = await client.cognition.grantMaintenance({
      subjectId,
      maxOperations: 1,
      maxModelCalls: 0,
      maxElapsedMs: 10000,
    });
    assert.equal(nearline.modelCalls, 0);
    assert.equal(
      (await client.memory.listEpisodes({ subjectId, status: "accepted" }))
        .items.length,
      0,
    );
    await advance(2000);
    await maintain();
    assert.equal(
      (await client.memory.listEpisodes({ subjectId, status: "accepted" }))
        .items.length,
      1,
    );
    for (let pass = 0; pass < 4; pass++) {
      await advance(400);
      if (pass === 0) await Promise.all([maintain(), maintain()]);
      else await maintain();
    }
    const episodes = (
      await client.memory.listEpisodes({ subjectId, status: "accepted" })
    ).items;
    const journals = (
      await client.memory.listJournals({ subjectId, status: "accepted" })
    ).items;
    const memories = (
      await client.memory.listMemories({ subjectId, status: "accepted" })
    ).items;
    assert.equal(episodes.length, 1);
    assert.equal(journals.length, 1);
    assert.equal(memories.length, 1);
    const episode = episodes[0]!;
    const journal = journals[0]!;
    const memory = memories[0]!;
    assert.equal(episode.currentRevision?.members.length, 3);
    assert.equal(episode.currentRevision?.title, "Calibration sequence");
    assert(
      journal.currentRevision?.points.every((point) => point.basis.length > 0),
    );
    assert.equal(calls.episode, 1);
    assert(calls.journal > 0 && calls.consolidation > 0);
    const refs = [
      {
        kind: "episode_revision",
        value: episode.currentRevision!.episodeRevisionId,
      },
      {
        kind: "journal_revision",
        value: journal.currentRevision!.journalRevisionId,
      },
      {
        kind: "memory_revision",
        value: memory.revisionId,
      },
    ].map((ref) => create(CognitiveRefSchema, ref));
    await app.close();
    const restarted = await control({ command: "restart" });
    assert(restarted.endpoint);
    app = await openCore(restarted.endpoint);
    publicEndpoint = await app.listen({ host: "127.0.0.1", port: 0 });
    client = connectNous(publicEndpoint, token);
    assert.equal(
      (
        await client.memory.getEpisodeRevision({
          subjectId,
          id: refs[0]!.value,
        })
      ).episodeRevisionId,
      refs[0]!.value,
    );
    assert.equal(
      (
        await client.memory.getJournalRevision({
          subjectId,
          id: refs[1]!.value,
        })
      ).journalRevisionId,
      refs[1]!.value,
    );
    assert.equal(
      (await client.memory.getMemoryRevision({ subjectId, id: refs[2]!.value }))
        .revisionId,
      refs[2]!.value,
    );
    const continuation = await client.runtime.createWorkContext({
      operationId: randomUUID(),
      subjectId,
      purpose: "Continue calibration",
      cognitionAnchors: refs,
    });
    assert.deepEqual(continuation.workContext?.cognitionAnchors, refs);
    const exact = await client.cognition.query({
      subjectId,
      expression: {
        operation: "atom",
        cues: [
          {
            cue: {
              case: "text",
              value: "Inspect selected calibration cognition",
            },
          },
          ...refs.map((reference) => ({
            cue: { case: "reference" as const, value: reference },
          })),
        ],
      },
    });
    assert.equal(exact.hits.length, 3);
    const query = await client.cognition.query({
      subjectId,
      expression: {
        operation: "atom",
        cues: [{ cue: { case: "text", value: "Calibration" } }],
        children: [],
        modifiers: {
          projection: { domains: ["episode", "journal", "memory"] },
          limit: 16,
        },
      },
    });
    for (const reference of refs)
      assert(
        query.hits.some(
          (hit) =>
            hit.reference?.kind === reference.kind &&
            hit.reference.value === reference.value,
        ),
      );
    const use = {
      subjectId,
      sessionId: session.sessionId,
      consumerRef: "consumer:smoke:longitudinal",
      events: refs.map((reference) => ({
        eventId: randomUUID(),
        reference,
        kind: "referenced",
        occurredAt: timestampFromDate(new Date("2026-10-03T01:00:00Z")),
        context: {},
      })),
    };
    assert.equal((await client.runtime.reportUse(use)).acceptedCount, 3);
    assert.equal((await client.runtime.reportUse(use)).duplicateCount, 3);
    const usedSession = await client.runtime.getSession({
      subjectId,
      id: session.sessionId,
    });
    assert(usedSession.lastMeaningfulUseAt);
    assert(
      usedSession.resident.some(
        (resident) =>
          resident.reference?.value === refs[0]?.value &&
          resident.state === "resident" &&
          resident.entryReason &&
          resident.enteredAt &&
          resident.lastMeaningfulUseAt,
      ),
    );
    const before = { ...calls };
    await maintain();
    assert.deepEqual(calls, before);
    const schema = await client.concepts.createCognitiveSchema({
      operationId: randomUUID(),
      subjectId,
      schema: {
        title: "Calibration source pattern",
        structuralClaim: "Calibration decisions retain their actual source",
        applicabilityScope: {
          description: "This calibration sequence",
          validTime: {
            value: {
              case: "interval",
              value: {
                start: fromJson(TimestampSchema, "2026-10-03T00:00:00Z"),
                end: fromJson(TimestampSchema, "2026-10-04T00:00:00Z"),
              },
            },
          },
        },
        boundaryDefinition: "No claim outside the source sequence",
        formationKind: "explicit_import",
        evidenceLinks: [
          {
            role: "support",
            basis: {
              basis: {
                case: "evidence",
                value: {
                  occurrenceId: observationIds[0],
                  locator: { case: "wholeOccurrence", value: true },
                  basisRole: "direct",
                  epistemicRelation: "supports",
                },
              },
            },
          },
        ],
      },
    });
    assert.equal(schema.evidenceLinks.length, 1);
    assert.equal(schema.evidenceLinks[0]?.basis?.basis.case, "evidence");
    if (schema.evidenceLinks[0]?.basis?.basis.case === "evidence")
      assert.equal(
        schema.evidenceLinks[0].basis.basis.value.occurrenceId,
        observationIds[0],
      );
    assert(schema.formedAt?.at);
    assert(schema.createdAt);
    assert.equal(schema.revisionNo, 1);
    assert.equal(schema.parentRevisionId, undefined);
    assert.equal(schema.revisionIntent, undefined);
    assert(schema.evidenceLinks[0]?.createdAt);
    const exactSchema = {
      subjectId,
      expression: {
        operation: "atom",
        cues: [
          {
            cue: { case: "text" as const, value: "Calibration source pattern" },
          },
          {
            cue: {
              case: "reference" as const,
              value: {
                kind: "cognitive_schema_revision",
                value: schema.revisionId,
              },
            },
          },
        ],
        modifiers: { projection: { domains: ["schema"] } },
      },
    };
    for (const constraints of [
      {
        formed: {
          predicate: { case: "point" as const, value: schema.formedAt.at },
        },
      },
      {
        valid: {
          predicate: {
            case: "point" as const,
            value: fromJson(TimestampSchema, "2026-10-03T00:00:00Z"),
          },
        },
      },
      { sourceClassesInclude: ["message"], authority: "subject_cognition" },
    ]) {
      const result = await client.cognition.query({
        ...exactSchema,
        expression: {
          ...exactSchema.expression,
          modifiers: { ...exactSchema.expression.modifiers, constraints },
        },
      });
      assert.equal(
        result.hits.length,
        1,
        JSON.stringify(constraints, (_key: string, value: unknown) =>
          typeof value === "bigint" ? String(value) : value,
        ),
      );
    }
    for (const constraints of [
      { authority: "evidence" },
      { sourceClassesInclude: ["web"] },
      { sourceClassesExclude: ["message"] },
      { cognitiveRoles: ["declarative"] },
      { formationModes: ["grounded"] },
      { modalities: ["image"] },
      { evidenceClasses: ["observed"] },
      {
        valid: {
          predicate: {
            case: "point" as const,
            value: fromJson(TimestampSchema, "2026-10-04T00:00:00Z"),
          },
        },
      },
    ]) {
      const result = await client.cognition.query({
        ...exactSchema,
        expression: {
          ...exactSchema.expression,
          modifiers: { ...exactSchema.expression.modifiers, constraints },
        },
      });
      assert.equal(result.hits.length, 0);
    }
    const nousqlPoint = await client.cognition.query({
      subjectId,
      nousql: `Calibration $return(schema) $time(formed,at="${toJson(TimestampSchema, create(TimestampSchema, schema.formedAt.at))}")`,
    });
    assert.equal(nousqlPoint.hits.length, 1);
    const schemas = await client.concepts.listCognitiveSchemas({
      subjectId,
      page: { pageSize: 1 },
    });
    assert.equal(schemas.items[0]?.schemaId, schema.schemaId);
    const schemaHistory = await client.concepts.listCognitiveSchemaRevisions({
      subjectId,
      object: { kind: "cognitive_schema", value: schema.schemaId },
      page: { pageSize: 1 },
    });
    assert.equal(schemaHistory.items[0]?.revisionId, schema.revisionId);
    assert(schemaHistory.items[0]?.evidenceLinks.length);
    const originalEpisode = episode.currentRevision!;
    const revisedEpisode = await client.memory.reviseEpisode({
      subjectId,
      episodeId: episode.episodeId,
      operationId: randomUUID(),
      expectedObjectEpoch: episode.objectEpoch,
      intent: "reinterpret",
      content: {
        title: "Calibration sequence clarified",
        producerSignatureId: originalEpisode.producerSignatureId,
        experienceTime: originalEpisode.experienceTime,
        boundaryExplanation: originalEpisode.boundaryExplanation,
        members: originalEpisode.members,
        basis: originalEpisode.basis,
      },
    });
    assert(revisedEpisode.episode?.currentRevision);
    assert.equal(
      revisedEpisode.episode.currentRevision.producerSignatureId,
      originalEpisode.producerSignatureId,
    );
    const firstHistory = await client.memory.listEpisodeRevisions({
      subjectId,
      episodeId: episode.episodeId,
      page: { pageSize: 1 },
    });
    assert.equal(firstHistory.items.length, 1);
    assert(firstHistory.nextPageToken);
    const history = [...firstHistory.items];
    let historyToken = firstHistory.nextPageToken;
    while (historyToken) {
      const page = await client.memory.listEpisodeRevisions({
        subjectId,
        episodeId: episode.episodeId,
        page: { pageSize: 1, pageToken: historyToken },
      });
      assert.equal(page.items.length, 1);
      assert(
        history.length < revisedEpisode.episode.currentRevision.revisionNo,
      );
      history.push(...page.items);
      historyToken = page.nextPageToken;
    }
    assert.deepEqual(
      history.map((item) => item.revisionNo),
      Array.from({ length: history.length }, (_, index) => index + 1),
    );
    assert.equal(
      history.at(-1)!.episodeRevisionId,
      revisedEpisode.episode.currentRevision.episodeRevisionId,
    );
    const prior = history.find(
      (item) => item.episodeRevisionId === originalEpisode.episodeRevisionId,
    )!;
    assert.deepEqual(prior.members, originalEpisode.members);
    assert.deepEqual(prior.basis, originalEpisode.basis);
    const extraEpisode = await client.memory.createEpisode({
      subjectId,
      operationId: randomUUID(),
      trackKey: "pagination-smoke",
      content: {
        title: "Alternate calibration track",
        experienceTime: originalEpisode.experienceTime,
        boundaryExplanation: originalEpisode.boundaryExplanation,
        members: originalEpisode.members,
        basis: originalEpisode.basis,
      },
    });
    assert(extraEpisode.episode);
    const firstPage = await client.memory.listEpisodes({
      subjectId,
      status: "accepted",
      page: { pageSize: 1 },
    });
    assert.equal(firstPage.items.length, 1);
    assert(firstPage.nextPageToken);
    const lastPage = await client.memory.listEpisodes({
      subjectId,
      status: "accepted",
      page: { pageSize: 1, pageToken: firstPage.nextPageToken },
    });
    assert.equal(lastPage.items.length, 1);
    assert.equal(lastPage.nextPageToken, "");
    assert.deepEqual(
      [firstPage.items[0]!.episodeId, lastPage.items[0]!.episodeId].sort(),
      [episode.episodeId, extraEpisode.episode.episodeId].sort(),
    );
    for (const item of [...firstPage.items, ...lastPage.items]) {
      assert(item.currentRevision?.members.length);
      assert(item.currentRevision?.basis.length);
    }
    // Real pending cognition fills a complete candidate page before a deterministic withdrawal.
    const waiting = [];
    for (let index = 0; index < 130; index++) {
      const formed = await client.memory.formMemory({
        subjectId,
        operationId: randomUUID(),
        cognitiveRole: "declarative",
        input: {
          formationMode: "grounded",
          groundingOccurrenceId: observationIds[0],
          semanticRole: "fact",
          text: `Calibration fact ${index}`,
          epistemicClass: "observed",
          basis: [
            {
              basis: {
                case: "evidence",
                value: {
                  occurrenceId: observationIds[0],
                  basisRole: "direct",
                  locator: { case: "wholeOccurrence", value: true },
                },
              },
            },
          ],
        },
      });
      waiting.push(formed.revisionId);
    }
    await control({
      command: "prioritize",
      subject: subjectId,
      references: waiting,
    });
    for (const current of [...firstPage.items, ...lastPage.items]) {
      const head = await client.memory.getEpisode({
        subjectId,
        id: current.episodeId,
      });
      await client.memory.withdrawEpisode({
        subjectId,
        operationId: randomUUID(),
        episodeId: head.episode!.episodeId,
        expectedObjectEpoch: head.episode!.objectEpoch,
      });
    }
    const priorCalls = { ...calls };
    const deterministic = await client.cognition.grantMaintenance({
      subjectId,
      maxOperations: 1,
      maxModelCalls: 0,
      maxElapsedMs: 60000,
    });
    assert.equal(deterministic.operationsConsumed, 1);
    assert.equal(deterministic.modelCalls, 0);
    assert.equal(deterministic.results[0]?.kind, "journal_revalidate");
    assert.equal(deterministic.results[0]?.status, "committed");
    assert.equal(
      (await client.memory.getJournal({ subjectId, id: journal.journalId }))
        .journal!.acceptanceState,
      "withdrawn",
    );
    assert.deepEqual(calls, priorCalls);

    const secondSubject = "ffffffff-ffff-4fff-bfff-fffffffffff0";
    await client.subjects.createSubject({
      subjectId: secondSubject,
      operationId: randomUUID(),
      cognitiveSeed: {
        text: "schema_version = 1",
        format: "application/vnd.nous-wave.cognitive-seed+toml;version=1",
      },
      capabilities: { memory: true },
    });
    const nextSession = await client.runtime.openSession({
      subjectId: secondSubject,
    });
    await client.runtime.recordObservation({
      subjectId: secondSubject,
      sessionId: nextSession.sessionId,
      requestId: randomUUID(),
      sourceClass: "message",
      material: {
        case: "inlineText",
        value: { text: "Second Subject experience", mediaType: "text/plain" },
      },
    });
    await control({
      command: "advance",
      subject: secondSubject,
      seconds: 2000,
    });
    await client.configuration.setSystemOverride({
      operationId: randomUUID(),
      path: "maintenance.max_model_calls_per_tick",
      value: fromJson(ValueSchema, 0),
    });
    const scheduler = new SubjectMaintenanceScheduler(
      KernelClient.connect(restarted.endpoint, token),
      models,
    );
    await scheduler.poll(new AbortController().signal);
    assert.equal(
      (
        await client.memory.listEpisodes({
          subjectId: secondSubject,
          status: "accepted",
        })
      ).items.length,
      1,
    );
    assert.deepEqual(calls, priorCalls);

    // A real required role with missing credentials must return its formal diagnostic.
    await client.configuration.setSystemOverride({
      operationId: randomUUID(),
      path: "maintenance.max_model_calls_per_tick",
      value: fromJson(ValueSchema, 1),
    });
    let recoveredCalls = 0;
    recoveryProvider = createServer((request, response) => {
      request.resume();
      request.on("end", () => {
        assert.equal(
          request.headers.authorization,
          "Bearer controlled-maintenance-recovery",
        );
        recoveredCalls++;
        response.setHeader("content-type", "application/json");
        response.end(
          JSON.stringify({
            choices: [
              {
                index: 0,
                finish_reason: "stop",
                message: {
                  role: "assistant",
                  content: JSON.stringify({
                    actions: [{ action: "no_change" }],
                  }),
                },
              },
            ],
          }),
        );
      });
    });
    await new Promise<void>((resolve) =>
      recoveryProvider!.listen(0, "127.0.0.1", resolve),
    );
    const recoveryAddress = recoveryProvider.address();
    assert(recoveryAddress && typeof recoveryAddress !== "string");
    const requiredConfig = modelConfigurationSchema.parse({
      gateway_profiles: {
        missing: {
          base_url: `http://127.0.0.1:${recoveryAddress.port}/v1`,
          credential_env: "NOUS_SMOKE_MISSING_CREDENTIAL",
        },
      },
      model_profiles: {
        chat: {
          gateway: "missing",
          protocol: "openai-chat",
          model: "unavailable",
          capabilities: ["text", "structured_output"],
        },
      },
      execution_profiles: { chat: { model: "chat" } },
      roles: {
        concept_maintenance: { routes: ["chat"], requirement: "required" },
      },
    });
    delete process.env.NOUS_SMOKE_MISSING_CREDENTIAL;
    models = await ModelRuntime.fromConfig(requiredConfig);
    await app.close();
    app = await openCore(restarted.endpoint);
    publicEndpoint = await app.listen({ host: "127.0.0.1", port: 0 });
    client = connectNous(publicEndpoint, token);
    const required = await client.cognition.grantMaintenance({
      subjectId,
      maxOperations: 1,
      maxModelCalls: 1,
      maxElapsedMs: 60000,
    });
    assert.equal(required.operationsConsumed, 1);
    assert.equal(required.modelCalls, 0);
    assert.equal(required.results[0]?.status, "blocked_dependency");
    assert.equal(
      required.results[0]?.errorDetail?.code,
      DomainErrorCode.CAPABILITY_UNAVAILABLE,
    );
    assert.equal(
      required.results[0]?.errorDetail?.context.requirement,
      "required",
    );
    assert.equal(
      required.results[0]?.errorDetail?.context.capability,
      "model.concept_maintenance",
    );
    assert.equal(recoveredCalls, 0);
    process.env.NOUS_SMOKE_MISSING_CREDENTIAL =
      "controlled-maintenance-recovery";
    models = await ModelRuntime.fromConfig(requiredConfig);
    await app.close();
    const recoveredKernel = await control({ command: "restart" });
    assert(recoveredKernel.endpoint);
    app = await openCore(recoveredKernel.endpoint);
    publicEndpoint = await app.listen({ host: "127.0.0.1", port: 0 });
    client = connectNous(publicEndpoint, token);
    const recovered = await client.cognition.grantMaintenance({
      subjectId,
      maxOperations: 1,
      maxModelCalls: 1,
      maxElapsedMs: 60000,
    });
    assert.equal(recovered.operationsConsumed, 1);
    assert.equal(recovered.modelCalls, 1);
    assert.equal(recoveredCalls, 1);
    assert.equal(recovered.results[0]?.needId, required.results[0]?.needId);
    assert.equal(recovered.results[0]?.status, "no_change");
    console.error("Longitudinal smoke passed");
  } finally {
    await app.close();
    delete process.env.NOUS_SMOKE_MISSING_CREDENTIAL;
    if (recoveryProvider) {
      recoveryProvider.closeAllConnections();
      await new Promise<void>((resolve) =>
        recoveryProvider!.close(() => resolve()),
      );
    }
    channel.close();
    process.stdout.write(JSON.stringify({ command: "done" }) + "\n");
    process.stdin.destroy();
  }
}

const endpoint = process.env.NOUS_LONGITUDINAL_ENDPOINT;
if (endpoint) {
  const token = process.env.NOUS_LONGITUDINAL_TOKEN;
  assert(token);
  await scenario(endpoint, token).catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
} else {
  await new Promise<void>((resolve, reject) => {
    const child = spawn(
      "cargo",
      [
        "test",
        "-p",
        "nous-kernel",
        "--test",
        "longitudinal_smoke",
        "--",
        "--nocapture",
      ],
      {
        stdio: "inherit",
        env: { ...process.env, NOUS_LONGITUDINAL_NODE: process.execPath },
        windowsHide: true,
      },
    );
    child.once("error", reject);
    child.once("exit", (code) =>
      code === 0
        ? resolve()
        : reject(new Error(`longitudinal smoke exited ${code}`)),
    );
  });
}
