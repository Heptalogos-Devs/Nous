// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { mkdtemp, rm, readdir, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { join } from "node:path";
import { expect, test } from "vitest";
import { workspacePaths } from "../../../scripts/workspace.js";
import { cliState } from "../src/state.js";
import { identity, policy } from "./support/client.js";
import { repositoryRoot } from "../../../scripts/workspace.js";
import { pathToFileURL } from "node:url";
import { create, toJson } from "@bufbuild/protobuf";
import { TimestampSchema } from "@bufbuild/protobuf/wkt";
import {
  CreateSubjectRequestSchema,
  SubjectSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { protocolReferences } from "@nous-wave/client/data";
import {
  freezeRequest,
  freezeResult,
  thawResult,
} from "../src/receipt-codec.js";

test("ProtoJSON receipts preserve generated messages and opaque Struct data", async () => {
  const root = await mkdtemp(join(workspacePaths.temporary, "consumer-codec-"));
  const subjectId = "33333333-3333-4333-8333-333333333333";
  try {
    const state = cliState(root, identity, policy);
    const request = create(CreateSubjectRequestSchema, {
      subjectId,
      operationId: "create-once",
      cognitiveSeed: {
        text: "schema_version = 1",
        format: "application/vnd.nous-wave.cognitive-seed+toml;version=1",
      },
    });
    const frozenRequest = freezeRequest("subjects.create", request);
    const result = create(SubjectSchema, {
      subjectId,
      metadata: {
        $typeName: "user JSON",
        $unknown: ["subject:33333333-3333-4333-8333-333333333333"],
        subjectId,
      },
      createdAt: create(TimestampSchema, { seconds: 123n, nanos: 0 }),
    });
    const frozenResult = freezeResult("subjects.create", result);
    await state.writeReceipt("operation", {
      receiptId: "operation",
      command: "subjects.create",
      request: frozenRequest.frozen,
      status: "pending",
      createdAt: "2026-01-01T00:00:00.000Z",
      updatedAt: "2026-01-01T00:00:00.000Z",
    });
    await state.writeReceipt("operation", {
      receiptId: "operation",
      command: "subjects.create",
      request: frozenRequest.frozen,
      status: "complete",
      result: frozenResult,
      createdAt: "2026-01-01T00:00:00.000Z",
      updatedAt: "2026-01-01T00:00:01.000Z",
    });
    const receipt = await state.receipt("operation");
    expect(receipt.status).toBe("complete");
    if (receipt.status !== "complete")
      throw new Error("Completion receipt missing");
    expect(receipt.request.typeName).toBe(CreateSubjectRequestSchema.typeName);
    expect(receipt.request.protoJson).toEqual(frozenRequest.frozen.protoJson);
    expect(thawResult("subjects.create", receipt.result)).toEqual(result);
    const json = toJson(SubjectSchema, result) as Record<string, unknown>;
    expect(json.metadata).toEqual({
      $typeName: "user JSON",
      $unknown: ["subject:33333333-3333-4333-8333-333333333333"],
      subjectId,
    });
    expect(protocolReferences(result, SubjectSchema)).toEqual([
      { kind: "subject", value: subjectId },
    ]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("selection updates preserve independent fields and reject conflicting or foreign Subject state", async () => {
  const root = await mkdtemp(join(workspacePaths.temporary, "consumer-cas-"));
  try {
    const state = cliState(root, identity, policy);
    const baseline = await state.selection();
    await state.save({ ...baseline, subjectId: "subject" }, baseline);
    const selected = await state.selection();
    await Promise.all([
      state.save({ ...selected, sessionId: "session" }, selected),
      state.save({ ...selected, workContextId: "context" }, selected),
    ]);
    expect(await state.selection()).toMatchObject({
      subjectId: "subject",
      sessionId: "session",
      workContextId: "context",
    });
    const current = await state.selection();
    await state.save({ subjectId: "another" }, current);
    await expect(
      state.save({ ...current, workContextId: "old-subject-context" }, current),
    ).rejects.toMatchObject({ code: "STATE_CONFLICT" });
    expect(await state.selection()).toEqual({
      subjectId: "another",
      sessionId: undefined,
      workContextId: undefined,
      lastQuery: undefined,
    });
    expect(
      await cliState(
        root,
        { ...identity, consumer: "consumer:other" },
        policy,
      ).selection(),
    ).toEqual({});
    expect(
      await cliState(
        root,
        { ...identity, instanceId: crypto.randomUUID() },
        policy,
      ).selection(),
    ).toEqual({});
    const namespace = createHash("sha256")
      .update(JSON.stringify([identity.instanceId, identity.consumer]))
      .digest("hex");
    const selectionPath = join(root, "consumers", namespace, "selection.json");
    const encoded = await readFile(selectionPath, "utf8");
    expect(JSON.parse(encoded)).toEqual({ subjectId: "another" });
    expect(await state.selection()).toMatchObject({ subjectId: "another" });

    const incompatible = JSON.stringify({
      format: "nous.consumer.selection",
      subjectId: "another",
    });
    await writeFile(selectionPath, incompatible);
    await expect(state.selection()).rejects.toMatchObject({
      code: "INVALID_ARGUMENT",
    });
    expect(await readFile(selectionPath, "utf8")).toBe(incompatible);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("different processes serialize receipt retention without blocking new requests", async () => {
  const root = await mkdtemp(
    join(workspacePaths.temporary, "consumer-processes-"),
  );
  const module = pathToFileURL(
    join(repositoryRoot, "apps/nous-cli/src/state.ts"),
  ).href;
  const policyModule = pathToFileURL(
    join(repositoryRoot, "packages/client/src/policy/consumer.ts"),
  ).href;
  const worker = `import {cliState} from ${JSON.stringify(module)}; import {consumerStatePolicySchema} from ${JSON.stringify(policyModule)};
    const state=cliState(process.argv[1],JSON.parse(process.argv[2]),consumerStatePolicySchema.parse({receipt_limit:2}));
    try { const id=process.argv[3]; const now=new Date().toISOString(); await state.writeReceipt(id,{receiptId:id,command:'subjects.create',request:{typeName:'request',protoJson:{}},status:'pending',createdAt:now,updatedAt:now});console.log('saved'); }
    catch(error) {console.log(error.code ?? 'failed');}`;
  try {
    const results = await Promise.all(
      [0, 1, 2, 3].map((index) =>
        promisify(execFile)(
          process.execPath,
          [
            "--import",
            "tsx",
            "--input-type=module",
            "-e",
            worker,
            root,
            JSON.stringify(identity),
            "request-" + index,
          ],
          { cwd: repositoryRoot, windowsHide: true },
        ),
      ),
    );
    expect(
      results.filter((result) => result.stdout.trim() === "saved"),
    ).toHaveLength(4);
    const [namespace] = await readdir(join(root, "consumers"));
    expect(
      await readdir(join(root, "consumers", namespace!, "operations")),
    ).toHaveLength(2);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}, 15000);

test("receipt transitions preserve the frozen request and terminal outcomes", async () => {
  const root = await mkdtemp(
    join(workspacePaths.temporary, "consumer-receipt-cas-"),
  );
  const subjectId = "33333333-3333-4333-8333-333333333333";
  const now = "2026-01-01T00:00:00.000Z";
  try {
    const state = cliState(root, identity, policy);
    const pending = {
      receiptId: "operation",
      command: "subjects.create",
      request: { typeName: "request", protoJson: { subjectId } },
      status: "pending" as const,
      createdAt: now,
      updatedAt: now,
    };
    await state.writeReceipt("operation", pending);
    await expect(
      state.writeReceipt("operation", {
        ...pending,
        request: {
          ...pending.request,
          protoJson: { subjectId: "replacement" },
        },
      }),
    ).rejects.toMatchObject({ code: "STATE_CONFLICT" });
    const complete = {
      ...pending,
      status: "complete" as const,
      result: { typeName: "response", protoJson: { subjectId } },
      updatedAt: "2026-01-01T00:00:01.000Z",
    };
    await state.writeReceipt("operation", complete);
    await expect(
      state.writeReceipt("operation", pending),
    ).rejects.toMatchObject({
      code: "STATE_CONFLICT",
    });
    expect(await state.receipt("operation")).toEqual(complete);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
