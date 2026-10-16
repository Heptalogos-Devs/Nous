// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import type { Context } from "@openclaw/ai";
import type { HostPrepareRequest } from "@nous-wave/client/host-bridge";
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
  if (result.length > 2048)
    throw Error(
      "Native caller history exceeds the bounded 2048-turn admission metadata request",
    );
  return result;
}
/** Preserve admitted native turns and causal chains. Runtime summaries keep their existing compaction custody. */
export function scopeCallerHistory(
  context: Context,
  allowed: readonly string[],
): Context {
  const accepted = new Set(allowed);
  let withheld = false;
  return {
    ...context,
    messages: context.messages.filter((message) => {
      const key = callerKey(message);
      if (key !== undefined) {
        withheld = !accepted.has(key);
        return !withheld;
      }
      if (message.role === "user") {
        const kind: unknown = Reflect.get(message, "nousNativeHistoryKind");
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
