// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { expect, it, vi } from "vitest";
import { NousError } from "@nous-wave/client";
import { create } from "@bufbuild/protobuf";
import { ReportUseResponseSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { ReportUseRequest } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  domainError,
  ErrorCode,
  ErrorOwner,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import type { connectNousInstance } from "@nous-wave/client/node";
import { rm } from "node:fs/promises";
import { runCli } from "./support/client.js";
import { cliErrorPayload } from "../src/agent.js";
import { workspaceTemp } from "../../../scripts/workspace.js";

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
  const read = vi.fn();
  const reportUse = vi.fn().mockResolvedValue({ acceptedCount: 1 });
  const client = {
    cognition: { query, reportUse, read },
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
  expect(read).not.toHaveBeenCalled();
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
  ).toMatchObject({ details: [] });
  expect(
    cliErrorPayload(
      new Error(
        '{"code":"UNRESOLVED_QUERY_REFERENCE","details":[{"start":0}]}',
      ),
    ),
  ).not.toHaveProperty("code");
  expect(
    cliErrorPayload(
      new NousError(
        domainError("STALE_CONTEXT is display text", 10, {
          code: ErrorCode.STALE_CONTEXT,
          owner: ErrorOwner.RUNTIME,
          recovery: ErrorRecovery.REFRESH_STATE,
        }),
      ),
    ),
  ).toMatchObject({
    code: "STALE_CONTEXT",
    owner: "RUNTIME",
    recovery: "REFRESH_STATE",
  });
  for (const code of [
    ErrorCode.OPERATION_ID_CONFLICT,
    ErrorCode.OPERATION_IN_PROGRESS,
  ] as const) {
    expect(
      cliErrorPayload(
        new NousError(
          domainError("Same display, distinct recovery", 10, {
            code,
            owner: ErrorOwner.RUNTIME,
            recovery:
              code === ErrorCode.OPERATION_ID_CONFLICT
                ? ErrorRecovery.NEW_OPERATION
                : ErrorRecovery.RETRY_OPERATION,
          }),
        ),
      ),
    ).toMatchObject({ code: ErrorCode[code], owner: "RUNTIME" });
  }
});

it("uses exact typed revisions and caller-stable feedback identity", async () => {
  const root = await workspaceTemp("tests", "cli-use-feedback-");
  const reportUse = vi.fn(async (_request: ReportUseRequest) =>
    create(ReportUseResponseSchema, { acceptedCount: 1 }),
  );
  const client = { cognition: { reportUse } } as unknown as Awaited<
    ReturnType<typeof connectNousInstance>
  >;
  const connect = vi.fn().mockResolvedValue(client);
  const ref = "journal_revision:33333333-3333-4333-8333-333333333333";
  try {
    await runCli(
      [
        "use",
        ref,
        "--instance-root",
        root,
        "--run-root",
        root,
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
    const event = reportUse.mock.calls[0]![0].events[0]!;
    expect(event.eventId).toBe("event");
    expect(event.queryId).toBe("44444444-4444-4444-8444-444444444444");
    expect(event.kind).toBe("result_supported");
    expect(event.reference?.kind).toBe("journal_revision");
    expect(event.reference?.value).toBe("33333333-3333-4333-8333-333333333333");
    expect(event.occurredAt?.seconds).toBe(1791331200n);
    expect(event.occurredAt?.nanos).toBe(0);
    await expect(
      runCli(
        [
          "use",
          "memory:33333333-3333-4333-8333-333333333333",
          "--instance-root",
          root,
          "--run-root",
          root,
          "--subject",
          "s",
        ],
        connect,
      ),
    ).rejects.toMatchObject({ code: "INVALID_ARGUMENT" });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
