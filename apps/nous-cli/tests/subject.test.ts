// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import {
  type CreateSubjectRequest,
  SubjectSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { mkdtemp, rm } from "node:fs/promises";
import { join } from "node:path";
import { expect, it, vi } from "vitest";
import type { connectNousInstance } from "@nous-wave/client/node";
import { workspacePaths } from "../../../scripts/workspace.js";
import { runCli } from "./support/client.js";
import { CliError } from "../src/agent.js";

it("requires a semantic seed before creating a Subject", async () => {
  const root = await mkdtemp(
    join(workspacePaths.temporary, "subject-defaults-"),
  );
  const createSubject = vi.fn(async (request: CreateSubjectRequest) =>
    create(SubjectSchema, {
      subjectId: request.subjectId ?? "",
      status: "active",
    }),
  );
  try {
    const outcome = await runCli(
      ["subject", "create", "--run-root", root, "--instance-root", root],
      async () =>
        ({ subjects: { create: createSubject } }) as unknown as Awaited<
          ReturnType<typeof connectNousInstance>
        >,
    ).catch((error: unknown) => error);
    expect(outcome).toBeInstanceOf(CliError);
    if (!(outcome instanceof CliError))
      throw new Error("Expected subject create to reject missing seed input");
    expect(outcome.code).toBe("INVALID_ARGUMENT");
    expect(outcome.message).toContain("--request-file");
    expect(createSubject).not.toHaveBeenCalled();
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
