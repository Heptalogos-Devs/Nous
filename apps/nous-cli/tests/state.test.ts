// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { mkdtemp, rm, readdir } from "node:fs/promises";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { join } from "node:path";
import { expect, test } from "vitest";
import { workspacePaths } from "../../../scripts/workspace.js";
import { cliState } from "../src/state.js";
import { identity, policy } from "./support/client.js";
import { repositoryRoot } from "../../../scripts/workspace.js";
import { pathToFileURL } from "node:url";

test("frozen request codecs preserve BigInt separately from arbitrary JSON keys", async () => {
  const root = await mkdtemp(join(workspacePaths.temporary, "consumer-codec-"));
  try {
    const state = cliState(root, identity, policy);
    const request = { expectedRevision: 7n, constraints: { $bigint: "5" } };
    await state.writeReceipt("operation", {
      format: "nous.consumer.operation",
      name: "context.update",
      request,
    });
    expect((await state.receipt("operation")).request).toEqual(request);
    await state.removeReceipt("operation");
    await expect(state.receipt("operation")).rejects.toMatchObject({
      code: "ENOENT",
    });
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
    await state.save(
      { format: "nous.consumer.selection", subjectId: "another" },
      current,
    );
    await expect(
      state.save({ ...current, workContextId: "old-subject-context" }, current),
    ).rejects.toMatchObject({ code: "STATE_CONFLICT" });
    expect(await state.selection()).toEqual({
      format: "nous.consumer.selection",
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
    ).toEqual({ format: "nous.consumer.selection" });
    expect(
      await cliState(
        root,
        { ...identity, instanceId: crypto.randomUUID() },
        policy,
      ).selection(),
    ).toEqual({ format: "nous.consumer.selection" });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("different processes serialize only the shared consumer receipt transaction", async () => {
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
    try { await state.writeReceipt(process.argv[3],{format:'nous.consumer.operation',name:'pending',request:{operationId:process.argv[3]}});console.log('saved'); }
    catch(error) {if(error.code!=='RESOURCE_EXHAUSTED')throw error;console.log(error.code);}`;
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
    ).toHaveLength(2);
    expect(
      results.filter((result) => result.stdout.trim() === "RESOURCE_EXHAUSTED"),
    ).toHaveLength(2);
    const [namespace] = await readdir(join(root, "consumers"));
    expect(
      await readdir(join(root, "consumers", namespace!, "operations")),
    ).toHaveLength(2);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}, 15000);

test("saved request identity rejects replacement input", async () => {
  const root = await mkdtemp(
    join(workspacePaths.temporary, "consumer-request-cas-"),
  );
  try {
    const state = cliState(root, identity, policy);
    const request = {
      format: "nous.consumer.operation" as const,
      name: "context.update",
      request: { operationId: "original", expectedRevision: 7n },
    };
    await state.writeReceipt("operation", request);
    await expect(
      state.writeReceipt("operation", {
        ...request,
        request: { operationId: "replacement" },
      }),
    ).rejects.toMatchObject({ code: "STATE_CONFLICT" });
    expect(await state.receipt("operation")).toEqual(request);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
