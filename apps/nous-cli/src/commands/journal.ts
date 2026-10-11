// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import {
  ListRequestSchema,
  ObjectRequestSchema,
  PageSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  JournalMutationRequestSchema,
  ListJournalRevisionsRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/journal_pb.js";
import { CliError, boundedInteger } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";

function epoch(value: string | undefined) {
  if (!value || !/^(0|[1-9][0-9]*)$/.test(value))
    throw new CliError(
      "INVALID_ARGUMENT",
      "--expected-epoch must be a nonnegative int64",
    );
  const parsed = BigInt(value);
  if (parsed > 9223372036854775807n)
    throw new CliError("INVALID_ARGUMENT", "Expected epoch exceeds int64");
  return parsed;
}

export async function journalCommands(
  env: CliEnvironment,
  action?: string,
  argument?: string,
) {
  const { client, values, subjectId, required, resolveReference } = env;
  const page = create(PageSchema, {
    pageSize: boundedInteger(values["page-size"], 1, 200, "--page-size"),
    pageToken: values["page-token"],
  });
  if (action === "list")
    return client.memory.listJournals(
      create(ListRequestSchema, {
        subjectId,
        page,
        status: values.status ?? "",
      }),
    );
  if (["get", "revision", "history"].includes(action ?? "")) {
    const ref = await resolveReference(
      required(
        argument,
        `Journal ${action === "revision" ? "revision" : "object"} reference`,
      ),
      action === "revision" ? "journal_revision" : "journal",
    );
    const request = create(ObjectRequestSchema, { subjectId, id: ref.value });
    if (action === "get") return client.memory.getJournal(request);
    if (action === "revision") return client.memory.getJournalRevision(request);
    return client.memory.listJournalRevisions(
      create(ListJournalRevisionsRequestSchema, {
        subjectId,
        journalId: ref.value,
        page,
      }),
    );
  }
  if (
    ["suppress", "restore", "withdraw", "reaccept", "purge"].includes(
      action ?? "",
    )
  ) {
    const ref = await resolveReference(
      required(argument, "Journal reference"),
      "journal",
    );
    const request = create(JournalMutationRequestSchema, {
      operationId: values["operation-id"] ?? crypto.randomUUID(),
      subjectId,
      journalId: ref.value,
      expectedObjectEpoch: epoch(values["expected-epoch"]),
    });
    switch (action) {
      case "suppress":
        return client.memory.suppressJournal(request);
      case "restore":
        return client.memory.restoreJournal(request);
      case "withdraw":
        return client.memory.withdrawJournal(request);
      case "reaccept":
        return client.memory.reacceptJournal(request);
      case "purge":
        return client.memory.purgeJournal(request);
    }
  }
  throw new CliError(
    "INVALID_ARGUMENT",
    "Use journal list|get|revision|history|suppress|restore|withdraw|reaccept|purge",
  );
}
