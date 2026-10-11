// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";
import { NousError } from "@nous-wave/client";
import {
  domainError,
  DomainErrorCode,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import type { connectNousInstance } from "@nous-wave/client/node";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { runCli } from "./support/client.js";
import { cliErrorPayload } from "../src/agent.js";
import { workspaceTemp } from "../../../scripts/workspace.js";

const exec = promisify(execFile);
it("invalidates result indices when the next query fails", async () => {
  const root = await workspaceTemp("tests", "cli-failed-query-");
  const query = vi
    .fn()
    .mockResolvedValueOnce({
      queryId: "first",
      hits: [
        {
          reference: {
            kind: "memory_revision",
            value: "33333333-3333-4333-8333-333333333333",
          },
          text: "old result",
        },
      ],
    })
    .mockResolvedValueOnce({
      queryId: "second",
      hits: [
        {
          reference: {
            kind: "memory_revision",
            value: "33333333-3333-4333-8333-333333333333",
          },
          text: "new result",
        },
      ],
    })
    .mockRejectedValueOnce(new Error("query rejected"));
  const revision = vi.fn();
  const reportUse = vi.fn().mockResolvedValue({ acceptedCount: 1 });
  const client = {
    cognition: { query, reportUse },
    memory: { revision },
  } as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
  const connect = vi.fn().mockResolvedValue(client);
  const globals = [
    "--run-root",
    "/tmp/nous",
    "--instance-root",
    root,
    "--subject",
    "s",
  ];
  await runCli(["query", "first", ...globals], connect);
  await runCli(
    [
      "use",
      "memory_revision:33333333-3333-4333-8333-333333333333",
      "--query-id",
      "query:last",
      ...globals,
    ],
    connect,
  );
  expect(reportUse).toHaveBeenCalledWith(
    expect.objectContaining({
      events: [expect.objectContaining({ queryId: "first" })],
    }),
  );
  const second = await runCli(["query", "second", ...globals], connect);
  expect(second).not.toHaveProperty("notices");
  await runCli(["use", "result:1", ...globals], connect);
  expect(reportUse).toHaveBeenLastCalledWith(
    expect.objectContaining({
      events: [expect.objectContaining({ queryId: "second" })],
    }),
  );
  await expect(
    runCli(["query", "invalid", ...globals], connect),
  ).rejects.toThrow("query rejected");
  await expect(
    runCli(["show", "result:1", ...globals], connect),
  ).rejects.toMatchObject({ code: "RESULT_SUBJECT_MISMATCH" });
  expect(revision).not.toHaveBeenCalled();
  await expect(
    runCli(
      [
        "use",
        "memory_revision:33333333-3333-4333-8333-333333333333",
        "--query-id",
        "query:last",
        ...globals,
      ],
      connect,
    ),
  ).rejects.toMatchObject({ code: "RESULT_SUBJECT_MISMATCH" });
});
it("reads source text through its Material owner with explicit byte bounds", async () => {
  const id = "33333333-3333-4333-8333-333333333333";
  const materialize = vi.fn().mockResolvedValue({
    reference: { kind: "source_region", value: id },
    mediaType: "text/markdown",
    content: new TextEncoder().encode("真实来源原文"),
    totalBytes: 18n,
    rangeStart: 0n,
    rangeEnd: 18n,
    partial: false,
    evidence: [],
    degradation: [],
  });
  const client = { material: { materialize } } as unknown as Awaited<
    ReturnType<typeof connectNousInstance>
  >;
  const result = await runCli(
    [
      "read",
      `source_region:${id}`,
      "--run-root",
      "/tmp/nous",
      "--subject",
      "s",
    ],
    vi.fn().mockResolvedValue(client),
  );
  expect(result).toHaveProperty("data.text", "真实来源原文");
  expect(materialize).toHaveBeenCalledWith({
    subjectId: "s",
    reference: { kind: "source_region", value: id },
    maxBytes: 65536n,
  });
});
it("continues canonical Material references returned by observe and trace", async () => {
  const id = "33333333-3333-4333-8333-333333333333";
  const sourceRegion = vi
    .fn()
    .mockResolvedValue({ sourceRegionId: id, artifactId: "a" });
  const occurrences = vi
    .fn()
    .mockResolvedValue({ items: [], truncated: false });
  const resolve = vi.fn();
  const client = {
    material: { sourceRegion, occurrences },
    identity: { resolve },
  } as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
  const result = await runCli(
    [
      "show",
      `source_region:${id}`,
      "--run-root",
      "/tmp/nous",
      "--subject",
      "s",
    ],
    vi.fn().mockResolvedValue(client),
  );
  expect(result).toHaveProperty("data.sourceRegionId", id);
  expect(sourceRegion).toHaveBeenCalledWith({ subjectId: "s", id });
  expect(occurrences).toHaveBeenCalledWith({
    subjectId: "s",
    artifactId: "a",
    limit: 20,
  });
  expect(resolve).not.toHaveBeenCalled();
});
describe("Agent CLI protocol", () => {
  it("prepares with explicit context and summarizes the frozen inspection", async () => {
    const prepared = {
      boundQuery: '{"representation":{"sha256":"digest"}}',
      embeddingText: "complete intent",
      embeddingRequired: true,
    };
    const prepareQuery = vi.fn().mockResolvedValue(prepared);
    const query = vi.fn();
    const resolve = vi.fn(async ({ kind }: { kind: string }) => ({
      status: "BOUND",
      candidates: [
        {
          canonical: {
            kind,
            value:
              kind === "subject" ? "s" : kind === "session" ? "sess" : "work",
          },
        },
      ],
    }));
    const client = {
      cognition: { prepareQuery, query },
      identity: { resolve },
    } as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
    const connect = vi.fn().mockResolvedValue(client);
    const result = await runCli(
      [
        "query",
        "inspect",
        "decision $return(memory)",
        "--instance-root",
        "/tmp/nous-cli-agent-tests",
        "--run-root",
        "/tmp/nous",
        "--subject",
        "sub:bahog-hijol-mokor",
        "--session",
        "session:babab-babab-babab",
        "--work-context",
        "ctx:zuzuz-zuzuz-zuzuz",
      ],
      connect,
    );
    expect(result).toMatchObject({
      schemaVersion: "nous.cli.v1",
      data: {
        intent: "decision $return(memory)",
        representation: { sha256: "digest", chars: 15 },
      },
    });
    expect(prepareQuery).toHaveBeenCalledWith({
      subjectId: "s",
      sessionId: "sess",
      workContextId: "work",
      nousql: "decision $return(memory)",
    });
    expect(query).not.toHaveBeenCalled();
  });

  it("preserves domain ambiguity candidates through identity and RPC errors", async () => {
    const result = {
      status: "AMBIGUOUS_REFERENCE",
      candidates: [
        { canonical: { kind: "entity", value: "Alice-1" } },
        { canonical: { kind: "entity", value: "Alice-2" } },
      ],
    };
    const client = {
      identity: { resolve: vi.fn().mockResolvedValue(result) },
    } as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
    await expect(
      runCli(
        [
          "identity",
          "resolve",
          "--name",
          "Alice",
          "--kind",
          "entity",
          "--instance-root",
          "/tmp/nous-cli-agent-tests",
          "--run-root",
          "/tmp/nous",
          "--subject",
          "s",
        ],
        vi.fn().mockResolvedValue(client),
      ),
    ).rejects.toMatchObject({
      code: "AMBIGUOUS_REFERENCE",
      candidates: result.candidates,
    });
    expect(
      cliErrorPayload(
        new Error(
          '{"code":"UNRESOLVED_QUERY_REFERENCE","details":[{"start":0}]}',
        ),
      ),
    ).toMatchObject({
      code: "INVALID_ARGUMENT",
      details: [],
    });
    expect(
      cliErrorPayload(
        new NousError(domainError("STALE_CONTEXT is display text", 10, {})),
      ),
    ).toMatchObject({ code: "ABORTED" });
    for (const code of [
      DomainErrorCode.OPERATION_ID_CONFLICT,
      DomainErrorCode.OPERATION_IN_PROGRESS,
    ]) {
      expect(
        cliErrorPayload(
          new NousError(
            domainError("Same display, distinct recovery", 10, {
              code,
              recovery:
                code === DomainErrorCode.OPERATION_ID_CONFLICT
                  ? ErrorRecovery.NEW_OPERATION
                  : ErrorRecovery.RETRY_OPERATION,
            }),
          ),
        ),
      ).toMatchObject({ code: DomainErrorCode[code] });
    }
  });

  it("supports discovery without an instance and emits parse errors only on stderr", async () => {
    const entry = "apps/nous-cli/src/main.ts";
    const help = await exec(process.execPath, [
      "node_modules/tsx/dist/cli.mjs",
      entry,
      "help",
      "--json",
    ]);
    const parsed: unknown = JSON.parse(help.stdout);
    expect(parsed).toHaveProperty("data.commands");
    expect(help.stdout).toContain('"command":"query prepare|inspect"');
    expect(help.stderr).toBe("");
    const failure: unknown = await exec(process.execPath, [
      "node_modules/tsx/dist/cli.mjs",
      entry,
      "--nonexistent",
      "--json",
    ]).catch((error: unknown) => error);
    expect(failure).toMatchObject({ code: 1, stdout: "" });
    if (
      !failure ||
      typeof failure !== "object" ||
      !("stderr" in failure) ||
      typeof failure.stderr !== "string"
    )
      throw new Error("missing process stderr");
    const payload: unknown = JSON.parse(failure.stderr);
    expect(payload).toMatchObject({
      code: "INVALID_ARGUMENT",
      details: [],
      candidates: [],
    });
  });
});

it("uses exact typed revisions and caller-stable feedback identity", async () => {
  const reportUse = vi.fn().mockResolvedValue({ acceptedCount: 1 });
  const client = { cognition: { reportUse } } as unknown as Awaited<
    ReturnType<typeof connectNousInstance>
  >;
  const connect = vi.fn().mockResolvedValue(client);
  const ref = "journal_revision:33333333-3333-4333-8333-333333333333";
  await runCli(
    [
      "use",
      ref,
      "--instance-root",
      "/tmp/nous-cli-agent-tests",
      "--run-root",
      "/tmp/nous",
      "--subject",
      "s",
      "--kind",
      "result_supported",
      "--event-id",
      "event",
      "--query-id",
      "44444444-4444-4444-8444-444444444444",
      "--occurred-at",
      "2026-10-07T00:00:00Z",
    ],
    connect,
  );
  expect(reportUse).toHaveBeenCalledWith(
    expect.objectContaining({
      events: [
        {
          eventId: "event",
          queryId: "44444444-4444-4444-8444-444444444444",
          kind: "result_supported",
          reference: {
            kind: "journal_revision",
            value: "33333333-3333-4333-8333-333333333333",
          },
          occurredAt: { seconds: 1791331200n, nanos: 0 },
        },
      ],
    }),
  );
  await expect(
    runCli(
      [
        "use",
        "memory:33333333-3333-4333-8333-333333333333",
        "--instance-root",
        "/tmp/nous-cli-agent-tests",
        "--run-root",
        "/tmp/nous",
        "--subject",
        "s",
      ],
      connect,
    ),
  ).rejects.toMatchObject({ code: "INVALID_ARGUMENT" });
});
