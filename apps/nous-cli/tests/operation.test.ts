// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { TimestampSchema } from "@bufbuild/protobuf/wkt";
import { ErrorOwner } from "@nous-wave/protocol/nous/wave/v1alpha1/errors_pb.js";
import {
  type GetOperationStatusRequest,
  GetOperationStatusResponseSchema,
  OperationState,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import {
  CognitiveRefSchema,
  CreateSubjectRequestSchema,
  ReportUseRequestSchema,
  ReportUseResponseSchema,
  SubjectSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { mkdtemp, readdir, rm } from "node:fs/promises";
import { join } from "node:path";
import { expect, it, vi } from "vitest";
import type { connectNousInstance } from "@nous-wave/client/node";
import { workspacePaths } from "../../../scripts/workspace.js";
import { createEnvironment, identity, policy } from "./support/client.js";
import { cliState } from "../src/state.js";

it("inspects a v2 receipt using its typed owner identity without returning bodies", async () => {
  const root = await mkdtemp(
    join(workspacePaths.temporary, "operation-inspect-"),
  );
  const subjectId = "33333333-3333-4333-8333-333333333333";
  const operationId = crypto.randomUUID();
  const operationStatus = vi.fn(async (_request: GetOperationStatusRequest) =>
    create(GetOperationStatusResponseSchema, {
      state: OperationState.COMMITTED,
      owner: ErrorOwner.SUBJECT,
      subjectId,
      resultReference: create(CognitiveRefSchema, {
        kind: "subject",
        value: subjectId,
      }),
      resultEpoch: 1n,
      recordedAt: create(TimestampSchema, { seconds: 123n }),
    }),
  );
  const original = {
    subjects: {
      create: vi
        .fn()
        .mockResolvedValue(
          create(SubjectSchema, { subjectId, status: "active" }),
        ),
    },
    system: { operationStatus },
  } as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
  try {
    const env = await createEnvironment(
      { "run-root": root, "instance-root": root },
      async () => original,
    );
    await env.client.subjects.create(
      create(CreateSubjectRequestSchema, {
        subjectId,
        operationId,
      }),
    );
    const [namespace] = await readdir(join(root, "consumers"));
    const [file] = await readdir(
      join(root, "consumers", namespace!, "operations"),
    );
    const receiptId = file!.slice(0, -5);
    const inspection = await env.inspectOperation(receiptId);
    const statusRequest = operationStatus.mock.calls[0]![0];
    expect(statusRequest.identity.case).toBe("subjectCreate");
    if (statusRequest.identity.case === "subjectCreate") {
      expect(statusRequest.identity.value.subjectId).toBe(subjectId);
      expect(statusRequest.identity.value.operationId).toBe(operationId);
    }
    expect(inspection).toMatchObject({
      receiptId,
      command: "subjects.create",
      owner: "SUBJECT",
      operationId,
      localStatus: "complete",
      serverState: "COMMITTED",
      certainty: "server_committed",
      resultReference: { kind: "subject", value: subjectId },
      resultEpoch: "1",
    });
    expect(inspection).not.toHaveProperty("request");
    expect(inspection).not.toHaveProperty("result");
    expect(
      await cliState(root, identity, policy).receipt(receiptId),
    ).toHaveProperty("status", "complete");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

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
