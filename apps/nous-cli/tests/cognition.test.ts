// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";
import { create } from "@bufbuild/protobuf";
import { PageSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { CliEnvironment } from "../src/runtime.js";
import { typedCognition } from "../src/commands/cognition.js";

describe("shared Cognition CLI operations", () => {
  it("reads exact revisions, pages typed history, and sends stable mutation inputs", async () => {
    const episode = {
      header: {
        objectRef: { kind: "episode", value: "episode-id" },
        currentRevisionRef: {
          kind: "episode_revision",
          value: "head-revision",
        },
        objectEpoch: 7n,
      },
      details: { trackKey: "events" },
      revision: {
        header: {
          revisionRef: { kind: "episode_revision", value: "exact-revision" },
        },
        boundaryExplanation: "A single event",
        basis: [],
      },
    };
    const result = { result: { case: "episode", value: episode } };
    const read = vi.fn().mockResolvedValue(result);
    const list = vi
      .fn()
      .mockResolvedValue({ items: [result], nextPageToken: "next" });
    const history = vi
      .fn()
      .mockResolvedValue({ items: [result], nextPageToken: "older" });
    const mutate = vi.fn().mockResolvedValue({
      outcome: { case: "cognition", value: result },
    });
    const env = {
      subjectId: "subject-id",
      values: {
        "expected-epoch": "12",
        "operation-id": "cli-operation",
        mode: "deep",
      },
      required: (value: string | undefined, name: string) => {
        if (!value) throw new Error(`${name} required`);
        return value;
      },
      resolveReference: async (value: string, kind = "memory") => ({
        kind,
        value,
      }),
      client: { cognition: { read, list, history, mutate } },
    } as unknown as CliEnvironment;
    const reference = {
      kind: "episode_revision" as const,
      value: "exact-revision",
    };
    const page = create(PageSchema, { pageSize: 25, pageToken: "cursor" });

    expect(await typedCognition.read(env, reference)).toBe(episode);
    expect(read.mock.calls[0]?.[0]).toMatchObject({
      subjectId: "subject-id",
      reference: { kind: "episode_revision", value: "exact-revision" },
    });

    await expect(
      typedCognition.list(env, "episode", page, "accepted"),
    ).resolves.toEqual({
      items: [episode],
      nextPageToken: "next",
    });
    expect(list.mock.calls[0]?.[0]).toMatchObject({
      subjectId: "subject-id",
      kind: "episode",
      status: "accepted",
      page: { pageSize: 25, pageToken: "cursor" },
    });

    await expect(
      typedCognition.history(
        env,
        { kind: "episode", value: "episode-id" },
        page,
      ),
    ).resolves.toEqual({ items: [episode], nextPageToken: "older" });
    expect(history.mock.calls[0]?.[0]).toMatchObject({
      subjectId: "subject-id",
      objectRef: { kind: "episode", value: "episode-id" },
      page: { pageSize: 25, pageToken: "cursor" },
    });

    expect(
      await typedCognition.mutate(
        env,
        { kind: "episode", value: "episode-id" },
        "suppress",
        9n,
        "operation-stable",
      ),
    ).toBe(episode);
    expect(mutate.mock.calls[0]?.[0]).toMatchObject({
      operationId: "operation-stable",
      subjectId: "subject-id",
      objectRef: { kind: "episode", value: "episode-id" },
      expectedObjectEpoch: 9n,
      action: { case: "suppress" },
    });

    mutate.mockResolvedValueOnce({
      outcome: {
        case: "cognition",
        value: { result: { case: "memory", value: {} } },
      },
    });
    await typedCognition.mutate(
      env,
      { kind: "memory", value: "memory-id" },
      "accessibility",
      12n,
      "cli-operation",
      "deep",
    );
    expect(mutate.mock.calls[1]?.[0]).toMatchObject({
      operationId: "cli-operation",
      objectRef: { kind: "memory", value: "memory-id" },
      expectedObjectEpoch: 12n,
      action: {
        case: "setAccessibility",
        value: { mode: "deep" },
      },
    });
  });
});
