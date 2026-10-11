// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  create,
  fromJson,
  isMessage,
  toJson,
  type DescMessage,
} from "@bufbuild/protobuf";
import { ValueSchema } from "@bufbuild/protobuf/wkt";
import type { Transport } from "@connectrpc/connect";
import { expect, it } from "vitest";
import { createNousClient } from "../src/index.js";
import { protocolReferences } from "../src/data.js";
import { ConfigurationSnapshotSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";
import { MemorySchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";

it("returns the generated response and keeps protocol-looking Value JSON opaque", async () => {
  const body = {
    $typeName: "nous.wave.v1alpha1.Subject",
    $unknown: "source content",
    subjectId: "33333333-3333-4333-8333-333333333333",
    nested: {
      $typeName: "body type",
      $unknown: ["memory:33333333-3333-4333-8333-333333333333"],
    },
  };
  const unary = async (method: { output: DescMessage }) => ({
    message: create(method.output, {
      entries: [{ path: "models", value: fromJson(ValueSchema, body) }],
    }),
  });
  const client = createNousClient({ unary } as unknown as Transport);
  const result = await client.configuration.get({ paths: ["models"] });
  expect(isMessage(result, ConfigurationSnapshotSchema)).toBe(true);
  expect(result.entries[0]?.value).toMatchObject({
    $typeName: "google.protobuf.Value",
    kind: { case: "structValue" },
  });
  expect(toJson(ValueSchema, result.entries[0]!.value!)).toEqual(body);
  expect(protocolReferences(result, ConfigurationSnapshotSchema)).toEqual([]);
});

it("reflects references only through declared message fields", () => {
  const id = "33333333-3333-4333-8333-333333333333";
  const body = {
    $typeName: "nous.wave.v1alpha1.CognitiveRef",
    kind: "memory",
    value: id,
  };
  const memory = create(MemorySchema, {
    subjectId: id,
    basis: [
      {
        basis: {
          case: "evidence",
          value: {
            occurrenceId: id,
            locator: { case: "sourceRegionId", value: id },
          },
        },
      },
    ],
  });
  expect(protocolReferences(memory, MemorySchema)).toContainEqual({
    kind: "subject",
    value: id,
  });
  expect(protocolReferences(body, MemorySchema)).toEqual([]);
});
