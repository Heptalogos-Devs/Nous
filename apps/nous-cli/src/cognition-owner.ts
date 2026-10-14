// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { CliError } from "./agent.js";
import { operationId, target } from "./content.js";
import type { CliEnvironment } from "./runtime.js";

/** All cognition lifecycle commands use the same typed owner transition semantics. */
export function lifecycle<M>(
  action: string | undefined,
  request: M,
  owner: Record<
    "suppress" | "restore" | "withdraw" | "reaccept" | "purge",
    (request: M) => Promise<unknown>
  >,
) {
  if (
    !action ||
    !["suppress", "restore", "withdraw", "reaccept", "purge"].includes(action)
  )
    throw new CliError(
      "INVALID_ARGUMENT",
      "Unknown cognition lifecycle action",
    );
  return owner[action as keyof typeof owner](request);
}

export async function revisionLink(
  env: CliEnvironment,
  kind: "memory_revision" | "episode_revision",
) {
  const file = (await env.requestPayload(env.values["request-file"])) as {
    from: string;
    to: string;
    relation: string;
  };
  return {
    subjectId: await env.subjectId,
    operationId: operationId(env),
    fromRevisionId: await target(env, file.from, kind),
    toRevisionId: await target(env, file.to, kind),
    relation: env.required(file.relation, "relation"),
  };
}
