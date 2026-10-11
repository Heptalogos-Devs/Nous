// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { createInterface } from "node:readline";
import { timestampFromDate, TimestampSchema } from "@bufbuild/protobuf/wkt";
import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { connectNous } from "@nous-wave/client/node";
import { MaintenancePlanSchema } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { KernelClient } from "../../apps/nous-core/src/kernel-client.js";
import { createCore } from "../../apps/nous-core/src/server.js";
import { ModelRuntime } from "../../apps/nous-core/src/model/runtime.js";
import { modelConfigurationSchema } from "../../apps/nous-core/src/model/configuration.js";
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
  models.invocations.snapshot = (role) => ({
    format: "nous.model.execution",
    implementationDigest: "0".repeat(64),
    role,
    configuration: modelConfigurationSchema.parse({}),
    profileDigest: "a".repeat(64),
    configDigest: "b".repeat(64),
  });
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
  models.segmentEpisode = async (input, _signal, _snapshot, beforeAttempt) => {
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
  models.synthesizeJournal = async (
    input,
    _signal,
    _snapshot,
    beforeAttempt,
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
  models.consolidate = async (input, _signal, _snapshot, beforeAttempt) => {
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
  const { models, calls } = proposalModels();
  let app = await createCore({
    kernel: KernelClient.connect(endpoint, token),
    token,
    consumers: [],
    models,
  });
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
    await client.subjects.create({
      subjectId,
      operationId: randomUUID(),
      cognitiveSeed: {
        text: "schema_version = 1",
        format: "application/vnd.nous-wave.cognitive-seed+toml;version=1",
        provenance: {},
      },
    });
    const session = await client.cognition.openSession({ subjectId });
    const observationIds: string[] = [];
    for (const text of [
      "Calibration plan confirmed.",
      "Calibration measurements collected.",
      "Calibration review completed.",
    ]) {
      const observed = await client.cognition.observe({
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
      await maintain();
    }
    const episodes = (
      await client.memory.listEpisodes({ subjectId, status: "accepted" })
    ).items;
    const journals = (
      await client.memory.listJournals({ subjectId, status: "accepted" })
    ).items;
    const memories = (
      await client.memory.list({ subjectId, status: "accepted" })
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
    assert(calls.episode > 0 && calls.journal > 0 && calls.consolidation > 0);
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
    ];
    await app.close();
    const restarted = await control({ command: "restart" });
    assert(restarted.endpoint);
    app = await createCore({
      kernel: KernelClient.connect(restarted.endpoint, token),
      token,
      consumers: [],
      models,
    });
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
      (await client.memory.revision({ subjectId, id: refs[2]!.value }))
        .revisionId,
      refs[2]!.value,
    );
    const continuation = await client.cognition.createWorkContext({
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
    assert.equal((await client.cognition.reportUse(use)).acceptedCount, 3);
    assert.equal((await client.cognition.reportUse(use)).duplicateCount, 3);
    const before = { ...calls };
    await maintain();
    assert.deepEqual(calls, before);
    const schema = await client.concepts.createSchema({
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
    assert(schema.formedAt);
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
                value: schema.currentRevisionId,
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
          predicate: { case: "point" as const, value: schema.formedAt },
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
      nousql: `Calibration $return(schema) $time(formed,at="${toJson(TimestampSchema, create(TimestampSchema, schema.formedAt))}")`,
    });
    assert.equal(nousqlPoint.hits.length, 1);
    console.error("Longitudinal smoke passed");
  } finally {
    await app.close();
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
