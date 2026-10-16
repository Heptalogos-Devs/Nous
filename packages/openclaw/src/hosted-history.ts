// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { toJson, type JsonObject } from "@bufbuild/protobuf";
import {
  CognitionUsageSchema,
  type CognitionUsage,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { Context } from "@openclaw/ai";
import {
  hostPrepareRequestSchema,
  type HostPrepareRequest,
} from "@nous-wave/client/host-bridge";
type Message = Context["messages"][number];
function callerKey(message: Message): string | undefined {
  if (message.role !== "user") return;
  const key: unknown = Reflect.get(message, "idempotencyKey");
  return typeof key === "string" ? key : undefined;
}
/** Native metadata, never prompt markers or tool/model arguments, names historical caller turns. */
export async function historyUsers(
  context: Context,
): Promise<NonNullable<HostPrepareRequest["historyUsers"]>> {
  const result: NonNullable<HostPrepareRequest["historyUsers"]> = [];
  for (const message of context.messages) {
    const scope: unknown = Reflect.get(message, "nousCompactionScope");
    if (
      scope &&
      typeof scope === "object" &&
      "historyUsers" in scope &&
      Array.isArray(scope.historyUsers)
    ) {
      const users =
        hostPrepareRequestSchema.shape.historyUsers.parse(scope.historyUsers) ??
        [];
      for (const user of users) {
        if (
          !user.key.endsWith(":user") ||
          user.nativeRunId !== user.key.slice(0, -5)
        )
          throw Error("Native summary caller provenance is invalid");
        result.push(user);
      }
    }
    const key = callerKey(message);
    if (!key || !key.endsWith(":user")) continue;
    const content =
      typeof message.content === "string"
        ? message.content
        : message.content
            .filter((part) => part.type === "text")
            .map((part) => (part.type === "text" ? part.text : ""))
            .join("\n");
    const digest = new Uint8Array(
      await crypto.subtle.digest("SHA-256", new TextEncoder().encode(content)),
    );
    result.push({
      key,
      nativeRunId: key.slice(0, -5),
      promptDigest: [...digest]
        .map((b) => b.toString(16).padStart(2, "0"))
        .join(""),
    });
  }
  const byKey = new Map<string, (typeof result)[number]>();
  for (const user of result) {
    const previous = byKey.get(user.key);
    if (previous && JSON.stringify(previous) !== JSON.stringify(user))
      throw Error("Conflicting native caller provenance");
    byKey.set(user.key, user);
  }
  const unique = [...byKey.values()];
  if (unique.length > 2048)
    throw Error(
      "Native caller history exceeds the bounded 2048-turn admission metadata request",
    );
  return unique;
}
/** Re-evaluate a native summary with the current authenticated role and source grant. */
export function allowsCompactionSummary(
  scope: unknown,
  usage: CognitionUsage,
  allowed: readonly string[],
): boolean {
  return allowsCompactionScope(
    scope,
    {
      ...(toJson(CognitionUsageSchema, usage) as JsonObject),
      timeDomain: usage.worldTime?.timeDomainRef,
      cognitionAudience: (toJson(CognitionUsageSchema, usage) as JsonObject)
        .audience,
    },
    allowed,
  );
}
export function allowsCompactionScope(
  scope: unknown,
  current: unknown,
  allowed: readonly string[],
): boolean {
  if (
    !scope ||
    typeof scope !== "object" ||
    Array.isArray(scope) ||
    !current ||
    typeof current !== "object" ||
    Array.isArray(current)
  )
    return false;
  const value = scope as Record<string, unknown>,
    now = current as Record<string, unknown>;
  for (const field of [
    "role",
    "purpose",
    "view",
    "contactNature",
    "worldId",
    "timeDomain",
  ])
    if ((value[field] ?? "") !== (now[field] ?? "")) return false;
  if (
    !Array.isArray(value.history) ||
    !value.history.every(
      (key) => typeof key === "string" && allowed.includes(key),
    )
  )
    return false;
  const beforeAudience = value.cognitionAudience,
    afterAudience = now.cognitionAudience;
  if (
    beforeAudience &&
    typeof beforeAudience === "object" &&
    afterAudience &&
    typeof afterAudience === "object"
  ) {
    for (const field of ["visibility", "conversationRef", "channelRef"])
      if (
        (Reflect.get(beforeAudience, field) ?? "") !==
        (Reflect.get(afterAudience, field) ?? "")
      )
        return false;
  } else if (beforeAudience !== afterAudience) return false;
  // The Host reauthorizes each original caller. SourceRead.sourceRefs are this
  // invocation's knowledge candidates, not a permanent history permission list.
  return true;
}
/** Preserve admitted native turns and causal chains. Runtime summaries keep their existing compaction custody. */
export function scopeCallerHistory(
  context: Context,
  allowed: readonly string[],
  usage?: CognitionUsage | ((scope: unknown) => boolean),
): Context {
  const accepted = new Set(allowed);
  let withheld = false;
  return {
    ...context,
    messages: context.messages.filter((message) => {
      const kind: unknown = Reflect.get(message, "nousNativeHistoryKind");
      const role: unknown = Reflect.get(message, "role");
      if (
        ["compaction_summary", "branch_summary"].includes(String(kind)) ||
        role === "compactionSummary" ||
        role === "branchSummary"
      ) {
        withheld =
          typeof usage === "function"
            ? !usage(Reflect.get(message, "nousCompactionScope"))
            : !!usage &&
              !allowsCompactionSummary(
                Reflect.get(message, "nousCompactionScope"),
                usage,
                allowed,
              );
        return !withheld;
      }
      const key = callerKey(message);
      if (key !== undefined) {
        withheld = !accepted.has(key);
        return !withheld;
      }
      if (message.role === "user") {
        const carrier: unknown = Reflect.get(message, "runtimeContextCarrier");
        withheld = !(
          ["compaction_summary", "branch_summary"].includes(String(kind)) ||
          carrier === true
        );
        return !withheld;
      }
      return !withheld;
    }),
  };
}
