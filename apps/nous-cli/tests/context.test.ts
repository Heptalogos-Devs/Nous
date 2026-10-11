// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { it, expect, vi } from "vitest";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runCli } from "./support/client.js";
import type { connectNousInstance } from "@nous-wave/client/node";

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
    const receipt = (failure as { receipt: string }).receipt;
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
  const read = vi
    .fn()
    .mockResolvedValue({ revisionId: revision.value, text: "Prior cognition" });
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
    },
    memory: { revision: read },
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
    expect(read).toHaveBeenCalledWith({
      subjectId: "subject",
      id: revision.value,
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

it("freezes semantic TOML targets and input before a recoverable mutation", async () => {
  const root = await mkdtemp(join(tmpdir(), "nous-cli-toml-"));
  const path = join(root, "tag.toml");
  const tagId = "30000000-0000-4000-8000-000000000002";
  const revise = vi
    .fn()
    .mockRejectedValueOnce(new Error("lost response"))
    .mockResolvedValue({ tagId });
  const get = vi
    .fn()
    .mockResolvedValue({ currentRevisionId: "original-revision" });
  const connect = vi
    .fn()
    .mockResolvedValue({ concepts: { getTag: get, reviseTag: revise } });
  const flags = [
    "--run-root",
    root,
    "--instance-root",
    root,
    "--subject",
    "subject",
  ];
  try {
    await writeFile(
      path,
      `target = "tag:${tagId}"\nlabel = "Release planning"\n`,
    );
    let receipt = "";
    try {
      await runCli(
        ["tag", "revise", "--request-file", path, ...flags],
        connect,
      );
    } catch (error) {
      receipt = (error as { receipt: string }).receipt;
    }
    expect(receipt).toBeTruthy();
    await writeFile(
      path,
      'target = "tag:changed"\nlabel = "Changed after admission"\n',
    );
    await runCli(["retry", receipt, ...flags], connect);
    expect(get).toHaveBeenCalledTimes(1);
    expect(revise.mock.calls[1]).toEqual(revise.mock.calls[0]);
    expect(revise.mock.calls[0]?.[0]).toMatchObject({
      target: { tagId, expectedRevisionId: "original-revision" },
      content: { label: "Release planning" },
    });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("renders Evidence/Resource and mixed hits while preserving each owner continuation", async () => {
  const root = await mkdtemp(join(tmpdir(), "nous-cli-source-results-"));
  const refs = [
    { kind: "memory_revision", value: "30000000-0000-4000-8000-000000000001" },
    { kind: "source_region", value: "30000000-0000-4000-8000-000000000002" },
    { kind: "resource", value: "resource:source" },
  ];
  const sourceRegion = vi.fn().mockResolvedValue({
    sourceRegionId: refs[1]!.value,
    artifactId: "artifact",
  });
  const getResource = vi
    .fn()
    .mockResolvedValue({ resourceRef: refs[2]!.value });
  const readRevision = vi
    .fn()
    .mockResolvedValue({ revisionId: refs[0]!.value });
  const reportUse = vi.fn();
  const query = vi.fn();
  const connect = vi.fn().mockResolvedValue({
    cognition: { query, reportUse },
    material: {
      sourceRegion,
      occurrences: vi.fn().mockResolvedValue({ items: [], truncated: false }),
    },
    memory: { revision: readRevision },
    resources: { get: getResource },
  });
  const flags = [
    "--run-root",
    root,
    "--instance-root",
    root,
    "--subject",
    "subject",
  ];
  try {
    for (const selection of [[refs[1]!], [refs[2]!], refs]) {
      query.mockResolvedValue({
        queryId: "q",
        hits: selection.map((reference) => ({
          reference,
          text: reference.kind,
        })),
        resourceRecords: [{ reference: { entryId: "external-source" } }],
        degradation: [],
      });
      const result = await runCli(
        ["query", "source $return(memory,evidence,resource)", ...flags],
        connect,
      );
      expect(result).toMatchObject({
        data: {
          results: selection.map((ref, index) => ({
            result: `result:${index + 1}`,
            ref,
          })),
          resourceRecords: [{ reference: { entryId: "external-source" } }],
        },
      });
    }
    expect(await runCli(["show", "result:2", ...flags], connect)).toMatchObject(
      { data: { sourceRegionId: refs[1]!.value } },
    );
    await runCli(["show", "result:3", ...flags], connect);
    expect(getResource).toHaveBeenCalledWith({
      subjectId: "subject",
      id: "resource:source",
    });
    await expect(
      runCli(["use", "result:2", ...flags], connect),
    ).rejects.toMatchObject({ code: "INVALID_ARGUMENT" });
    expect(reportUse).not.toHaveBeenCalled();
    await runCli(["show", "result:1", ...flags], connect);
    expect(readRevision).toHaveBeenCalledWith({
      subjectId: "subject",
      id: refs[0]!.value,
    });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("reads a returned closed Session reference without selecting or opening a Session", async () => {
  const root = await mkdtemp(join(tmpdir(), "nous-cli-closed-session-"));
  const sessionId = "30000000-0000-4000-8000-000000000003";
  const read = vi.fn().mockResolvedValue({
    sessionId,
    subjectId: "subject",
    runtimeRevision: 2n,
    closed: true,
    residentRefs: [],
  });
  const open = vi.fn();
  const close = vi.fn();
  const client = {
    cognition: { getSession: read, openSession: open, closeSession: close },
    identity: {
      resolve: vi
        .fn()
        .mockImplementation(
          async (request: { locator: { value: string }; kind: string }) => ({
            status: "BOUND",
            candidates: [
              {
                lexicalRef: request.locator.value,
                canonical: { kind: request.kind, value: sessionId },
              },
            ],
          }),
        ),
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
    const result = await runCli(
      ["session", "show", "session:vanib-jalup-jifuj", ...flags],
      connect,
    );
    expect(result).toMatchObject({
      data: { closed: true, runtimeRevision: 2n },
    });
    expect(read).toHaveBeenCalledWith({ subjectId: "subject", id: sessionId });
    expect(open).not.toHaveBeenCalled();
    await expect(
      runCli(["session", "show", ...flags], connect),
    ).rejects.toThrow("Selected Session");
    await expect(
      runCli(["session", "show", `memory:${sessionId}`, ...flags], connect),
    ).rejects.toMatchObject({ code: "REFERENCE_TYPE_MISMATCH" });
    await expect(
      runCli(
        [
          "session",
          "close",
          "session:vanib-jalup-jifuj",
          ...flags,
          "--session",
          sessionId,
        ],
        connect,
      ),
    ).rejects.toMatchObject({ code: "INVALID_ARGUMENT" });
    expect(close).not.toHaveBeenCalled();
    expect(read).toHaveBeenCalledTimes(1);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
