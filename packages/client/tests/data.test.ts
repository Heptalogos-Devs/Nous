// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson, toJson, type DescMessage } from "@bufbuild/protobuf";
import { ValueSchema } from "@bufbuild/protobuf/wkt";
import type { Transport } from "@connectrpc/connect";
import { expect, it } from "vitest";
import { createNousClient } from "../src/index.js";
import { messageData } from "@nous-wave/protocol/messages";

it("keeps protocol-looking keys and reference bodies inside JSON values opaque", async () => {
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
  const result = await client.configuration.getConfiguration({
    paths: ["models"],
  });
  expect(toJson(ValueSchema, result.entries[0]!.value!)).toEqual(body);
  expect(result.$typeName).toBe("nous.wave.v1alpha1.ConfigurationSnapshot");
  expect(messageData(result)).toMatchObject({ entries: [{ value: body }] });
});
