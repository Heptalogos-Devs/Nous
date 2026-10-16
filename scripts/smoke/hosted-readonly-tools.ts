// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { appendFile } from "node:fs/promises";
import { join } from "node:path";
import { create } from "@bufbuild/protobuf";
import { CognitionUsageSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { createManagedBinding } from "@nous-wave/client/managed";
import {
  hostedReadonlyTools,
  runHostedTool,
} from "../../packages/openclaw/src/readonly-tools.js";
import { prepare, boot, stop, type Boot } from "./support.js";
import { workspaceTemp } from "../workspace.js";
const home = await workspaceTemp("smoke", "hosted-readonly-");
let core: Boot | undefined;
try {
  const locator = await prepare(home);
  await appendFile(
    join(home, "config/nous.toml"),
    '\n[[consumers]]\nconsumer_id="consumer:hosted:readonly"\nmemory="OPTIONAL"\nself_cognition="OPTIONAL"\nsocial_cognition="OPTIONAL"\nruntime="OPTIONAL"\nresource="FORBIDDEN"\nmax_items=16\nmax_text_bytes=65536\nmaterialize=true\n',
  );
  core = await boot(locator);
  const c = core.client;
  const subject = await c.subjects.createSubject({
    operationId: crypto.randomUUID(),
    capabilities: { memory: true, selfCognition: true, socialCognition: true },
  });
  const subjectId = subject.subjectId;
  const occurrence = await c.runtime.recordObservation({
    subjectId,
    requestId: crypto.randomUUID(),
    sourceClass: "message",
    conversationRef: "conversation:synthetic-public",
    actorEntityRef: "entity:synthetic:author",
    material: {
      case: "inlineText",
      value: {
        text: "Synthetic public evidence for a readonly dependency check",
        mediaType: "text/plain",
      },
    },
    admit: false,
  });
  const memory = await c.memory.formMemory({
    subjectId,
    operationId: crypto.randomUUID(),
    cognitiveRole: "declarative",
    input: {
      text: "PUBLIC READ TOOL FACT",
      semanticRole: "task_evidence",
      epistemicClass: "reported",
      formationMode: "grounded",
      groundingOccurrenceId: occurrence.occurrenceId,
      basis: [
        {
          basis: {
            case: "evidence",
            value: {
              occurrenceId: occurrence.occurrenceId,
              locator: { case: "wholeOccurrence", value: true },
              basisRole: "direct",
            },
          },
        },
      ],
      validTime: {},
    },
  });
  const session = await c.runtime.openSession({ subjectId });
  const work = (
    await c.runtime.createWorkContext({
      subjectId,
      operationId: crypto.randomUUID(),
      purpose: "Synthetic readonly history check",
    })
  ).workContext!;
  await c.runtime.setActiveWorkContext({
    subjectId,
    sessionId: session.sessionId,
    workContextId: work.workContextId,
    expectedRuntimeRevision: session.runtimeRevision,
    operationId: crypto.randomUUID(),
  });
  const usage = create(CognitionUsageSchema, {
    agentInstanceId: "native-session:native-run",
    purpose: "deliberation",
    intent: "Read exact public source",
    audience: {
      conversationRef: "conversation:synthetic-public",
      visibility: "public",
    },
    sourceRead: {
      subjectId,
      sourceRefs: [{ kind: "work_context", value: work.workContextId }],
      originConversationRefs: ["conversation:synthetic-public"],
    },
  });
  const tool = hostedReadonlyTools().find(
    (candidate) => candidate.name === "nous_read",
  )!;
  const reference = {
    kind: "memory_revision",
    value: memory.revisionId,
  };
  const identity = {
    version: 1 as const,
    subjectId,
    nousSessionId: session.sessionId,
    invocationId: "native-run:invocation",
    contactRequestId: crypto.randomUUID(),
    native: {
      agentId: "fixture",
      sessionId: "native-session",
      sessionKey: "native:synthetic-public",
      runId: "native-run",
      historyGeneration: "fresh",
    },
    promptDigest: "0".repeat(64),
  };
  const result = await runHostedTool(
    {
      identity,
      execution: {
        toolName: "nous_read",
        toolCallId: "actual-native-call",
        phase: "execute",
      },
      usage,
      workContextId: work.workContextId,
      client: async () => c,
      signal: new AbortController().signal,
      consumerRef: "consumer:hosted:readonly",
    },
    () => tool.execute("actual-native-call", { references: [reference] }),
  );
  assert(JSON.stringify(result).includes("PUBLIC READ TOOL FACT"));
  const binding = createManagedBinding({
    client: () => c,
    subjectId,
    sessionId: session.sessionId,
    consumerId: "consumer:hosted:readonly",
  });
  const admitNext = async (invocationId: string) => {
    const prepared = await binding.prepare({
      usage,
      workContextId: work.workContextId,
      cognitionTokenBudget: 4096,
      signal: new AbortController().signal,
    });
    const applied = binding.apply(
      { text: "native history contains actual readonly result" },
      prepared,
      { render: (base) => base },
    );
    await binding.admit({
      invocationId,
      applied: applied.applied,
      occurredAt: new Date(),
    });
  };
  await admitNext("before-withdrawal");
  await c.memory.withdrawMemory({
    subjectId,
    memoryId: memory.memoryId,
    expectedObjectEpoch: memory.objectEpoch,
    operationId: crypto.randomUUID(),
  });
  await assert.rejects(
    () => admitNext("after-withdrawal"),
    /history|invalidated|reset/i,
  );
  console.log(
    "Hosted readonly real PostgreSQL PASS: actual tool read recorded presented refs; withdrawal of original cognition blocks the next managed model input; model calls=0.",
  );
} catch (error) {
  console.error(`Readonly evidence retained at ${home}`);
  throw error;
} finally {
  await stop(core);
}
