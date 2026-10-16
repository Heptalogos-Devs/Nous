// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { join } from "node:path";
import { runCli } from "../../apps/nous-cli/src/commands/index.js";
import { create, clone } from "@bufbuild/protobuf";
import { prepare, boot, stop, type Boot } from "./support.js";
import { workspaceTemp } from "../workspace.js";
import { RevisionBasisSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  SocialContentSchema,
  RelationDegreeSchema,
  type SocialContent,
} from "@nous-wave/protocol/nous/wave/v1alpha1/social_pb.js";
const home = await workspaceTemp("smoke", "social-");
let core: Boot | undefined;
try {
  const locator = await prepare(home);
  core = await boot(locator);
  const c = core.client;
  const subject = await c.subjects.createSubject({
    operationId: crypto.randomUUID(),
    capabilities: { memory: true },
  });
  const subjectId = subject.subjectId;
  assert.equal(subject.capabilities?.socialCognition, false);
  const actor = "entity:social:alice",
    peer = "entity:social:bob";
  const observe = async (who: string, text: string) =>
    c.runtime.recordObservation({
      subjectId,
      requestId: crypto.randomUUID(),
      sourceClass: "conversation",
      actorEntityRef: who,
      conversationRef: "conversation:team",
      material: {
        case: "inlineText",
        value: { text, mediaType: "text/plain" },
      },
      admit: false,
    });
  const a = await observe(
    actor,
    "This task succeeded. In this team we use 灯塔 for the deployment dashboard.",
  );
  const b = await observe(
    peer,
    "For this task I remain cautious about Alice's estimates.",
  );
  const evidence = (occurrenceId: string) =>
    create(RevisionBasisSchema, {
      basis: {
        case: "evidence",
        value: {
          occurrenceId,
          locator: { case: "wholeOccurrence", value: true },
          basisRole: "direct",
        },
      },
    });
  const scope = {
    description: "team task",
    entities: [actor, peer],
    conversation: "conversation:team",
    domain: "deployment",
  };
  const relation = (
    from: string,
    to: string,
    level: string,
    occurrenceId: string,
  ) =>
    create(SocialContentSchema, {
      content: {
        case: "relation",
        value: {
          from,
          to,
          relationType: "trust",
          degree: { degree: { case: "ordinal", value: level } },
          scope,
          statement: `${from} trusts ${to} in deployment with ${level} scope`,
          judgment: {
            state: "supported",
            explanation:
              "Observed bounded task outcome; this does not establish global intimacy.",
          },
          basis: [evidence(occurrenceId)],
          validTime: { timeDomainRef: "time:real", extent: {} },
        },
      },
    });
  const adopt = (content: SocialContent) =>
    c.social.adopt({
      subjectId,
      operationId: crypto.randomUUID(),
      content,
      reason: "explicit owner contract test from actual isolated observation",
    });
  await assert.rejects(adopt(relation(actor, peer, "stable", a.occurrenceId)));
  await c.subjects.provisionCapabilities({
    subjectId,
    operationId: crypto.randomUUID(),
    capabilities: { socialCognition: true },
  });
  const request = {
    subjectId,
    operationId: crypto.randomUUID(),
    content: relation(actor, peer, "stable", a.occurrenceId),
    reason: "task-specific judgment",
  };
  const forward = await c.social.adopt(request);
  assert.deepEqual(await c.social.adopt(request), forward);
  await adopt(relation(peer, actor, "cautious", b.occurrenceId));
  const sourceRead = {
    subjectId,
    sourceRefs: [
      { kind: "occurrence", value: a.occurrenceId },
      { kind: "occurrence", value: b.occurrenceId },
    ],
  };
  const selection = {
    subjectId,
    scope,
    sourceRead,
    maxItems: 128,
    maxBytes: 1048576,
  };
  const pair = await c.social.pair({ selection, from: actor, to: peer });
  assert.equal(pair.forward?.records.length, 1);
  assert.equal(pair.reverse?.records.length, 1);
  assert.notDeepEqual(
    pair.forward?.records[0]?.content,
    pair.reverse?.records[0]?.content,
  );
  const changedForward = await c.social.revise({
    subjectId,
    operationId: crypto.randomUUID(),
    targetRevision: forward.reference,
    expectedObjectEpoch: forward.objectEpoch,
    content: relation(actor, peer, "limited", a.occurrenceId),
    reason: "isolated historical exact source contract",
  });
  const historicalUsage = {
    agentInstanceId: "social-history-smoke",
    purpose: "reflection",
    intent: "Read the earlier task relationship",
    participants: [
      { entityRef: actor, role: "speaker" },
      { entityRef: peer, role: "participant" },
    ],
    audience: {
      conversationRef: "conversation:team",
      recipientEntityRefs: [peer],
      visibility: "group",
    },
    sourceRead,
    socialDomain: "deployment",
  };
  const historical = await c.cognition.query({
    subjectId,
    usage: historicalUsage,
    expression: {
      operation: "atom",
      cues: [
        { cue: { case: "text", value: "deployment trust" } },
        { cue: { case: "reference", value: forward.objectRef! } },
      ],
      modifiers: {
        projection: { domains: ["social_relation"] },
        asOf: forward.recordedAt,
        materialize: true,
      },
    },
  });
  assert.ok(
    historical.hits.some((h) => h.revision?.value === forward.reference?.value),
    "Trusted historical Query authorizes the actual old exact head",
  );
  assert.notEqual(changedForward.reference?.value, forward.reference?.value);
  const denied = await c.social.select({
    ...selection,
    sourceRead: { subjectId, sourceRefs: [] },
  });
  assert.equal(denied.items.length, 0);
  const wrongScene = await c.social.select({
    ...selection,
    scope: { ...scope, conversation: "conversation:other" },
  });
  assert.equal(wrongScene.items.length, 0);
  const other = await c.subjects.createSubject({
    operationId: crypto.randomUUID(),
  });
  await assert.rejects(
    c.social.select({
      ...selection,
      sourceRead: { ...sourceRead, subjectId: other.subjectId },
    }),
  );
  const bad = relation(actor, peer, "stable", a.occurrenceId);
  if (bad.content.case === "relation")
    bad.content.value.scope!.domain = undefined;
  await assert.rejects(adopt(bad));
  const convention = create(SocialContentSchema, {
    content: {
      case: "convention",
      value: {
        expression: "灯塔",
        variants: ["LT"],
        meaning: "deployment dashboard",
        scope,
        pragmaticUse: "team shorthand",
        judgment: {
          state: "hypothesis",
          explanation:
            "Alice explained the usage; no claim of universal shared understanding.",
        },
        basis: [evidence(a.occurrenceId)],
        validTime: { timeDomainRef: "time:real", extent: {} },
      },
    },
  });
  const generated = await c.runtime.recordObservation({
    subjectId,
    requestId: crypto.randomUUID(),
    sourceClass: "generated",
    actorEntityRef: actor,
    material: {
      case: "inlineText",
      value: {
        text: "The Subject repeats its own shorthand; this is no independent confirmation.",
        mediaType: "text/plain",
      },
    },
    admit: false,
  });
  const selfConfirmation = clone(SocialContentSchema, convention);
  if (selfConfirmation.content.case === "convention") {
    selfConfirmation.content.value.expression = "self-confirmed-unsupported";
    selfConfirmation.content.value.judgment!.state = "supported";
    selfConfirmation.content.value.basis = [evidence(generated.occurrenceId)];
  }
  await assert.rejects(
    adopt(selfConfirmation),
    /external interaction evidence/,
  );
  const word = await adopt(convention);
  const resolved = await c.social.resolveConventions({
    selection,
    original: "请检查灯塔",
    consumption: "understanding",
  });
  assert.equal(resolved.original, "请检查灯塔");
  assert.equal(resolved.candidates.length, 1);
  assert.equal(
    (
      await c.social.resolveConventions({
        selection: {
          ...selection,
          scope: { ...scope, conversation: "conversation:other" },
        },
        original: "灯塔",
        consumption: "expression",
      })
    ).candidates.length,
    0,
  );
  const usage = {
    agentInstanceId: "social-owner-smoke",
    purpose: "expression",
    intent: "deployment 灯塔",
    participants: [
      { entityRef: actor, role: "speaker" },
      { entityRef: peer, role: "addressee" },
    ],
    audience: {
      conversationRef: "conversation:team",
      recipientEntityRefs: [peer],
      visibility: "group",
    },
    sourceRead,
    socialDomain: "deployment",
  };
  const recalled = await c.cognition.query({
    subjectId,
    nousql: "灯塔 $return(language_convention)",
    usage,
  });
  assert.ok(
    recalled.hits.some(
      (h) => h.revision?.kind === "language_convention_revision",
    ),
  );
  await assert.rejects(
    c.cognition.query({
      subjectId,
      nousql: "灯塔 $return(language_convention)",
    }),
  );
  const session = await c.runtime.openSession({ subjectId });
  const projected = await c.cognition.buildProjection({
    subjectId,
    sessionId: session.sessionId,
    consumerId: "consumer:smoke:memory-reference",
    usage,
    maxItems: 32,
    maxTextBytes: 8192,
  });
  assert.ok(projected.segments.some((s) => s.domain === "social"));
  const supported = await c.social.addEvidence({
    subjectId,
    operationId: crypto.randomUUID(),
    targetRevision: word.reference,
    expectedObjectEpoch: word.objectEpoch,
    role: "counterexample",
    basis: evidence(b.occurrenceId),
  });
  assert.equal(supported.revisionNo, 1);
  assert.equal(supported.evidence.length, 1);
  assert.ok(supported.objectEpoch > word.objectEpoch);
  const trace = await c.cognition.trace({
    subjectId,
    reference: word.reference,
    maxNodes: 16,
    maxBytes: 65536,
  });
  assert.ok(trace.nodes.some((n) => n.content.case === "socialRecord"));
  const cli = (await runCli([
    "--run-root",
    join(home, "run"),
    "--instance-root",
    join(home, "social-cli"),
    "--subject",
    subjectId,
    "social",
    "list",
    "--page-size",
    "1",
  ])) as { data: { items: unknown[]; nextPageToken: string } };
  assert.equal(cli.data.items.length, 1);
  assert.ok(cli.data.nextPageToken);
  const pages = [];
  let pageToken = "";
  do {
    const page = await c.social.list({
      subjectId,
      page: { pageSize: 1, pageToken },
    });
    pages.push(...page.items);
    pageToken = page.nextPageToken;
  } while (pageToken);
  assert.equal(pages.length, 3);
  const viaCognition = await c.social.select({
    ...selection,
    sourceRead: { subjectId, sourceRefs: [word.reference!] },
  });
  assert.equal(
    viaCognition.items.length,
    3,
    "Authorized exact cognition resolves actual source roots in a read-only view",
  );
  const dependentContent = relation(actor, peer, "growing", a.occurrenceId);
  if (dependentContent.content.case === "relation") {
    dependentContent.content.value.relationType = "familiarity";
    dependentContent.content.value.judgment!.state = "hypothesis";
    dependentContent.content.value.basis = [
      create(RevisionBasisSchema, {
        basis: {
          case: "cognitionDependency",
          value: {
            targetRevision: word.reference!,
            basisRole: "interpretation",
          },
        },
      }),
    ];
  }
  const dependent = await adopt(dependentContent);
  await assert.rejects(
    c.social.addEvidence({
      subjectId,
      operationId: crypto.randomUUID(),
      targetRevision: word.reference,
      expectedObjectEpoch: supported.objectEpoch,
      role: "support",
      basis: create(RevisionBasisSchema, {
        basis: {
          case: "cognitionDependency",
          value: {
            targetRevision: dependent.reference!,
            basisRole: "interpretation",
          },
        },
      }),
    }),
    /cycle/,
  );
  await c.runtime.reportUse({
    subjectId,
    sessionId: session.sessionId,
    consumerRef: "consumer:smoke:memory-reference",
    modelInput: true,
    events: [
      {
        eventId: crypto.randomUUID(),
        reference: dependent.reference,
        kind: "presented",
        occurredAt: {
          seconds: BigInt(Math.floor(Date.now() / 1000)),
          nanos: 0,
        },
        context: { invocation_id: "social-dependency-history" },
      },
    ],
  });
  const withdrawn = await c.social.setLifecycle({
    mutation: {
      subjectId,
      operationId: crypto.randomUUID(),
      objectRef: word.objectRef,
      expectedObjectEpoch: supported.objectEpoch,
    },
    action: "withdraw",
  });
  assert.equal(withdrawn.acceptanceState, "withdrawn");
  assert.equal(
    (
      await c.social.resolveConventions({
        selection,
        original: "灯塔",
        consumption: "understanding",
      })
    ).candidates.length,
    0,
  );
  assert.equal(
    (await c.social.get({ subjectId, id: dependent.objectRef!.value }))
      .integrityState,
    "revalidation_required",
  );
  assert.equal(
    (await c.runtime.getSession({ subjectId, id: session.sessionId }))
      .historyInvalidated,
    true,
  );
  await c.social.purge({
    subjectId,
    operationId: crypto.randomUUID(),
    objectRef: word.objectRef,
    expectedObjectEpoch: withdrawn.objectEpoch,
  });
  await assert.rejects(
    c.social.getRevision({ subjectId, id: word.reference!.value }),
  );
  for (let i = 0; i < 105; i++) {
    const candidate = clone(SocialContentSchema, convention);
    if (candidate.content.case === "convention") {
      candidate.content.value.expression =
        i === 104 ? "capacity-late-social-marker" : `local-term-${i}`;
      candidate.content.value.variants = [];
      candidate.content.value.meaning = `isolated capacity hypothesis ${i}`;
    }
    await adopt(candidate);
  }
  const lateUsage = { ...usage, intent: "capacity-late-social-marker" };
  const late = await c.cognition.query({
    subjectId,
    nousql: "capacity-late-social-marker $return(language_convention)",
    usage: lateUsage,
  });
  assert.ok(
    late.hits.some((h) => h.text?.includes("isolated capacity hypothesis 104")),
    "Shared query reaches scoped candidates beyond management page 100",
  );
  const session2 = await c.runtime.openSession({ subjectId });
  const lateProjection = await c.cognition.buildProjection({
    subjectId,
    sessionId: session2.sessionId,
    consumerId: "consumer:smoke:memory-reference",
    usage: lateUsage,
    maxItems: 32,
    maxTextBytes: 8192,
  });
  assert.ok(
    lateProjection.segments.some((s) =>
      s.text.includes("isolated capacity hypothesis 104"),
    ),
    "Automatic scope selection ranks beyond the first management page",
  );
  const mentor = relation(actor, peer, "stable", a.occurrenceId);
  if (mentor.content.case === "relation") {
    mentor.content.value.relationType = "mentor_of";
    mentor.content.value.degree = undefined;
    mentor.content.value.statement =
      "Alice mentors Bob within the deployment task";
  }
  const mentorRecord = await adopt(mentor);
  const mentored = await c.social.pair({ selection, from: actor, to: peer });
  assert.ok(
    mentored.reverse?.inverseViews.some(
      (v) =>
        v.role === "mentee_of" &&
        v.record?.reference?.value === mentorRecord.reference?.value,
    ),
  );
  const collaboration = relation(peer, actor, "stable", a.occurrenceId);
  if (collaboration.content.case === "relation") {
    collaboration.content.value.relationType = "collaboration";
    collaboration.content.value.degree = create(RelationDegreeSchema, {
      degree: { case: "state", value: "active" },
    });
    collaboration.content.value.symmetric = true;
    collaboration.content.value.statement =
      "Alice and Bob collaborate on deployment";
  }
  const shared = await adopt(collaboration);
  const symmetric = await c.social.pair({ selection, from: actor, to: peer });
  assert.ok(
    symmetric.forward?.records.some(
      (r) => r.reference?.value === shared.reference?.value,
    ),
  );
  assert.ok(
    symmetric.reverse?.records.some(
      (r) => r.reference?.value === shared.reference?.value,
    ),
  );
  await c.subjects.provisionCapabilities({
    subjectId,
    operationId: crypto.randomUUID(),
    capabilities: { selfCognition: true },
  });
  const selfSource = await c.self.adopt({
    subjectId,
    operationId: crypto.randomUUID(),
    reason: "isolated cross-owner provenance contract",
    content: {
      content: {
        case: "facet",
        value: {
          kind: "preference",
          statement: "I tentatively prefer explicit deployment coordination",
          epistemicClass: "inferred",
          scope: { description: "deployment" },
          stance: "tentative",
          basis: [evidence(a.occurrenceId)],
          validTime: { timeDomainRef: "time:real", extent: {} },
        },
      },
    },
  });
  const crossContent = relation(actor, peer, "close", a.occurrenceId);
  if (crossContent.content.case === "relation") {
    crossContent.content.value.relationType = "closeness";
    crossContent.content.value.judgment!.state = "hypothesis";
    crossContent.content.value.basis = [
      create(RevisionBasisSchema, {
        basis: {
          case: "cognitionDependency",
          value: {
            targetRevision: selfSource.reference!,
            basisRole: "interpretation",
          },
        },
      }),
    ];
  }
  const cross = await adopt(crossContent);
  await assert.rejects(
    c.self.addEvidence({
      subjectId,
      operationId: crypto.randomUUID(),
      targetRevision: selfSource.reference,
      expectedObjectEpoch: selfSource.objectEpoch,
      role: "support",
      basis: create(RevisionBasisSchema, {
        basis: {
          case: "cognitionDependency",
          value: {
            targetRevision: cross.reference!,
            basisRole: "interpretation",
          },
        },
      }),
    }),
    /cycle/,
  );
  await c.self.setLifecycle({
    mutation: {
      subjectId,
      operationId: crypto.randomUUID(),
      objectRef: selfSource.objectRef,
      expectedObjectEpoch: selfSource.objectEpoch,
    },
    action: "suppress",
  });
  assert.equal(
    (await c.social.get({ subjectId, id: cross.objectRef!.value }))
      .integrityState,
    "revalidation_required",
  );
  console.log(
    "Social PostgreSQL owner smoke passed: capability, exact replay, directed pair, scope/source denial, typed degree, convention consumption, scoped Query/Projection, evidence roots, trace, pagination, transitive withdrawal/history, purge and capacity beyond 100.",
  );
} finally {
  if (core) await stop(core);
}
