// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { realpath } from "node:fs/promises";
import { sep } from "node:path";
import { fileURLToPath } from "node:url";

// Compiled again in an independent npm consumer with skipLibCheck disabled.
import {
  createNousClient,
  serviceDescriptors,
  NousError as RootNousError,
  type NousClient,
  type RequestOptions,
} from "@nous-wave/client";
import { connectNousInstance } from "@nous-wave/client/node";
import {
  createManagedBinding,
  eventIdentity,
  type CognitionUsage,
  type ContextRenderer,
} from "@nous-wave/client/managed";
import {
  NousError,
  DomainErrorCode,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import {
  consumerStatePolicySchema,
  type ConsumerStatePolicy,
} from "@nous-wave/client/consumer-policy";
import {
  executionEnvelope,
  executionOpportunitySchema,
  type ExecutionOpportunityPolicy,
} from "@nous-wave/client/execution-policy";

import {
  HOST_BRIDGE_PATHS,
  withHostObservationScope,
  encodeCognitionUsage,
  decodeCognitionUsage,
  encodeObservationInput,
  decodeObservationInput,
  encodeAcceptedObservation,
  decodeAcceptedObservation,
  encodeProjection,
  decodeProjection,
  timestampFromDate,
  HOST_BRIDGE_JSON_SCHEMAS,
  hostPrepareRequestSchema,
  type HostPrepareRequest,
} from "@nous-wave/client/host-bridge";
export const hostCodecs = {
  withHostObservationScope,
  encodeCognitionUsage,
  decodeCognitionUsage,
  encodeObservationInput,
  decodeObservationInput,
  encodeAcceptedObservation,
  decodeAcceptedObservation,
  encodeProjection,
  decodeProjection,
  timestampFromDate,
};
export const hostPaths = HOST_BRIDGE_PATHS;
export const hostSchemas = HOST_BRIDGE_JSON_SCHEMAS;
export const parseHostRequest = (input: unknown): HostPrepareRequest =>
  hostPrepareRequestSchema.parse(input);

export const connect = (runRoot: string) => connectNousInstance({ runRoot });
export const binding = (client: NousClient) =>
  createManagedBinding({
    client: () => client,
    subjectId: "subject-id",
    sessionId: "session-id",
    consumerId: "consumer:host",
  });
export const renderer: ContextRenderer<string> = {
  render(base, segments, usage: CognitionUsage) {
    const invocation: string = usage.agentInstanceId;
    return base + segments.map((segment) => segment.text).join("") + invocation;
  },
};
export const consumerPolicy: ConsumerStatePolicy =
  consumerStatePolicySchema.parse({});
export const executionPolicy: ExecutionOpportunityPolicy =
  executionOpportunitySchema.parse({});
export const envelope = executionEnvelope(executionPolicy);
export const types = {
  createNousClient,
  serviceDescriptors,
  NousError,
  DomainErrorCode,
  ErrorRecovery,
};
export async function inspect(client: NousClient, options: RequestOptions) {
  const status = await client.system.getStatus({}, options);
  const subject = await client.subjects.getSubject(
    { subjectId: "subject-id" },
    options,
  );
  const id: string = subject.subjectId;
  const phase: string | undefined = status.components[0]?.state;
  // @ts-expect-error Generated request types must reject unknown fields.
  await client.subjects.getSubject({ inventedField: "not a protobuf request" });
  // @ts-expect-error Generated response fields must retain their actual type.
  const wrong: number = subject.subjectId;
  return { id, phase, wrong };
}

const expected = JSON.parse(process.env.NOUS_CLIENT_EXPORTS ?? "{}") as Record<
  string,
  string[]
>;
const consumerRoot = await realpath(process.cwd());
for (const [entry, names] of Object.entries(expected)) {
  const specifier = "@nous-wave/client" + (entry === "." ? "" : entry.slice(1));
  const resolved = await realpath(
    fileURLToPath(import.meta.resolve(specifier)),
  );
  assert(resolved.startsWith(consumerRoot + sep), resolved);
  assert(resolved.endsWith(".js"), resolved);
  assert.deepEqual(
    Object.keys((await import(specifier)) as Record<string, unknown>).sort(),
    names,
  );
}
assert.equal(RootNousError, NousError);
assert.equal(consumerPolicy.receipt_limit, 256);
assert.equal(envelope.responseTimeoutMs, 316000);
assert.equal(
  await eventIdentity("host-public-client"),
  await eventIdentity("host-public-client"),
);
if (process.env.NOUS_CLIENT_RUN_ROOT) {
  const client = await connectNousInstance({
    runRoot: process.env.NOUS_CLIENT_RUN_ROOT,
  });
  const status = await client.system.getStatus({});
  assert(status.components.length > 0);
  const subject = await client.subjects.createSubject({
    operationId: crypto.randomUUID(),
    cognitiveSeed: {
      text: "schema_version = 1",
      format: "application/vnd.nous-wave.cognitive-seed+toml;version=1",
    },
  });
  assert(subject.subjectId);
  const loaded = await client.subjects.getSubject({
    subjectId: subject.subjectId,
  });
  assert.equal(loaded.subjectId, subject.subjectId);
  const listed = await client.subjects.listSubjects({});
  assert(listed.items.some((item) => item.subjectId === subject.subjectId));
  console.log(
    JSON.stringify({
      status: "PASS",
      operations: ["status", "createSubject", "getSubject", "listSubjects"],
      subjectId: subject.subjectId,
    }),
  );
}
console.log(
  JSON.stringify({
    artifact: process.env.NOUS_CLIENT_ARTIFACT,
    entries: Object.keys(expected),
    types: "PASS",
    node: process.version,
  }),
);
