// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  create,
  isMessage,
  toJson,
  type DescMessage,
} from "@bufbuild/protobuf";
import { ValueSchema } from "@bufbuild/protobuf/wkt";
import type { Transport } from "@connectrpc/connect";
import {
  SetSubjectOverrideRequestSchema,
  SetSystemOverrideRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";
import { expect, it, vi } from "vitest";
import { configurationValue, createNousClient } from "../src/index.js";

it("forwards protobuf configuration requests and Values without rewrapping", async () => {
  const unary = vi.fn(
    async (
      method: { name: string; output: DescMessage },
      _signal: AbortSignal | undefined,
      _timeout: number | undefined,
      _headers: HeadersInit | undefined,
      _input: unknown,
    ) => ({ message: create(method.output, {}) as never }),
  );
  const client = createNousClient({ unary } as unknown as Transport);
  const value = configurationValue("true");
  expect(isMessage(value, ValueSchema)).toBe(true);
  expect(toJson(ValueSchema, value)).toBe(true);

  await client.configuration.setSystem(
    create(SetSystemOverrideRequestSchema, {
      operationId: "system-operation",
      path: "maintenance.enabled",
      expectedRevision: 7n,
      value,
    }),
  );
  await client.configuration.setSubject(
    create(SetSubjectOverrideRequestSchema, {
      operationId: "subject-operation",
      subjectId: "33333333-3333-4333-8333-333333333333",
      path: "maintenance.enabled",
      expectedRevision: 8n,
      value,
    }),
  );

  expect(unary.mock.calls.map((call) => call[0].name)).toEqual([
    "SetSystemOverride",
    "SetSubjectOverride",
  ]);
  const systemRequest = unary.mock.calls[0]?.[4];
  const subjectRequest = unary.mock.calls[1]?.[4];
  expect(isMessage(systemRequest, SetSystemOverrideRequestSchema)).toBe(true);
  expect(isMessage(subjectRequest, SetSubjectOverrideRequestSchema)).toBe(true);
  if (
    !isMessage(systemRequest, SetSystemOverrideRequestSchema) ||
    !isMessage(subjectRequest, SetSubjectOverrideRequestSchema)
  )
    throw new Error("Generated configuration requests were not forwarded");
  if (!systemRequest.value || !subjectRequest.value)
    throw new Error("Configuration requests omitted their protobuf Value");
  expect(toJson(ValueSchema, systemRequest.value)).toBe(true);
  expect(toJson(ValueSchema, subjectRequest.value)).toBe(true);
});
