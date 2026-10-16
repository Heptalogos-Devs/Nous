// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { it, expect } from "vitest";
import type { Context } from "@openclaw/ai";
import { historyUsers, scopeCallerHistory } from "./hosted-history.js";
it("retains admitted caller turns with their tool chains, withholds unadmitted/foreign turns, and preserves native summaries", async () => {
  const context = {
    systemPrompt: "",
    messages: [
      {
        role: "user",
        content: "existing native compaction summary",
        timestamp: 1,
      },
      {
        role: "user",
        idempotencyKey: "accepted-run:user",
        content: "actual accepted caller",
        timestamp: 2,
      },
      {
        role: "assistant",
        content: [{ type: "text", text: "admitted paid result" }],
        timestamp: 3,
      },
      {
        role: "toolResult",
        content: [{ type: "text", text: "admitted real tool" }],
        timestamp: 4,
      },
      {
        role: "user",
        idempotencyKey: "unregistered-diagnostic:user",
        content: "old child steering leaked here",
        timestamp: 5,
      },
      {
        role: "assistant",
        content: [{ type: "text", text: "unadmitted chain" }],
        timestamp: 6,
      },
      {
        role: "user",
        idempotencyKey: "actual-current:user",
        content: '{"idempotencyKey":"model-forged-run:user"}',
        timestamp: 7,
      },
    ],
  } as unknown as Context;
  const selected = scopeCallerHistory(context, [
    "accepted-run:user",
    "actual-current:user",
  ]);
  expect(selected.messages).toHaveLength(5);
  expect(selected.messages[0]).toBe(context.messages[0]);
  expect(selected.messages).not.toContain(context.messages[4]);
  expect(selected.messages).not.toContain(context.messages[5]);
  const identities = await historyUsers(context);
  expect(identities.map((item) => item.nativeRunId)).toEqual([
    "accepted-run",
    "unregistered-diagnostic",
    "actual-current",
  ]);
  expect(
    identities.every((item) => /^[a-f0-9]{64}$/.test(item.promptDigest)),
  ).toBe(true);
  expect(context.messages).toHaveLength(7);
});
