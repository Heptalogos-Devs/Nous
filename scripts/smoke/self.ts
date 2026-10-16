// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { prepare, boot, stop, type Boot } from "./support.js";
import { workspaceTemp } from "../workspace.js";
import { create } from "@bufbuild/protobuf";
import { appendFile } from "node:fs/promises";
import { join } from "node:path";
import { RevisionBasisSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { SelfContentSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/self_pb.js";
const home = await workspaceTemp("smoke", "self-");
let core: Boot | undefined;
try {
  const locator = await prepare(home);
  await appendFile(
    join(home, "config/nous.toml"),
    '\n[[consumers]]\nconsumer_id="consumer:self-only"\nmemory="FORBIDDEN"\nself_cognition="REQUIRED"\nruntime="REQUIRED"\nresource="FORBIDDEN"\nmaterialize=true\n',
  );
  core = await boot(locator);
  const c = core.client;
  const subject = await c.subjects.createSubject({
    operationId: crypto.randomUUID(),
    capabilities: { memory: true },
  });
  const subjectId = subject.subjectId;
  const observation = await c.runtime.recordObservation({
    subjectId,
    requestId: crypto.randomUUID(),
    sourceClass: "generation",
    material: {
      case: "inlineText",
      value: {
        text: "我自主选择自称青砚；核对项目事实时先读原始材料，闲聊无需形式化调用工具。",
        mediaType: "text/plain",
      },
    },
    admit: false,
  });
  const evidence = create(RevisionBasisSchema, {
    basis: {
      case: "evidence" as const,
      value: {
        occurrenceId: observation.occurrenceId,
        locator: { case: "wholeOccurrence" as const, value: true },
        basisRole: "direct",
      },
    },
  });
  const facet = create(SelfContentSchema, {
    content: {
      case: "facet",
      value: {
        kind: "identity",
        epistemicClass: "observed",
        statement: "我选择自称青砚。",
        scope: { description: "general" },
        stance: "adopted",
        basis: [evidence],
        validTime: { timeDomainRef: "time:real", extent: {} },
      },
    },
  });
  await assert.rejects(
    c.self.adopt({
      subjectId,
      operationId: crypto.randomUUID(),
      content: facet,
      reason: "actual self choice",
    }),
  );
  await c.subjects.provisionCapabilities({
    subjectId,
    operationId: crypto.randomUUID(),
    capabilities: { selfCognition: true },
  });
  assert.equal(
    (await c.self.list({ subjectId, page: { pageSize: 100 } })).items.length,
    0,
  );
  const seeded = await c.subjects.createSubject({
    operationId: crypto.randomUUID(),
    capabilities: { selfCognition: true },
    cognitiveSeed: {
      format: "text/markdown",
      text: "# Self\nName: 露白\n",
      provenance: { author_role: "creator" },
    },
  });
  const seed = await c.subjects.getCognitiveSeed({
    subjectId: seeded.subjectId,
  });
  assert.equal(seed.seed?.text, "# Self\nName: 露白\n");
  const seededContent = create(SelfContentSchema, {
    content: {
      case: "facet",
      value: {
        kind: "identity",
        epistemicClass: "narrative",
        statement: "采用的初始自称为露白。",
        scope: { description: "general" },
        stance: "adopted",
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
        validTime: { timeDomainRef: "time:real", extent: {} },
      },
    },
  });
  await c.self.adopt({
    subjectId: seeded.subjectId,
    operationId: crypto.randomUUID(),
    content: seededContent,
    reason: "explicit preserved Seed adoption",
  });
  await assert.rejects(
    c.self.adopt({
      subjectId,
      operationId: crypto.randomUUID(),
      content: seededContent,
      reason: "foreign Seed is not owned",
    }),
  );
  const request = {
    subjectId,
    operationId: crypto.randomUUID(),
    content: facet,
    reason: "actual self choice",
  };
  const first = await c.self.adopt(request);
  const replay = await c.self.adopt(request);
  assert.equal(first.reference?.value, replay.reference?.value);
  await assert.rejects(
    c.self.adopt({ ...request, reason: "different request" }),
  );
  const projectionSession = await c.runtime.openSession({ subjectId });
  const projected = await c.cognition.buildProjection({
    subjectId,
    sessionId: projectionSession.sessionId,
    consumerId: "consumer:smoke:cognitive-runtime-episode",
    situationRefs: [first.reference!],
  });
  assert(
    projected.segments.some(
      (s) =>
        s.domain === "self" &&
        s.authority === "subject_cognition" &&
        s.text.includes("青砚"),
    ),
  );
  const preference = await c.self.adopt({
    subjectId,
    operationId: crypto.randomUUID(),
    reason: "explicit contextual self choice",
    content: {
      content: {
        case: "facet",
        value: {
          kind: "preference",
          statement: "核验工作先读原始材料，普通闲聊无需为了形式调用工具。",
          epistemicClass: "observed",
          scope: { description: "仅核验执行者", purposes: ["worker"] },
          stance: "adopted",
          basis: [evidence],
          validTime: { timeDomainRef: "time:real", extent: {} },
        },
      },
    },
  });
  const usage = {
    agentInstanceId: "self-worker",
    sourceRead: {
      subjectId,
      sourceRefs: [{ kind: "occurrence", value: observation.occurrenceId }],
    },
    purpose: "deliberation",
    intent: "继续当前工作并保持自称",
    audience: { conversationRef: "conversation:self", visibility: "internal" },
  };
  const managedInput = {
    subjectId,
    sessionId: projectionSession.sessionId,
    consumerId: "consumer:smoke:cognitive-runtime-episode",
    usage,
  };
  const ungranted = await c.cognition.buildManagedContext({
    projection: { ...managedInput, usage: { ...usage, sourceRead: undefined } },
  });
  assert(
    !ungranted.projection?.segments.some((s) => s.text.includes("青砚")),
    "Self identity still needs actual source permission",
  );
  const automatic = await c.cognition.buildManagedContext({
    projection: managedInput,
  });
  assert(
    automatic.projection?.segments.some(
      (s) =>
        s.domain === "self" &&
        s.usage === "identity" &&
        s.required &&
        s.text.includes("青砚"),
    ),
  );
  const synchronized = await c.cognition.buildManagedContext({
    projection: managedInput,
    knownCursor: automatic.cursor,
  });
  assert.equal(synchronized.kind, "APPEND");
  assert.equal(synchronized.projection?.segments.length, 0);
  assert(
    !automatic.projection?.segments.some(
      (s) =>
        s.usage === "situational_guidance" &&
        s.sourceRefs.some((r) => r.value === preference.reference!.value),
    ),
  );
  const worker = await c.cognition.buildManagedContext({
    projection: {
      ...managedInput,
      usage: {
        ...usage,
        purpose: "worker",
        agentInstanceId: "self-worker-scope",
      },
    },
  });
  assert(
    worker.projection?.segments.some(
      (s) =>
        s.usage === "situational_guidance" &&
        s.sourceRefs.some((r) => r.value === preference.reference!.value),
    ),
  );
  await assert.rejects(
    c.cognition.buildManagedContext({
      projection: { ...managedInput, maxTextBytes: 8 },
    }),
  );
  const selfOnlySession = await c.runtime.openSession({
    subjectId: seeded.subjectId,
  });
  const selfOnly = await c.cognition.buildManagedContext({
    projection: {
      subjectId: seeded.subjectId,
      sessionId: selfOnlySession.sessionId,
      consumerId: "consumer:self-only",
      usage: {
        ...usage,
        agentInstanceId: "self-only",
        sourceRead: {
          subjectId: seeded.subjectId,
          sourceRefs: [
            { kind: "cognitive_seed_version", value: seed.seedVersionId },
          ],
        },
      },
    },
  });
  assert(
    selfOnly.projection?.segments.some(
      (s) => s.required && s.text.includes("露白"),
    ),
  );
  const recalled = await c.cognition.query({
    subjectId: seeded.subjectId,
    nousql: "采用的初始自称为露白 $return(self_facet)",
  });
  assert(
    recalled.hits.some(
      (h) =>
        h.revision?.kind === "self_facet_revision" && h.text?.includes("露白"),
    ),
  );
  const memorySource = await c.memory.formMemory({
    subjectId,
    operationId: crypto.randomUUID(),
    cognitiveRole: "declarative",
    input: {
      formationMode: "grounded",
      groundingOccurrenceId: observation.occurrenceId,
      semanticRole: "reported_self_choice",
      text: "本次输出作出了自称选择。",
      epistemicClass: "observed",
      basis: [evidence],
      validTime: { timeDomainRef: "time:real", extent: {} },
    },
  });
  const dependent = await c.self.adopt({
    subjectId,
    operationId: crypto.randomUUID(),
    reason: "source-backed contextual belief",
    content: {
      content: {
        case: "facet",
        value: {
          kind: "capability_belief",
          statement: "在当前会话中我能选择一个自称。",
          epistemicClass: "inferred",
          scope: { description: "本次选择情境" },
          stance: "tentative",
          basis: [
            {
              basis: {
                case: "cognitionDependency",
                value: {
                  targetRevision: {
                    kind: "memory_revision",
                    value: memorySource.revisionId,
                  },
                  basisRole: "direct",
                },
              },
            },
          ],
          validTime: { timeDomainRef: "time:real", extent: {} },
        },
      },
    },
  });
  await c.memory.suppressMemory({
    subjectId,
    memoryId: memorySource.memoryId,
    operationId: crypto.randomUUID(),
    expectedObjectEpoch: memorySource.objectEpoch,
  });
  assert.equal(
    (await c.self.get({ subjectId, id: dependent.objectRef!.value }))
      .integrityState,
    "revalidation_required",
  );
  const narrative = await c.self.adopt({
    subjectId,
    operationId: crypto.randomUUID(),
    reason: "organize the actual choice",
    content: {
      content: {
        case: "narrative",
        value: {
          theme: "选择自称",
          epistemicClass: "narrative",
          interpretation: "我选择青砚作为当前自称；这不提供虚构的个人来历。",
          sourceRefs: [first.reference!],
          basis: [evidence],
          validTime: { timeDomainRef: "time:real", extent: {} },
        },
      },
    },
  });
  await assert.rejects(
    c.self.purge({
      subjectId,
      operationId: crypto.randomUUID(),
      objectRef: { kind: "self_facet", value: narrative.objectRef!.value },
      expectedObjectEpoch: narrative.objectEpoch,
    }),
  );
  await assert.rejects(
    c.self.addEvidence({
      subjectId,
      operationId: crypto.randomUUID(),
      targetRevision: first.reference,
      expectedObjectEpoch: first.objectEpoch,
      role: "support",
      basis: {
        basis: {
          case: "cognitionDependency",
          value: { targetRevision: narrative.reference, basisRole: "direct" },
        },
      },
    }),
  );
  const corrected = await c.runtime.recordObservation({
    subjectId,
    requestId: crypto.randomUUID(),
    sourceClass: "generation",
    material: {
      case: "inlineText",
      value: {
        text: "考虑新语境后，我决定改用澄砚作为自称。",
        mediaType: "text/plain",
      },
    },
    admit: false,
  });
  const correctedBasis = create(RevisionBasisSchema, {
    basis: {
      case: "evidence" as const,
      value: {
        occurrenceId: corrected.occurrenceId,
        locator: { case: "wholeOccurrence" as const, value: true },
        basisRole: "direct",
      },
    },
  });
  const facetBody =
    facet.content.case === "facet"
      ? facet.content.value
      : assert.fail("expected facet");
  const next = create(SelfContentSchema, {
    content: {
      case: "facet",
      value: {
        ...facetBody,
        statement: "我选择自称澄砚。",
        basis: [correctedBasis],
      },
    },
  });
  const changed = await c.self.revise({
    subjectId,
    operationId: crypto.randomUUID(),
    targetRevision: first.reference,
    expectedObjectEpoch: first.objectEpoch,
    content: next,
    reason: "actual new self choice in a changed situation",
  });
  assert.equal(changed.objectRef?.value, first.objectRef?.value);
  assert.notEqual(changed.reference?.value, first.reference?.value);
  const updatedInput = await c.cognition.buildManagedContext({
    projection: managedInput,
    knownCursor: synchronized.cursor,
  });
  assert.equal(updatedInput.kind, "RESET");
  assert(
    updatedInput.projection?.segments
      .filter((s) => s.usage === "identity")
      .every((s) => s.text.includes("澄砚") && !s.text.includes("青砚")),
  );
  const past = await c.cognition.query({
    subjectId,
    expression: {
      operation: "atom",
      cues: [
        { cue: { case: "text", value: "旧自称" } },
        { cue: { case: "reference", value: first.objectRef! } },
      ],
      modifiers: {
        projection: { domains: ["self_facet"] },
        asRecordedAt: first.recordedAt,
      },
    },
  });
  assert(
    past.hits.some(
      (h) =>
        h.revision?.value === first.reference?.value &&
        h.text?.includes("青砚"),
    ),
  );
  const old = await c.self.getRevision({
    subjectId,
    id: first.reference!.value,
  });
  assert.equal(old.content?.content.case, "facet");
  if (old.content?.content.case === "facet")
    assert.equal(old.content.content.value.statement, "我选择自称青砚。");
  assert.equal(
    (await c.self.get({ subjectId, id: narrative.objectRef!.value }))
      .integrityState,
    "revalidation_required",
  );
  const pendingNarrative = await c.self.get({
    subjectId,
    id: narrative.objectRef!.value,
  });
  const reinterpretation = await c.self.revise({
    subjectId,
    operationId: crypto.randomUUID(),
    targetRevision: narrative.reference,
    expectedObjectEpoch: pendingNarrative.objectEpoch,
    reason: "reinterpret the actual naming history after the new choice",
    content: {
      content: {
        case: "narrative",
        value: {
          theme: "选择自称",
          interpretation:
            "我曾使用青砚，后来选择了澄砚；它们分别是当时和当前的自称。",
          epistemicClass: "narrative",
          sourceRefs: [first.reference!, changed.reference!],
          basis: [correctedBasis],
          validTime: { timeDomainRef: "time:real", extent: {} },
        },
      },
    },
  });
  assert.equal(reinterpretation.integrityState, "valid");
  await assert.rejects(
    c.self.revise({
      subjectId,
      operationId: crypto.randomUUID(),
      targetRevision: first.reference,
      expectedObjectEpoch: first.objectEpoch,
      content: next,
      reason: "stale caller",
    }),
  );
  const supported = await c.self.addEvidence({
    subjectId,
    operationId: crypto.randomUUID(),
    targetRevision: changed.reference,
    expectedObjectEpoch: changed.objectEpoch,
    role: "boundary_case",
    basis: correctedBasis,
  });
  assert.equal(supported.reference?.value, changed.reference?.value);
  assert.equal(supported.evidence.length, 1);
  const suppressed = await c.self.setLifecycle({
    mutation: {
      subjectId,
      operationId: crypto.randomUUID(),
      objectRef: changed.objectRef,
      expectedObjectEpoch: supported.objectEpoch,
    },
    action: "suppress",
  });
  assert.equal(suppressed.suppressionState, "suppressed");
  const restored = await c.self.setLifecycle({
    mutation: {
      subjectId,
      operationId: crypto.randomUUID(),
      objectRef: changed.objectRef,
      expectedObjectEpoch: suppressed.objectEpoch,
    },
    action: "restore",
  });
  await stop(core);
  core = await boot(locator);
  const restarted = core.client;
  assert.equal(
    (await restarted.self.get({ subjectId, id: first.objectRef!.value }))
      .reference?.value,
    changed.reference?.value,
  );
  const purge = {
    subjectId,
    operationId: crypto.randomUUID(),
    objectRef: changed.objectRef,
    expectedObjectEpoch: restored.objectEpoch,
  };
  await restarted.self.purge(purge);
  await restarted.self.purge(purge);
  await assert.rejects(
    restarted.self.getRevision({ subjectId, id: changed.reference!.value }),
  );
  console.log(
    "SELF_OWNER_SMOKE identity=true source=true exact_history=true revision_cas=true evidence=true lifecycle=true purge_replay=true restart=true",
  );
} finally {
  await stop(core);
}
