// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import {
  ReportUseRequestSchema,
  ReportUseResponseSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { mkdtemp, readdir, rm } from "node:fs/promises";
import { join } from "node:path";
import { expect, it, vi } from "vitest";
import type { connectNousInstance } from "@nous-wave/client/node";
import { workspacePaths } from "../../../scripts/workspace.js";
import { createEnvironment } from "./support/client.js";

it("does not forge a mutation status identity for UseEvent receipts", async () => {
  const root = await mkdtemp(join(workspacePaths.temporary, "use-inspect-"));
  const subjectId = "44444444-4444-4444-8444-444444444444";
  const operationStatus = vi.fn();
  const original = {
    cognition: {
      reportUse: vi
        .fn()
        .mockResolvedValue(
          create(ReportUseResponseSchema, { acceptedCount: 1 }),
        ),
    },
    system: { operationStatus },
  } as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
  try {
    const env = await createEnvironment(
      { "run-root": root, "instance-root": root },
      async () => original,
    );
    await env.client.cognition.reportUse(
      create(ReportUseRequestSchema, {
        subjectId,
        consumerRef: "consumer:test",
        events: [{ eventId: crypto.randomUUID(), kind: "use" }],
      }),
    );
    const [namespace] = await readdir(join(root, "consumers"));
    const [file] = await readdir(
      join(root, "consumers", namespace!, "operations"),
    );
    const inspection = await env.inspectOperation(file!.slice(0, -5));
    expect(operationStatus).not.toHaveBeenCalled();
    expect(inspection).toMatchObject({
      command: "cognition.reportUse",
      serverState: "UNKNOWN",
      certainty: "outcome_unknown",
    });
    const unsupportedNotice = (
      inspection as { notices: { code: string; message: string }[] }
    ).notices[0]!;
    expect(unsupportedNotice.code).toBe("STATUS_LOOKUP_UNSUPPORTED");
    expect(unsupportedNotice.message).toContain(
      "has no operation-status lookup for this command",
    );
    expect(inspection).not.toHaveProperty("request");
    expect(inspection).not.toHaveProperty("result");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
