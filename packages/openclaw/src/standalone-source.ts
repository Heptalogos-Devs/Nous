// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { z } from "zod";
import { create } from "@bufbuild/protobuf";
import { CognitionUsageSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { ManagedModelInput } from "./contract.js";
const refs = z
  .array(z.strictObject({ kind: z.string().min(1), value: z.string().min(1) }))
  .max(1024);
export const standaloneSourceConfiguration = z.strictObject({
  currentConversation: z.boolean().default(false),
  sourceRefs: refs.default([]),
  originConversationRefs: z
    .array(z.string().min(1).max(1024))
    .max(64)
    .default([]),
  routes: z
    .array(
      z.strictObject({
        conversationRef: z.string().min(1),
        sourceRefs: refs.default([]),
        originConversationRefs: z
          .array(z.string().min(1).max(1024))
          .max(64)
          .default([]),
      }),
    )
    .max(64)
    .default([]),
});
export function standaloneUsage(
  input: Pick<
    ManagedModelInput,
    "sessionKey" | "sessionId" | "runId" | "audience" | "prompt"
  >,
  subjectId: string,
  policy?: z.infer<typeof standaloneSourceConfiguration>,
  observation = false,
) {
  const conversation = input.sessionKey ?? input.sessionId;
  const route = policy?.routes.find((r) => r.conversationRef === conversation);
  const origins = new Set([
    ...(policy?.originConversationRefs ?? []),
    ...(route?.originConversationRefs ?? []),
  ]);
  if (policy?.currentConversation) {
    // Native direct/group delivery must already use an isolated account/channel/peer session.
    // Shared DM main history is never promoted to a conversation authorization.
    if (
      input.audience.visibility !== "internal" &&
      (!input.sessionKey || /:main$/.test(input.sessionKey))
    )
      throw new Error(
        "Standalone SourceRead currentConversation requires native account/channel/peer session isolation; shared DM main history is not authorized",
      );
    origins.add(conversation);
  }
  if (origins.size > 64)
    throw new Error("Standalone SourceRead origin scope exceeds64");
  return create(CognitionUsageSchema, {
    agentInstanceId: observation
      ? `openclaw:${input.sessionId}`
      : `${input.sessionId}:${input.runId}`,
    purpose: observation
      ? "reflection"
      : input.sessionKey?.includes(":subagent:")
        ? "worker"
        : "deliberation",
    intent: observation
      ? "Actual observed native source"
      : input.prompt.trim() ||
        "Continue current work using actual tool results.",
    audience: {
      conversationRef: conversation,
      visibility: input.audience.visibility,
      channelRef: input.audience.channel,
    },
    sourceRead: {
      subjectId,
      sourceRefs: [...(policy?.sourceRefs ?? []), ...(route?.sourceRefs ?? [])],
      originConversationRefs: [...origins],
    },
  });
}
