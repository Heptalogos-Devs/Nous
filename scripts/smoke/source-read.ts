// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { appendFile } from "node:fs/promises";
import { join } from "node:path";
import { create, clone } from "@bufbuild/protobuf";
import {
  CognitionUsageSchema,
  SourceReadScopeSchema,
  CognitiveRefSchema,
  RevisionBasisSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { mcpConnection } from "../../apps/nous-cli/src/mcp-scope.js";
import { prepare, boot, stop, type Boot } from "./support.js";
import { workspaceTemp } from "../workspace.js";
const dump = (value: unknown, _ignored?: unknown) =>
  JSON.stringify(value, (_, v: unknown) =>
    typeof v === "bigint" ? v.toString() : v,
  );
const home = await workspaceTemp("smoke", "source-read-");
let core: Boot | undefined;
try {
  const locator = await prepare(home);
  await appendFile(
    join(home, "config/nous.toml"),
    '\n[[consumers]]\nconsumer_id="consumer:source-read"\nmemory="OPTIONAL"\nself_cognition="OPTIONAL"\nsocial_cognition="OPTIONAL"\nruntime="OPTIONAL"\nresource="FORBIDDEN"\nmax_items=32\nmax_text_bytes=65536\nmaterialize=true\n',
  );
  core = await boot(locator);
  core.process.stderr!.on("data", (chunk: Buffer) => {
    void appendFile(join(home, "diagnostic.stderr"), chunk);
  });
  const c = core.client;
  const subject = await c.subjects.createSubject({
    operationId: crypto.randomUUID(),
    capabilities: { memory: true, selfCognition: true, socialCognition: true },
  });
  const subjectId = subject.subjectId;
  const observe = (
    conversationRef: string,
    text = "Identical deployment evidence bytes",
  ) =>
    c.runtime.recordObservation({
      subjectId,
      requestId: crypto.randomUUID(),
      sourceClass: "message",
      actorEntityRef: "entity:scope:alice",
      conversationRef,
      material: {
        case: "inlineText",
        value: { text, mediaType: "text/plain" },
      },
      admit: false,
    });
  const pub = await observe("conversation:public"),
    priv = await observe("conversation:private");
  const p = await c.material.getOccurrence({ subjectId, id: pub.occurrenceId }),
    q = await c.material.getOccurrence({ subjectId, id: priv.occurrenceId });
  assert.equal(
    p.artifactId,
    q.artifactId,
    "Same bytes must not merge provenance attribution",
  );
  const basis = (occurrenceId: string) =>
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
  const adoptSelf = (occurrenceId: string, statement: string) =>
    c.self.adopt({
      subjectId,
      operationId: crypto.randomUUID(),
      reason: "Actual source-scoped test",
      content: {
        content: {
          case: "facet",
          value: {
            kind: "preference",
            statement,
            stance: "tentative",
            epistemicClass: "inferred",
            scope: {
              description: "deployment",
              conversationRef: "conversation:public",
            },
            basis: [basis(occurrenceId)],
            validTime: {},
          },
        },
      },
    });
  const publicSelf = await adoptSelf(
      pub.occurrenceId,
      "Public deployment preference",
    ),
    privateSelf = await adoptSelf(
      priv.occurrenceId,
      "PRIVATE deployment preference",
    );
  const form = (occurrenceId: string, text: string) =>
    c.memory.formMemory({
      subjectId,
      operationId: crypto.randomUUID(),
      cognitiveRole: "declarative",
      input: {
        text,
        semanticRole: "deployment_fact",
        epistemicClass: "reported",
        formationMode: "grounded",
        groundingOccurrenceId: occurrenceId,
        basis: [basis(occurrenceId)],
        validTime: {},
      },
    });
  const publicMemory = await form(
      pub.occurrenceId,
      "Public deployment evidence fact",
    ),
    privateMemory = await form(
      priv.occurrenceId,
      "PRIVATE deployment evidence fact",
    );
  const adoptConvention = (occurrenceId: string, meaning: string) =>
    c.social.adopt({
      subjectId,
      operationId: crypto.randomUUID(),
      reason: "Actual attributed source",
      content: {
        content: {
          case: "convention",
          value: {
            expression: "deploycode",
            meaning,
            pragmaticUse: "local test",
            scope: {
              description: "public structural scope",
              conversation: "conversation:public",
            },
            judgment: {
              state: "hypothesis",
              explanation: "Actual attributed source only",
            },
            basis: [basis(occurrenceId)],
            validTime: {},
          },
        },
      },
    });
  const publicSocial = await adoptConvention(
      pub.occurrenceId,
      "public dashboard",
    ),
    privateSocial = await adoptConvention(
      priv.occurrenceId,
      "PRIVATE dashboard",
    );
  const usage = create(CognitionUsageSchema, {
    agentInstanceId: "source-read-contract",
    purpose: "deliberation",
    intent: "deployment evidence",
    participants: [{ entityRef: "entity:scope:alice", role: "speaker" }],
    audience: { conversationRef: "conversation:public", visibility: "group" },
    sourceRead: { subjectId, originConversationRefs: ["conversation:public"] },
  });
  const refs = [
    publicSelf.reference!,
    privateSelf.reference!,
    { kind: "memory_revision", value: publicMemory.revisionId },
    { kind: "memory_revision", value: privateMemory.revisionId },
    publicSocial.reference!,
    privateSocial.reference!,
    { kind: "occurrence", value: pub.occurrenceId },
    { kind: "occurrence", value: priv.occurrenceId },
  ];
  const read = await c.cognition.readSources({
    subjectId,
    usage,
    references: refs,
    maxTextBytes: 65536,
  });
  assert.equal(read.items.length, 4, dump(read));
  assert(!dump(read).includes("PRIVATE"));
  assert(read.degradation.some((d) => d.code === "source_read_omission"));
  const exactUsage = clone(CognitionUsageSchema, usage);
  exactUsage.sourceRead = create(SourceReadScopeSchema, {
    subjectId,
    sourceRefs: [{ kind: "occurrence", value: pub.occurrenceId }],
  });
  assert.equal(
    (
      await c.cognition.readSources({
        subjectId,
        usage: exactUsage,
        references: refs,
        maxTextBytes: 65536,
      })
    ).items.length,
    4,
    "Exact public occurrence cannot authorize private same-byte attribution",
  );
  const denied = await c.cognition.readSources({
    subjectId,
    usage: { ...usage, sourceRead: undefined },
    references: refs,
    maxTextBytes: 65536,
  });
  assert.equal(denied.items.length, 0);
  const select = await c.self.select({
    subjectId,
    scope: { description: "current", conversationRef: "conversation:public" },
    sources: [],
    ranked: [publicSelf.reference!, privateSelf.reference!],
    intent: "deployment",
    maxItems: 8,
    maxBytes: 65536,
    sourceRead: usage.sourceRead,
  });
  assert.equal(select.items.length, 1);
  assert.equal(select.items[0]?.reference?.value, publicSelf.reference?.value);
  const selectedSocial = await c.social.select({
    subjectId,
    scope: { description: "current", conversation: "conversation:public" },
    sourceRead: usage.sourceRead,
    maxItems: 8,
    maxBytes: 65536,
  });
  assert.equal(selectedSocial.items.length, 1);
  // Scope filtering precedes candidate rank/item budgets even when private matches dominate.
  for (let i = 0; i < 105; i++)
    await form(
      priv.occurrenceId,
      `PRIVATE deployment evidence ranked decoy ${i}`,
    );
  const found = await c.cognition.query({
    subjectId,
    usage,
    nousql: "deployment evidence $return(memory) $limit(1)",
  });
  assert.equal(
    found.hits.length,
    1,
    JSON.stringify(found, (_, v: unknown) =>
      typeof v === "bigint" ? v.toString() : v,
    ),
  );
  assert(
    !JSON.stringify(found, (_, v: unknown) =>
      typeof v === "bigint" ? v.toString() : v,
    ).includes("PRIVATE"),
  );
  assert.equal(found.hits[0]?.revision?.value, publicMemory.revisionId);
  await assert.rejects(
    c.cognition.query({
      subjectId,
      usage,
      expression: {
        operation: "atom",
        cues: [
          {
            cue: {
              case: "reference",
              value: {
                kind: "memory_revision",
                value: privateMemory.revisionId,
              },
            },
          },
        ],
        modifiers: { projection: { domains: ["memory"] } },
      },
    }),
  );
  const work = await c.runtime.createWorkContext({
    subjectId,
    operationId: crypto.randomUUID(),
    purpose: "Explicit source-scoped task",
    contextText: "WORK_CONTEXT_PRIVATE_BODY",
    cognitionAnchors: [
      { kind: "memory_revision", value: publicMemory.revisionId },
      { kind: "memory_revision", value: privateMemory.revisionId },
    ],
  });
  const workRef = create(CognitiveRefSchema, {
    kind: "work_context",
    value: work.workContext!.workContextId,
  });
  assert.equal(
    (await c.cognition.readSources({ subjectId, usage, references: [workRef] }))
      .items.length,
    0,
  );
  const workUsage = clone(CognitionUsageSchema, usage);
  workUsage.sourceRead = create(SourceReadScopeSchema, {
    subjectId,
    originConversationRefs: usage.sourceRead!.originConversationRefs,
    sourceRefs: [workRef],
  });
  assert(
    (
      await c.cognition.readSources({
        subjectId,
        usage: workUsage,
        references: [workRef],
      })
    ).items[0]?.text.includes("WORK_CONTEXT_PRIVATE_BODY"),
  );
  const session = await c.runtime.openSession({ subjectId });
  await c.runtime.setActiveWorkContext({
    subjectId,
    sessionId: session.sessionId,
    workContextId: work.workContext!.workContextId,
    expectedRuntimeRevision: session.runtimeRevision,
    operationId: crypto.randomUUID(),
  });
  const projection = await c.cognition.buildManagedContext({
    projection: {
      subjectId,
      sessionId: session.sessionId,
      consumerId: "consumer:source-read",
      usage,
      maxItems: 16,
      maxTextBytes: 65536,
    },
  });
  assert(!dump(projection).includes("WORK_CONTEXT_PRIVATE_BODY"));
  assert(!dump(projection).includes("PRIVATE deployment"));
  const explicitProjection = await c.cognition.buildManagedContext({
    projection: {
      subjectId,
      sessionId: session.sessionId,
      consumerId: "consumer:source-read",
      usage: workUsage,
      maxItems: 16,
      maxTextBytes: 65536,
    },
  });
  assert(dump(explicitProjection).includes("WORK_CONTEXT_PRIVATE_BODY"));
  const scoped = mcpConnection(c, false, usage);
  assert.equal(
    (
      await scoped.cognition.readSources({
        subjectId,
        references: [publicSelf.reference!, privateSelf.reference!],
        usage: create(CognitionUsageSchema, {
          agentInstanceId: "model-attempt",
          purpose: "deliberation",
          intent: "cannot grant",
          sourceRead: {
            subjectId,
            originConversationRefs: ["conversation:private"],
          },
        }),
      })
    ).items.length,
    1,
    "Tool arguments cannot grant origins",
  );
  await assert.rejects(async () =>
    scoped.self.getRevision({ subjectId, id: privateSelf.reference!.value }),
  );
  await assert.rejects(async () =>
    scoped.runtime.recordObservation({ subjectId }),
  );
  assert.equal(
    (await c.self.getRevision({ subjectId, id: privateSelf.reference!.value }))
      .content?.content.case,
    "facet",
    "Authenticated management remains available",
  );
  console.log(
    "SourceRead real PostgreSQL smoke passed: same-byte provenance, origin/exact grants, three cognition owners, pre-budget query filtering beyond100 private matches, scoped body read, WorkContext explicit text, MCP non-grant and management preservation.",
  );
} catch (error) {
  console.error(`SourceRead evidence retained at ${home}`);
  throw error;
} finally {
  await stop(core);
}
