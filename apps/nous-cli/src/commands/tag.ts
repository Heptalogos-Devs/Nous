// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { CliError, boundedInteger, uniqueReference } from "../agent.js";
import {
  reviseInput,
  mergeInput,
  splitInput,
  attachmentInput,
  tagTarget,
  tagContent,
  revisionBasis,
  associationBasis,
} from "../mutation-input.js";
import type { CliEnvironment } from "../runtime.js";

export async function tagCommands(
  env: CliEnvironment,
  action: string | undefined,
  argument?: string,
) {
  const {
    client,
    values,
    subjectId,
    required,
    resolveReference,
    requestPayload,
  } = env;
  const operationId = values["operation-id"] ?? crypto.randomUUID();
  if (action === "create")
    return client.concepts.createTag({
      subjectId,
      operationId,
      tag: {
        label: required(values.name, "--name"),
        description: values.description,
        kindHint: values["kind-hint"],
        origin: "host_explicit",
      },
    });
  if (action === "revise") {
    const input = reviseInput.parse(
      await requestPayload(values["request-file"]),
    );
    return client.concepts.reviseTag({
      subjectId,
      operationId,
      target: await tagTarget(env, input.target),
      content: tagContent(input),
    });
  }
  if (action === "merge") {
    const input = mergeInput.parse(
      await requestPayload(values["request-file"]),
    );
    return client.concepts.mergeTags({
      subjectId,
      operationId,
      survivor: await tagTarget(env, input.survivor),
      retired: await Promise.all(
        input.retired.map((text) => tagTarget(env, text)),
      ),
      basis: await Promise.all(
        input.basis.map((entry) => revisionBasis(env, entry)),
      ),
    });
  }
  if (action === "split") {
    const input = splitInput.parse(
      await requestPayload(values["request-file"]),
    );
    return client.concepts.splitTag({
      subjectId,
      operationId,
      parent: await tagTarget(env, input.parent),
      children: input.children.map(tagContent),
      basis: await Promise.all(
        input.basis.map((entry) => revisionBasis(env, entry)),
      ),
    });
  }
  if (action === "attach") {
    const input = attachmentInput.parse(
      await requestPayload(values["association-file"]),
    );
    const to = await resolveReference(
      required(values.tag?.[0], "--tag"),
      "tag",
    );
    const from = await resolveReference(
      required(argument, "Exact cognition revision"),
    );
    if (
      ![
        "memory_revision",
        "cognitive_schema_revision",
        "episode_revision",
        "journal_revision",
      ].includes(from.kind)
    )
      throw new CliError(
        "REFERENCE_TYPE_MISMATCH",
        "Attachment requires an exact cognition revision",
      );
    return client.concepts.associate({
      subjectId,
      operationId,
      association: {
        from,
        to,
        relationKind: "tag_attachment",
        polarity: "positive",
        basisClass: "host_explicit",
        basis: await Promise.all(
          input.basis.map((entry) => associationBasis(env, entry)),
        ),
      },
    });
  }
  if (action === "list" || action === "search") {
    const page = {
      pageToken: values["page-token"],
      pageSize: boundedInteger(values["page-size"], 1, 200, "--page-size"),
    };
    return action === "list"
      ? client.concepts.listTags({ subjectId, page })
      : client.concepts.searchTags({
          subjectId,
          text: required(argument, "Search text"),
          page,
        });
  }
  if (action === "resolve") {
    const result = await client.identity.resolve({
      subjectId,
      kind: "tag",
      locator: { case: "name", value: required(argument, "Tag name") },
    });
    uniqueReference(result);
    return result;
  }
  if (action === "get") {
    const ref = await resolveReference(
      required(argument, "Tag reference"),
      "tag",
    );
    return client.concepts.getTag({ subjectId, id: ref.value });
  }

  throw new CliError("INVALID_ARGUMENT", "Unknown tag command");
}
