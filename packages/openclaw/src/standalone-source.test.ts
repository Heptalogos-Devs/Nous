// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { expect, test } from "vitest";
import {
  standaloneSourceConfiguration,
  standaloneUsage,
} from "./standalone-source.js";
const input = {
  sessionKey: "agent:main:qq:account:group:public",
  sessionId: "native-session",
  runId: "run",
  audience: { visibility: "group" as const, channel: "qq" },
  prompt: "real task",
};
test("standalone source policy is explicit and channel isolated", () => {
  expect(
    standaloneUsage(input, "subject").sourceRead?.originConversationRefs,
  ).toEqual([]);
  const policy = standaloneSourceConfiguration.parse({
    currentConversation: true,
    routes: [
      {
        conversationRef: input.sessionKey,
        originConversationRefs: ["explicit-public"],
      },
    ],
  });
  expect(
    standaloneUsage(input, "subject", policy).sourceRead
      ?.originConversationRefs,
  ).toEqual(["explicit-public", input.sessionKey]);
  expect(
    standaloneUsage(
      { ...input, sessionKey: "agent:main:qq:account:group:other" },
      "subject",
      policy,
    ).sourceRead?.originConversationRefs,
  ).toEqual(["agent:main:qq:account:group:other"]);
  expect(() =>
    standaloneUsage({ ...input, sessionKey: "agent:main" }, "subject", policy),
  ).toThrow("session isolation");
  expect(() =>
    standaloneUsage({ ...input, sessionKey: undefined }, "subject", policy),
  ).toThrow("session isolation");
});
