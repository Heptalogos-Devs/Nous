// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { NousError } from "@nous-wave/client";
import { create } from "@bufbuild/protobuf";
import { EmptySchema } from "@bufbuild/protobuf/wkt";
import {
  domainError,
  ErrorCode,
  ErrorOwner,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import type { connectNousInstance } from "@nous-wave/client/node";
import {
  SubjectSchema,
  CognitiveRefSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  CognitionMutationRequestSchema,
  CognitionMutationResponseSchema,
  type CognitionMutationRequest,
} from "@nous-wave/protocol/nous/wave/v1alpha1/cognition_pb.js";
import {
  GetOperationStatusResponseSchema,
  OperationState,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { ErrorOwner as ProtocolErrorOwner } from "@nous-wave/protocol/nous/wave/v1alpha1/errors_pb.js";
import { mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { expect, it, vi } from "vitest";
import { workspacePaths } from "../../../scripts/workspace.js";
import {
  createEnvironment,
  runCli,
  identity,
  policy,
} from "./support/client.js";
import { cliState } from "../src/state.js";

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
    ).toBe("pending");
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
  const requestPath = join(root, "subject.toml");
  await writeFile(
    requestPath,
    [
      "[cognitive_seed]",
      'text = "schema_version = 1"',
      'format = "application/vnd.nous-wave.cognitive-seed+toml;version=1"',
      "",
    ].join("\n"),
  );
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

it("derives unified cognition receipt owners from the target and retries the frozen identity", async () => {
  const root = await mkdtemp(
    join(workspacePaths.temporary, "cognition-receipt-"),
  );
  const subjectId = "22222222-2222-4222-8222-222222222222";
  const memoryId = "33333333-3333-4333-8333-333333333333";
  const schemaId = "44444444-4444-4444-8444-444444444444";
  const memoryOperationId = "memory-operation";
  const conceptOperationId = "concept-operation";
  let loseMemoryResponse = true;
  const mutate = vi.fn(async (request: CognitionMutationRequest) => {
    if (request.operationId === memoryOperationId && loseMemoryResponse) {
      loseMemoryResponse = false;
      throw new Error("Response lost");
    }
    return create(CognitionMutationResponseSchema, {
      outcome: { case: "purged", value: create(EmptySchema) },
    });
  });
  const operationStatus = vi.fn(async () =>
    create(GetOperationStatusResponseSchema, {
      state: OperationState.COMMITTED,
      owner: ProtocolErrorOwner.CONCEPT,
    }),
  );
  const connect = async () =>
    ({
      cognition: { mutate },
      system: { operationStatus },
    }) as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
  const mutation = (kind: string, value: string, operationId: string) =>
    create(CognitionMutationRequestSchema, {
      operationId,
      subjectId,
      objectRef: create(CognitiveRefSchema, { kind, value }),
      expectedObjectEpoch: 3n,
      action: { case: "purge", value: create(EmptySchema) },
    });
  try {
    const env = await createEnvironment(
      {
        "run-root": root,
        "instance-root": root,
        subject: subjectId,
      },
      connect,
    );
    const local = cliState(root, identity, policy);
    const memoryRequest = mutation("memory", memoryId, memoryOperationId);
    const failure: unknown = await env.client.cognition
      .mutate(memoryRequest)
      .catch((error: unknown) => error);
    expect(failure).toMatchObject({
      code: "OUTCOME_UNKNOWN",
      certainty: "outcome_unknown",
    });
    const memoryReceiptId = (failure as { receiptId: string }).receiptId;
    expect(await local.receipt(memoryReceiptId)).toMatchObject({
      command: "cognition.mutate",
      owner: "MEMORY",
      operationKind: "cognition_lifecycle",
      operationId: memoryOperationId,
      status: "pending",
      request: {
        protoJson: { objectRef: { kind: "memory", value: memoryId } },
      },
    });

    await env.retry(memoryReceiptId);
    expect(mutate).toHaveBeenCalledTimes(2);
    expect(mutate.mock.calls.map(([request]) => request.operationId)).toEqual([
      memoryOperationId,
      memoryOperationId,
    ]);
    expect(mutate.mock.calls[1]?.[0]).toEqual(mutate.mock.calls[0]?.[0]);

    const status = await env.inspectOperation(memoryReceiptId);
    expect(operationStatus).toHaveBeenCalledTimes(1);
    expect(status).toMatchObject({
      owner: "MEMORY",
      serverState: "COMMITTED",
      serverOwner: "CONCEPT",
      certainty: "outcome_unknown",
      notices: [{ code: "SERVER_OWNER_MISMATCH" }],
    });

    await env.client.cognition.mutate(
      mutation("cognitive_schema", schemaId, conceptOperationId),
    );
    const [namespace] = await readdir(join(root, "consumers"));
    const operations = join(root, "consumers", namespace!, "operations");
    const conceptFile = (await readdir(operations)).find(
      (name) => name !== `${memoryReceiptId}.json`,
    );
    expect(conceptFile).toBeTruthy();
    expect(await local.receipt(conceptFile!.slice(0, -5))).toMatchObject({
      command: "cognition.mutate",
      owner: "CONCEPT",
      operationId: conceptOperationId,
      status: "complete",
      request: {
        protoJson: { objectRef: { kind: "cognitive_schema", value: schemaId } },
      },
    });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
