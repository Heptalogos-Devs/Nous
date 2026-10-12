// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import type { connectNousInstance } from "@nous-wave/client/node";
import { mkdtemp, rm, readdir } from "node:fs/promises";
import { join } from "node:path";
import { expect, it } from "vitest";
import { workspacePaths } from "../../../scripts/workspace.js";
import {
  createEnvironment,
  runCli,
  identity,
  policy,
} from "./support/client.js";
import { cliState } from "../src/state.js";

it("retries only the frozen request through its owner and removes it after acknowledgement", async () => {
  const root = await mkdtemp(join(workspacePaths.temporary, "frozen-retry-"));
  const requests: unknown[] = [];
  const result = {
    subjectId: "33333333-3333-4333-8333-333333333333",
    status: "active",
  };
  let lost = true;
  const connect = async () =>
    ({
      subjects: {
        createSubject: async (request: unknown) => {
          requests.push(request);
          if (lost) {
            lost = false;
            throw new Error("response lost");
          }
          return result;
        },
      },
    }) as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
  try {
    const env = await createEnvironment(
      { "run-root": root, "instance-root": root },
      connect,
    );
    const request = { operationId: crypto.randomUUID() };
    const failure = (await env.client.subjects
      .createSubject(request)
      .catch((error: unknown) => error)) as { code: string; receipt: string };
    expect(failure.code).toBe("OUTCOME_UNKNOWN");
    expect(
      (await cliState(root, identity, policy).receipt(failure.receipt)).request,
    ).toEqual(request);
    expect(await env.retry(failure.receipt)).toEqual(result);
    expect(requests).toEqual([request, request]);
    await expect(
      cliState(root, identity, policy).receipt(failure.receipt),
    ).rejects.toMatchObject({ code: "ENOENT" });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("keeps owner success when presentation fails without caching a second outcome", async () => {
  const root = await mkdtemp(
    join(workspacePaths.temporary, "presentation-failure-"),
  );
  const result = {
    subjectId: "33333333-3333-4333-8333-333333333333",
    status: "active",
  };
  const connect = async () =>
    ({ subjects: { createSubject: async () => result } }) as unknown as Awaited<
      ReturnType<typeof connectNousInstance>
    >;
  try {
    expect(
      await runCli(
        ["subject", "create", "--run-root", root, "--instance-root", root],
        connect,
        async () => {
          throw new Error("directory unavailable");
        },
      ),
    ).toMatchObject({
      data: result,
      notices: [{ code: "PRESENTATION_UNAVAILABLE" }],
    });
    const [namespace] = await readdir(join(root, "consumers"));
    expect(
      await readdir(join(root, "consumers", namespace!, "operations")),
    ).toEqual([]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
