// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { expect, test } from "vitest";
import {
  HOST_BRIDGE_JSON_SCHEMAS,
  decodeCognitionUsage,
  encodeCognitionUsage,
  decodeObservationInput,
  encodeObservationInput,
  timestampFromDate,
  withHostObservationScope,
  withAcceptedContactRoots,
} from "../src/host-bridge.js";
test("public Host bridge schemas use draft 7 for the formal Fastify/Ajv owner", () => {
  for (const schema of Object.values(HOST_BRIDGE_JSON_SCHEMAS)) {
    expect(schema.$schema).toBe("http://json-schema.org/draft-07/schema#");
    expect(JSON.stringify(schema)).not.toContain('"$ref"');
  }
});
test("public codecs preserve canonical scope and reject unknown cognition fields", () => {
  const usage = decodeCognitionUsage(
    encodeCognitionUsage({
      agentInstanceId: "native:run",
      purpose: "subject_deliberation",
      intent: "Read actual shared sources",
      sourceRead: {
        subjectId: "subject:fixture",
        sourceRefs: [{ kind: "occurrence", value: "actual-source" }],
      },
      socialDomain: "technical-study",
      audience: {
        groupEntityRef: "entity:qq:authorized-group",
        channelRef: "channel:qq",
        visibility: "group",
        conversationRef: "conversation:qq:group",
      },
    }),
  );
  expect(usage.sourceRead).toMatchObject({
    subjectId: "subject:fixture",
    sourceRefs: [{ kind: "occurrence", value: "actual-source" }],
  });
  expect(usage.audience).toMatchObject({
    groupEntityRef: "entity:qq:authorized-group",
    channelRef: "channel:qq",
  });
  expect(usage.socialDomain).toBe("technical-study");
  expect(() => decodeCognitionUsage({ inventedAuthority: true })).toThrow();
  const observation = withHostObservationScope(
    {
      subjectId: "subject",
      sessionId: "session",
      requestId: "request",
      sourceClass: "message",
      observedAt: timestampFromDate(new Date()),
      material: {
        case: "inlineText",
        value: { text: "actual contact", mediaType: "text/plain" },
      },
    },
    {
      usage,
      activityRef: "activity:current",
      contactRequestId: "contact:current",
    },
  );
  const decoded = decodeObservationInput(encodeObservationInput(observation));
  expect(decoded.context).toMatchObject({
    activity_ref: "activity:current",
    contact_request_id: "contact:current",
    cognition_usage: encodeCognitionUsage(usage),
  });
});

test("accepted contact roots extend only explicit same-Subject scope using actual receipts", () => {
  const original = {
    sourceRead: {
      subjectId: "s",
      sourceRefs: [{ kind: "material", value: "m" }],
    },
  };
  const actual = withAcceptedContactRoots(original, "s", [
    { occurrenceId: "actual" },
    { occurrenceId: "actual" },
  ]);
  expect(encodeCognitionUsage(actual).sourceRead).toEqual({
    subjectId: "s",
    sourceRefs: [
      { kind: "material", value: "m" },
      { kind: "occurrence", value: "actual" },
    ],
  });
  expect(original.sourceRead.sourceRefs).toHaveLength(1);
  expect(() => withAcceptedContactRoots({}, "s", [])).toThrow(/explicit/);
  expect(() => withAcceptedContactRoots(original, "other", [])).toThrow(
    /same-Subject/,
  );
  expect(() => withAcceptedContactRoots(original, "s", [{}])).toThrow(
    /occurrenceId/,
  );
});
