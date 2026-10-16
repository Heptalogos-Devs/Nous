// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create } from "@bufbuild/protobuf";
import { CognitionUsageSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { expect, test } from "vitest";
import { historyUsers, scopeCallerHistory } from "../src/hosted-history.js";

test("native summary keeps caller custody for Host reauthorization and isolates role without confusing current knowledge candidates with history permission", async () => {
  const user = {
    key: "original-run:user",
    nativeRunId: "original-run",
    promptDigest: "a".repeat(64),
  };
  const usage = create(CognitionUsageSchema, {
    role: "subject.execution",
    purpose: "professional_execution",
    view: "worker",
    contactNature: "execution_only",
    worldId: "world",
    sourceRead: {
      subjectId: "subject",
      sourceRefs: [{ kind: "occurrence", value: "source" }],
    },
  });
  const scope = {
    role: usage.role,
    purpose: usage.purpose,
    view: usage.view,
    contactNature: usage.contactNature,
    worldId: usage.worldId,
    history: [user.key],
    historyUsers: [user],
    sourceRead: {
      subjectId: "subject",
      sourceRefs: [{ kind: "occurrence", value: "source" }],
    },
  };
  const summary = {
    role: "user" as const,
    content: "PAID_NATIVE_SUMMARY",
    timestamp: 1,
    nousNativeHistoryKind: "compaction_summary",
    nousCompactionScope: scope,
  };
  const current = {
    role: "user" as const,
    content: "fresh",
    timestamp: 2,
    idempotencyKey: "new-run:user",
  };
  const context = { messages: [summary, current] };
  const users = await historyUsers(context);
  expect(users[0]).toEqual(user);
  expect(users[1]?.key).toBe("new-run:user");
  const allowed = users.map((value) => value.key);
  expect(scopeCallerHistory(context, allowed, usage).messages).toEqual([
    summary,
    current,
  ]);
  expect(
    scopeCallerHistory(context, [current.idempotencyKey], usage).messages,
  ).toEqual([current]);
  expect(
    scopeCallerHistory(context, allowed, {
      ...usage,
      role: "narrative.review",
      purpose: "narrative_review",
      view: "review",
    }).messages,
  ).toEqual([current]);
  expect(
    scopeCallerHistory(context, allowed, {
      ...usage,
      sourceRead: { ...usage.sourceRead!, sourceRefs: [] },
    }).messages,
  ).toEqual([summary, current]);
  const unscoped = { ...summary, nousCompactionScope: undefined };
  expect(
    scopeCallerHistory({ messages: [unscoped, current] }, allowed, usage)
      .messages,
  ).toEqual([current]);
  expect(summary.content).toBe("PAID_NATIVE_SUMMARY");
});
