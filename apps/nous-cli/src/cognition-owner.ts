// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import type { NousClient } from "@nous-wave/client";
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

/** One CLI dispatch table owns revision reads and the follow-up actions actually available. */
const revisionReaders: Record<
  string,
  {
    read: (
      client: NousClient,
      input: { subjectId: string; id: string },
    ) => Promise<unknown>;
    trace: boolean;
  }
> = {
  memory_revision: {
    read: (client, input) => client.memory.getMemoryRevision(input),
    trace: true,
  },
  cognitive_schema_revision: {
    read: (client, input) => client.concepts.getCognitiveSchemaRevision(input),
    trace: true,
  },
  episode_revision: {
    read: (client, input) => client.memory.getEpisodeRevision(input),
    trace: true,
  },
  journal_revision: {
    read: (client, input) => client.memory.getJournalRevision(input),
    trace: true,
  },
  social_relation_revision: {
    read: (client, input) => client.social.getRevision(input),
    trace: true,
  },
  language_convention_revision: {
    read: (client, input) => client.social.getRevision(input),
    trace: true,
  },
  self_facet_revision: {
    read: (client, input) => client.self.getRevision(input),
    trace: true,
  },
  narrative_thread_revision: {
    read: (client, input) => client.self.getRevision(input),
    trace: true,
  },
};

export function cognitionRevisionReader(kind: string) {
  return Object.hasOwn(revisionReaders, kind)
    ? revisionReaders[kind]
    : undefined;
}
