// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";

export async function useCommands(
  env: CliEnvironment,
  action: string | undefined,
) {
  const { client, values, state, subjectId, required, resolveReference } = env;
  const kind = values.kind ?? "referenced";
  if (
    ![
      "presented",
      "referenced",
      "acted_on",
      "result_supported",
      "result_refuted",
      "corrected",
      "pinned",
    ].includes(kind)
  )
    throw new CliError("INVALID_ARGUMENT", "Invalid use kind");
  const text = required(action, "Exact cognition revision");
  const reference = await resolveReference(text);
  if (
    ![
      "memory_revision",
      "cognitive_schema_revision",
      "episode_revision",
      "journal_revision",
    ].includes(reference.kind)
  )
    throw new CliError(
      "INVALID_ARGUMENT",
      "Use requires an exact cognition revision",
    );
  const occurred = values["occurred-at"]
    ? new Date(values["occurred-at"])
    : new Date();
  if (
    !Number.isFinite(occurred.getTime()) ||
    (values["occurred-at"] &&
      !/(Z|[+-]\d{2}:\d{2})$/.test(values["occurred-at"]))
  )
    throw new CliError(
      "INVALID_ARGUMENT",
      "--occurred-at requires an ISO timestamp with offset",
    );
  const queryId = values["query-id"];
  if (
    queryId === "query:last" &&
    (!state.lastQuery || state.lastQuery.subjectId !== subjectId)
  )
    throw new CliError(
      "RESULT_SUBJECT_MISMATCH",
      "No query is saved for this Subject",
    );
  return client.cognition.reportUse({
    subjectId,
    sessionId: state.sessionId,
    consumerRef: values.consumer,
    events: [
      {
        eventId: values["event-id"] ?? crypto.randomUUID(),
        queryId:
          (queryId === "query:last" ? state.lastQuery?.queryId : queryId) ??
          (/^result:\d+$/.test(text) ? state.lastQuery?.queryId : undefined),
        reference,
        kind,
        occurredAt: {
          seconds: BigInt(Math.floor(occurred.getTime() / 1000)),
          nanos: occurred.getUTCMilliseconds() * 1_000_000,
        },
      },
    ],
  });
}
