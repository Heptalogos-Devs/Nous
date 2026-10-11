// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, isMessage } from "@bufbuild/protobuf";
import {
  ListMemoriesResponseSchema,
  ListRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { ListRequest } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { connectNousInstance } from "@nous-wave/client/node";
import { expect, it, vi } from "vitest";
import { runCli } from "./support/client.js";

it("lists paged Memories through the typed Client request", async () => {
  const list = vi.fn(async (_request: ListRequest) =>
    create(ListMemoriesResponseSchema, { nextPageToken: "next" }),
  );
  const client = {
    memory: { list },
  } as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
  const output = await runCli(
    [
      "memory",
      "list",
      "--page-size",
      "17",
      "--page-token",
      "opaque",
      "--status",
      "active",
      "--run-root",
      "/tmp/nous-cli-memory-tests",
      "--instance-root",
      "/tmp/nous-cli-memory-tests",
      "--subject",
      "33333333-3333-4333-8333-333333333333",
    ],
    vi.fn().mockResolvedValue(client),
  );
  const request = list.mock.calls[0]![0];
  expect(request.subjectId).toBe("33333333-3333-4333-8333-333333333333");
  expect(request.page?.pageSize).toBe(17);
  expect(request.page?.pageToken).toBe("opaque");
  expect(request.status).toBe("active");
  expect(output).toMatchObject({
    schemaVersion: "nous.cli.v2",
    kind: "memory.list",
    data: { nextPageToken: "next" },
  });
  expect(isMessage(list.mock.calls[0]?.[0], ListRequestSchema)).toBe(true);
});
