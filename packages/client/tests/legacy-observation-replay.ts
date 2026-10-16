// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { NousError } from "../src/errors.js";
import { readFile, writeFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { randomUUID, createHash } from "node:crypto";
import {
  fromJson,
  toJson,
  type JsonObject,
  type MessageInitShape,
} from "@bufbuild/protobuf";
import { createGrpcTransport } from "@connectrpc/connect-node";
const [stage, project, endpoint, token, file] = process.argv.slice(
  2,
) as string[];
const moduleAt = (path: string): Promise<unknown> =>
  import(pathToFileURL(resolve(project!, path)).href);
const { createNousClient } = (await moduleAt(
  "packages/client/src/index.ts",
)) as typeof import("../src/index.js");
const { KernelClient } = (await moduleAt(
  "apps/nous-core/src/kernel-client.ts",
)) as typeof import("../../../apps/nous-core/src/kernel-client.js");
const { ObservationInputSchema } = (await moduleAt(
  "packages/protocol-ts/src/generated/nous/wave/v1alpha1/types_pb.ts",
)) as typeof import("@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js");
const { SaveOperationRequestSchema, OperationIdentitySchema } = (await moduleAt(
  "packages/protocol-ts/src/generated/nous/wave/kernel/v1alpha1/model_pb.ts",
)) as typeof import("@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js");
type Fixture = {
  requests: JsonObject[];
  occurrenceIds: string[];
  artifactId: string;
  identity: MessageInitShape<typeof OperationIdentitySchema>;
  proposal: string;
};
const occurredField = ObservationInputSchema.fields.find(
  (field) => field.name === "occurred_time",
);
assert(occurredField?.fieldKind === "message");
const transport = createGrpcTransport({
  baseUrl: endpoint!,
  interceptors: [
    (next) => async (request) => {
      request.header.set("authorization", `Bearer ${token}`);
      return next(request);
    },
  ],
});
const client = createNousClient(transport);
const kernel = new KernelClient(transport);
if (stage === "old") {
  const binary = process.env.NOUS_TIME_LEGACY_KERNEL;
  assert(binary);
  assert.equal(
    createHash("sha256")
      .update(await readFile(binary))
      .digest("hex"),
    "e6434e9aa77f027f4e9cc82d557b1e6a0c2d3f03cfb42ac5e06e0e493cb89f5f",
  );

  assert.equal(
    occurredField.message.typeName,
    "nous.wave.v1alpha1.TemporalExtent",
  );
  const subjectId = randomUUID();
  await client.subjects.createSubject({
    subjectId,
    operationId: randomUUID(),
    capabilities: {
      memory: false,
      selfCognition: false,
      socialCognition: false,
      motivation: false,
    },
    metadata: { fixture: "legacy-domain-time" },
  });
  const contact = new Date().toISOString();
  const common = {
    subjectId,
    sourceClass: "file",
    observedAt: contact,
    occurredTime: { instant: contact },
    context: { fixture: "actual old producer" },
  };
  const first = fromJson(ObservationInputSchema, {
    ...common,
    requestId: randomUUID(),
    inlineText: {
      text: "Legacy actual inline content",
      mediaType: "text/plain",
    },
  });
  const inline = await client.runtime.recordObservation(first);
  assert(inline.artifactId);
  const second = fromJson(ObservationInputSchema, {
    ...common,
    requestId: randomUUID(),
    artifactId: inline.artifactId,
  });
  const artifact = await client.runtime.recordObservation(second);
  assert.notEqual(inline.occurrenceId, artifact.occurrenceId);
  const identity = {
    subjectId,
    owner: "material",
    operationKey: randomUUID(),
    semanticDigest: "known-result-before-domain-upgrade",
  };
  await kernel.modelResults.saveOperation(
    fromJson(SaveOperationRequestSchema, {
      identity,
      context: { cognitiveFormedAt: contact },
      proposal: {
        payloadJson: JSON.stringify({
          rawOutput:
            'Original retained result; do not rewrite {"formedAt":"old"}',
          inputIds: [inline.occurrenceId, artifact.occurrenceId],
        }),
      },
    }),
  );
  const result = await kernel.modelResults.readOperation(identity);
  assert(result.proposal);
  await writeFile(
    file!,
    JSON.stringify({
      subjectId,
      requests: [
        toJson(ObservationInputSchema, first),
        toJson(ObservationInputSchema, second),
      ],
      occurrenceIds: [inline.occurrenceId, artifact.occurrenceId],
      artifactId: inline.artifactId,
      identity,
      proposal: result.proposal.payloadJson,
    }),
  );
} else {
  assert.equal(
    occurredField.message.typeName,
    "nous.wave.v1alpha1.DomainTemporalExtent",
  );
  const fixture = JSON.parse(await readFile(file!, "utf8")) as Fixture;
  for (const [index, legacy] of fixture.requests.entries()) {
    assert(legacy.occurredTime !== undefined);
    const canonical = {
      ...legacy,
      occurredTime: { timeDomainRef: "time:real", extent: legacy.occurredTime },
    };
    const input = fromJson(ObservationInputSchema, canonical);
    const replay = await client.runtime.recordObservation(input);
    assert.equal(replay.occurrenceId, fixture.occurrenceIds[index]);
    assert.equal(replay.artifactId, fixture.artifactId);
    const repeated = await client.runtime.recordObservation(input);
    assert.equal(repeated.occurrenceId, replay.occurrenceId);
  }
  const old = fixture.requests[0];
  assert(old?.occurredTime !== undefined);
  const changed = fromJson(ObservationInputSchema, {
    ...old,
    occurredTime: { timeDomainRef: "time:real", extent: old.occurredTime },
    inlineText: {
      text: "Changed bytes must conflict",
      mediaType: "text/plain",
    },
  });
  await assert.rejects(
    () => client.runtime.recordObservation(changed),
    (error: unknown) =>
      error instanceof NousError &&
      error.domainCode === "OPERATION_ID_CONFLICT",
  );
  const result = await kernel.modelResults.readOperation(fixture.identity);
  assert(result.proposal);
  assert.equal(result.proposal.payloadJson, fixture.proposal);
  assert(result.context?.cognitiveFormedAt);
  assert.equal(result.context.cognitiveFormedAt.timeDomainRef, "time:real");
  console.log(
    JSON.stringify({
      legacyReplay: "PASS",
      inlineOccurrence: fixture.occurrenceIds[0],
      artifactOccurrence: fixture.occurrenceIds[1],
      retainedResult: true,
    }),
  );
}
