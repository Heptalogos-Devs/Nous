// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { it, expect, vi } from "vitest";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runCli } from "./support/client.js";
import type { connectNousInstance } from "@nous-wave/client/node";
import type { ReadCognitionRequest } from "@nous-wave/protocol/nous/wave/v1alpha1/cognition_pb.js";

it("recovers an unknown context mutation using the exact saved identity and expected revision", async () => {
  const root = await mkdtemp(join(tmpdir(), "nous-cli-context-"));
  const context = {
    workContextId: "context",
    subjectId: "subject",
    revision: 1n,
    purpose: "Review deployment",
    contextText: "old",
    cognitionAnchors: [],
    entityAnchors: [],
    tagAnchors: [],
    unresolvedQuestions: ["Approval?"],
    constraints: {},
    resumeConditions: [],
    budgetSummary: {},
  };
  const updates: unknown[] = [];
  let lost = true;
  const client = {
    cognition: {
      getWorkContext: async () => ({ workContext: context }),
      updateWorkContext: async (request: unknown) => {
        updates.push(request);
        if (lost) {
          lost = false;
          throw new Error("lost RPC response");
        }
        return {
          workContext: { ...context, revision: 2n, contextText: "new task" },
        };
      },
    },
  } as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
  const connect = vi.fn().mockResolvedValue(client);
  const flags = [
    "--run-root",
    root,
    "--instance-root",
    root,
    "--subject",
    "subject",
    "--work-context",
    "context",
  ];
  try {
    let failure: unknown;
    try {
      await runCli(["context", "set", "--text", "new task", ...flags], connect);
    } catch (error) {
      failure = error;
    }
    expect(failure).toMatchObject({
      code: "OUTCOME_UNKNOWN",
    });
    const receipt = (failure as { receiptId: string }).receiptId;
    expect(typeof receipt).toBe("string");
    context.revision = 7n;
    await runCli(["retry", receipt, ...flags], connect);
    expect(updates).toHaveLength(2);
    expect(updates[1]).toEqual(updates[0]);
    expect(updates[1]).toMatchObject({
      expectedRevision: 1n,
      purpose: "Review deployment",
      contextText: "new task",
      unresolvedQuestions: ["Approval?"],
    });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("retains exact query result revisions for continuation and rejects another Subject", async () => {
  const root = await mkdtemp(join(tmpdir(), "nous-cli-result-"));
  const revision = {
    kind: "memory_revision",
    value: "30000000-0000-4000-8000-000000000001",
  };
  const read = vi.fn(async (_request: ReadCognitionRequest) => ({
    result: {
      case: "memory",
      value: {
        header: {
          objectRef: { kind: "memory", value: "mutable" },
          objectEpoch: 1n,
        },
        details: {},
        revision: {
          header: {
            revisionRef: revision,
          },
          details: { text: "Prior cognition" },
        },
      },
    },
  }));
  const client = {
    cognition: {
      query: async () => ({
        queryId: "query",
        status: "complete",
        hits: [
          {
            reference: { kind: "memory", value: "mutable" },
            revision,
            synopsis: "Prior cognition",
          },
        ],
      }),
      read,
    },
  } as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
  const connect = vi.fn().mockResolvedValue(client);
  const flags = [
    "--run-root",
    root,
    "--instance-root",
    root,
    "--subject",
    "subject",
  ];
  try {
    await runCli(["query", "Recall prior cognition", ...flags], connect);
    await runCli(["show", "result:1", ...flags], connect);
    const request = read.mock.calls[0]?.[0];
    expect(request?.subjectId).toBe("subject");
    expect(request?.reference).toMatchObject({
      kind: "memory_revision",
      value: revision.value,
    });
    await expect(
      runCli(
        [
          "show",
          "result:1",
          "--subject",
          "different",
          "--run-root",
          root,
          "--instance-root",
          root,
        ],
        connect,
      ),
    ).rejects.toMatchObject({ code: "RESULT_SUBJECT_MISMATCH" });
    expect(read).toHaveBeenCalledTimes(1);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
