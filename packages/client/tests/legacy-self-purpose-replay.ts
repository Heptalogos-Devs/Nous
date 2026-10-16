// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { createGrpcTransport } from "@connectrpc/connect-node";
import type { SavedReflectionProposal } from "../../../apps/nous-core/src/model/subject-reflection.js";
const [stage, project, endpoint, token, file] = process.argv.slice(
  2,
) as string[];
const moduleAt = (path: string): Promise<unknown> =>
  import(pathToFileURL(resolve(project!, path)).href);
const { KernelClient } = (await moduleAt(
  "apps/nous-core/src/kernel-client.ts",
)) as typeof import("../../../apps/nous-core/src/kernel-client.js");
const { compileReflectionActions, applyReflectionActions } = (await moduleAt(
  "apps/nous-core/src/model/subject-reflection-actions.ts",
)) as typeof import("../../../apps/nous-core/src/model/subject-reflection-actions.js");
const { ProducerSignatureSchema, ObservationInputSchema } = (await moduleAt(
  "packages/protocol-ts/src/generated/nous/wave/v1alpha1/types_pb.ts",
)) as typeof import("@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js");
const { SaveOperationRequestSchema } = (await moduleAt(
  "packages/protocol-ts/src/generated/nous/wave/kernel/v1alpha1/model_pb.ts",
)) as typeof import("@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js");
const kernel = new KernelClient(
  createGrpcTransport({
    baseUrl: endpoint!,
    interceptors: [
      (next) => async (request) => {
        request.header.set("authorization", `Bearer ${token}`);
        return next(request);
      },
    ],
  }),
);
type Capture = {
  subjectId: string;
  identity: {
    subjectId: string;
    owner: string;
    operationKey: string;
    semanticDigest: string;
  };
  raw: JsonValue;
  prompt: string;
  ids: string[];
  committedId: string;
  producer: JsonValue;
};
if (stage === "old") {
  const subjectId = randomUUID();
  await kernel.subjects.createSubject({
    subjectId,
    operationId: randomUUID(),
    capabilities: { selfCognition: true },
  });
  const source = await kernel.runtime.recordObservation(
    fromJson(ObservationInputSchema, {
      subjectId,
      requestId: randomUUID(),
      sourceClass: "file",
      inlineText: {
        text: "A bounded execution preference, actually encountered.",
        mediaType: "text/plain",
      },
    }),
  );
  const workId = randomUUID();
  const producer = fromJson(ProducerSignatureSchema, {
    providerClass: "fixture",
    operation: "self_formation_text",
    implementation: "captured-old-owner",
    modelIdentity: "retained-result",
    preprocessingIdentity: "captured-prompt",
    preprocessingRevision: "1",
    configDigest: "capture",
  });
  const output = {
    actions: ["deliberation", "expression"].map((purpose) => ({
      domain: "self",
      scopeKey: "invocation",
      proposal: {
        action: "adopt",
        reason: `Captured ${purpose} decision`,
        content: {
          family: "self_facet",
          kind: "preference",
          statement: `Preference for ${purpose}`,
          stance: "adopted",
          epistemicClass: "reported",
          scope: {
            description: "Known bounded use",
            purposes: [purpose, "worker"],
            bindToInvocation: false,
          },
          exceptions: [],
          basisKeys: ["source"],
          validTime: { kind: "unknown" },
        },
      },
    })),
  };
  const prompt = 'Original prompt {"purposes":["worker"]}';
  const plan = {
    subjectId,
    domains: ["self"],
    basis: [
      [
        "source",
        {
          evidence: {
            occurrenceId: source.occurrenceId,
            wholeOccurrence: true,
            basisRole: "direct",
          },
        },
      ],
    ],
    currentSelf: [],
    currentSocial: [],
    currentMemory: [],
    currentMotivation: [],
    entities: [],
    scopes: [
      [
        "invocation",
        {
          self: { description: "Invocation", purposes: ["worker"] },
          social: {},
        },
      ],
    ],
    sourceKeys: ["source"],
    dependencies: [{ kind: "occurrence", value: source.occurrenceId }],
    input: prompt,
    maxActions: 2,
    degradation: [],
  };
  // The historical module compiles its original schema; the wire is captured below.
  const proposal = {
    plan,
    workId,
    output,
    producer: toJson(ProducerSignatureSchema, producer),
  } as unknown as SavedReflectionProposal;
  const actions = compileReflectionActions(
    proposal.plan,
    proposal.output,
    producer,
    workId,
  );
  const first = await applyReflectionActions(kernel, [actions[0]!], {});
  assert.equal(first[0]!.status, "applied", first[0]!.reason);
  const identity = {
    subjectId,
    owner: "subject_reflection",
    operationKey: workId,
    semanticDigest: "retained-self-purpose-capture",
  };
  await kernel.modelResults.saveOperation(
    fromJson(SaveOperationRequestSchema, {
      identity,
      proposal: { payloadJson: JSON.stringify(proposal) },
      mutationOperations: actions.map((a) => a.operationId),
    }),
  );
  const committed = first[0]!.ownerResult;
  assert.equal(committed.case, "selfRecord");
  await writeFile(
    file!,
    JSON.stringify({
      subjectId,
      identity,
      raw: output,
      prompt,
      ids: actions.map((a) => a.id),
      committedId: committed.value!.reference!.value,
      producer: proposal.producer,
    } satisfies Capture),
  );
} else {
  const capture = JSON.parse(await readFile(file!, "utf8")) as Capture;
  const saved = await kernel.modelResults.readOperation(capture.identity);
  assert(saved.proposal);
  const proposal = JSON.parse(
    saved.proposal.payloadJson,
  ) as SavedReflectionProposal;
  assert.deepEqual(proposal.output, capture.raw);
  assert.equal(proposal.plan.input, capture.prompt);
  assert.deepEqual(proposal.producer, capture.producer);
  const producer = fromJson(ProducerSignatureSchema, proposal.producer);
  const actions = compileReflectionActions(
    proposal.plan,
    proposal.preparation,
    producer,
    proposal.workId,
    proposal.output,
  );
  assert.deepEqual(
    actions.map((a) => a.id),
    capture.ids,
  );
  const applied = await applyReflectionActions(kernel, actions, {});
  assert.deepEqual(
    applied.map((a) => a.status),
    ["applied", "applied"],
  );
  for (const [index, a] of applied.entries()) {
    assert.equal(a.ownerResult.case, "selfRecord");
    const record = a.ownerResult.value!;
    assert("content" in record);
    if (index === 0) assert.equal(record.reference!.value, capture.committedId);
    assert.equal(record.content!.content.case, "facet");
    if (record.content!.content.case === "facet")
      assert.deepEqual(record.content!.content.value.scope!.purposes, [
        index === 0 ? "subject_deliberation" : "subject_rendering",
        "professional_execution",
      ]);
  }
  const again = await applyReflectionActions(kernel, actions, {});
  assert.deepEqual(
    again.map((a) => a.ownerResult),
    applied.map((a) => a.ownerResult),
  );
  console.log("SELF_PURPOSE_CAPTURE_REPLAY_PASS");
}
