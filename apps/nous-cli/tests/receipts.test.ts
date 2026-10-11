// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { NousError } from "@nous-wave/client";
import { create } from "@bufbuild/protobuf";
import {
  domainError,
  ErrorCode,
  ErrorOwner,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import type { connectNousInstance } from "@nous-wave/client/node";
import { SubjectSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { mkdtemp, readdir, rm } from "node:fs/promises";
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
import { cliErrorPayload } from "../src/agent.js";

it("preserves an owner rejection and recovery when reading a terminal receipt", async () => {
  const root = await mkdtemp(
    join(workspacePaths.temporary, "typed-rejection-"),
  );
  const operationId = crypto.randomUUID();
  const refusal = new NousError(
    domainError("Different input reused an operation identity", 10, {
      code: ErrorCode.OPERATION_ID_CONFLICT,
      owner: ErrorOwner.SUBJECT,
      recovery: ErrorRecovery.NEW_OPERATION,
      context: { operation_id: operationId },
    }),
  );
  let calls = 0;
  const original = {
    subjects: {
      create: async () => {
        calls++;
        throw refusal;
      },
    },
  } as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
  try {
    const env = await createEnvironment(
      {
        "run-root": root,
        "instance-root": root,
        subject: "22222222-2222-4222-8222-222222222222",
      },
      async () => original,
    );
    await expect(
      env.client.subjects.create({
        subjectId: "22222222-2222-4222-8222-222222222222",
        operationId,
      }),
    ).rejects.toBe(refusal);
    const [namespace] = await readdir(join(root, "consumers"));
    const [file] = await readdir(
      join(root, "consumers", namespace!, "operations"),
    );
    const receiptId = file!.slice(0, -5);
    expect(
      await cliState(root, identity, policy).receipt(receiptId),
    ).toMatchObject({
      status: "rejected",
      error: {
        transportCode: 10,
        code: "OPERATION_ID_CONFLICT",
        owner: "SUBJECT",
        recovery: "NEW_OPERATION",
        context: { operation_id: operationId },
      },
    });
    const recovered: unknown = await env
      .retry(receiptId)
      .catch((error: unknown) => error);
    expect(cliErrorPayload(recovered)).toMatchObject({
      code: "OPERATION_ID_CONFLICT",
      owner: "CLI",
      recovery: "NEW_OPERATION",
      context: { operation_id: operationId },
    });
    expect(calls).toBe(1);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("reclaims definite rejections and finalizes a recovered unknown request rejected by Authority", async () => {
  const root = await mkdtemp(
    join(workspacePaths.temporary, "rejected-receipt-"),
  );
  const refusal = new NousError(
    domainError("Invalid Subject", 3, {
      code: ErrorCode.INVALID_REQUEST,
      owner: ErrorOwner.SUBJECT,
      recovery: ErrorRecovery.CORRECT_REQUEST,
    }),
  );
  let unknown = false;
  const original = {
    subjects: {
      create: async () => {
        if (unknown) {
          unknown = false;
          throw new Error("Response lost");
        }
        throw refusal;
      },
    },
  } as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
  try {
    const env = await createEnvironment(
      {
        "run-root": root,
        "instance-root": root,
        subject: "22222222-2222-4222-8222-222222222222",
      },
      async () => original,
    );
    for (let i = 0; i < policy.receipt_limit + 1; i++)
      await expect(
        env.client.subjects.create({
          subjectId: "22222222-2222-4222-8222-222222222222",
          operationId: crypto.randomUUID(),
        }),
      ).rejects.toBe(refusal);
    const [namespace] = await readdir(join(root, "consumers"));
    const path = join(root, "consumers", namespace!, "operations");
    const files = await readdir(path);
    expect(files).toHaveLength(policy.receipt_limit);
    expect(
      (await cliState(root, identity, policy).receipt(files[0]!.slice(0, -5)))
        .status,
    ).toBe("rejected");
    unknown = true;
    const retryRequest = {
      subjectId: "22222222-2222-4222-8222-222222222222",
      operationId: crypto.randomUUID(),
    };
    let receipt = "";
    try {
      await env.client.subjects.create(retryRequest);
    } catch (error) {
      receipt = (error as { receiptId: string }).receiptId;
    }
    expect(receipt).toBeTruthy();
    expect(
      (await cliState(root, identity, policy).receipt(receipt)).status,
    ).toBe("outcome_unknown");
    await expect(env.retry(receipt)).rejects.toBe(refusal);
    expect(
      (await cliState(root, identity, policy).receipt(receipt)).status,
    ).toBe("rejected");
    await expect(env.retry(receipt)).rejects.toMatchObject({
      code: "INVALID_REQUEST",
    });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("returns a successful operation when presentation fails and reads complete receipts without repeating it", async () => {
  const root = await mkdtemp(
    join(workspacePaths.temporary, "complete-receipt-"),
  );
  const result = {
    subjectId: "33333333-3333-4333-8333-333333333333",
    status: "active",
  };
  let calls = 0;
  const connect = async () =>
    ({
      subjects: {
        create: async (request: { subjectId: string }) => {
          calls++;
          return create(SubjectSchema, {
            ...result,
            subjectId: (request as { subjectId: string }).subjectId,
          });
        },
      },
    }) as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
  try {
    const output = await runCli(
      ["subject", "create", "--run-root", root, "--instance-root", root],
      connect,
      async () => {
        throw new Error("Directory unavailable");
      },
    );
    const response = output as {
      data: { status: string; subjectId: string };
      notices: { code: string }[];
    };
    expect(response.data.status).toBe("active");
    expect(response.data.subjectId).toMatch(/^[0-9a-f-]{36}$/i);
    expect(response.notices[0]?.code).toBe("PRESENTATION_UNAVAILABLE");
    const createdSubjectId = response.data.subjectId;
    const [namespace] = await readdir(join(root, "consumers"));
    const [file] = await readdir(
      join(root, "consumers", namespace!, "operations"),
    );
    const env = await createEnvironment(
      {
        "run-root": root,
        "instance-root": root,
        subject: createdSubjectId,
      },
      connect,
    );
    expect(await env.retry(file!.slice(0, -5))).toMatchObject({
      subjectId: createdSubjectId,
      status: "active",
    });
    expect(calls).toBe(1);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("preserves a known Authority refusal when its terminal receipt exceeds the state budget", async () => {
  const root = await mkdtemp(join(workspacePaths.temporary, "refusal-save-"));
  const refusal = new NousError(
    domainError("Invalid request", 3, {
      code: ErrorCode.INVALID_REQUEST,
      owner: ErrorOwner.SUBJECT,
      recovery: ErrorRecovery.CORRECT_REQUEST,
      context: { payload: "x".repeat(policy.file_max_bytes + 1024) },
    }),
  );
  try {
    const env = await createEnvironment(
      { "run-root": root, "instance-root": root },
      async () =>
        ({
          subjects: {
            create: async () => {
              throw refusal;
            },
          },
        }) as unknown as Awaited<ReturnType<typeof connectNousInstance>>,
    );
    const returned: unknown = await env.client.subjects
      .create({
        subjectId: "22222222-2222-4222-8222-222222222222",
        operationId: crypto.randomUUID(),
      })
      .catch((error: unknown) => error);
    expect(returned === refusal).toBe(true);
    expect(
      typeof (refusal as unknown as { receiptId?: unknown }).receiptId,
    ).toBe("string");
    expect(refusal).toHaveProperty("notices.0.code", "RECEIPT_UNSAVED");
    expect(cliErrorPayload(returned)).toMatchObject({
      code: "INVALID_REQUEST",
      owner: "SUBJECT",
      notices: [{ code: "RECEIPT_UNSAVED" }],
    });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
