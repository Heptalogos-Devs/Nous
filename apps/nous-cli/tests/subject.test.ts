// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import {
  type CreateSubjectRequest,
  SubjectSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
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

it("parses explicit Subject seed, provenance, metadata and capabilities from TOML", async () => {
  const root = await mkdtemp(join(workspacePaths.temporary, "subject-input-"));
  const requestPath = join(root, "subject.toml");
  await writeFile(
    requestPath,
    [
      "[cognitive_seed]",
      'text = "schema_version = 1"',
      'format = "application/vnd.nous-wave.cognitive-seed+toml;version=1"',
      "[cognitive_seed.provenance]",
      'source = "approved-import"',
      "[metadata]",
      'purpose = "project memory"',
      "[capabilities]",
      "memory = false",
      "",
    ].join("\n"),
  );
  const createSubject = vi.fn(async (request: CreateSubjectRequest) =>
    create(SubjectSchema, {
      subjectId: request.subjectId ?? "",
      status: "active",
    }),
  );
  try {
    await runCli(
      [
        "subject",
        "create",
        "--request-file",
        requestPath,
        "--run-root",
        root,
        "--instance-root",
        root,
      ],
      async () =>
        ({ subjects: { create: createSubject } }) as unknown as Awaited<
          ReturnType<typeof connectNousInstance>
        >,
    );
    const request = createSubject.mock.calls[0]![0];
    expect(request.subjectId).toMatch(/^[0-9a-f-]{36}$/i);
    expect(request.operationId).toMatch(/^[0-9a-f-]{36}$/i);
    expect(request.cognitiveSeed?.text).toBe("schema_version = 1");
    expect(request.cognitiveSeed?.format).toBe(
      "application/vnd.nous-wave.cognitive-seed+toml;version=1",
    );
    expect(request.cognitiveSeed?.provenance).toEqual({
      source: "approved-import",
    });
    expect(request.metadata).toEqual({ purpose: "project memory" });
    expect(request.capabilities?.memory).toBe(false);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
