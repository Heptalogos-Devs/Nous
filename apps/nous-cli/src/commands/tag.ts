// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { CognitiveRefSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { CliError, uniqueReference } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
import {
  TagContentSchema,
  ReviseTagRequestSchema,
  MergeTagsRequestSchema,
  SplitTagRequestSchema,
  AssociationSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { content, page } from "../content.js";

export async function tagCommands(
  env: CliEnvironment,
  action: string | undefined,
  argument?: string,
) {
  const { client, values, required, resolveReference } = env;
  const subjectId = await env.subjectId;
  const operationId = values["operation-id"] ?? crypto.randomUUID();
  if (action === "history")
    return client.concepts.listTagRevisions({
      subjectId,
      object: await resolveReference(
        required(argument, "Tag reference"),
        "tag",
      ),
      page: page(env),
    });
  if (action === "create")
    return client.concepts.createTag({
      subjectId,
      operationId,
      content: values["request-file"]
        ? await content(env, TagContentSchema)
        : {
            label: required(values.name, "--name"),
            description: values.description,
            kindHint: values["kind-hint"],
          },
    });
  if (action === "revise")
    return client.concepts.reviseTag({
      ...(await content(env, ReviseTagRequestSchema)),
      subjectId,
      operationId,
    });
  if (action === "merge")
    return client.concepts.mergeTags({
      ...(await content(env, MergeTagsRequestSchema)),
      subjectId,
      operationId,
    });
  if (action === "split")
    return client.concepts.splitTag({
      ...(await content(env, SplitTagRequestSchema)),
      subjectId,
      operationId,
    });
  if (action === "attach") {
    const association = await content(env, AssociationSchema);
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
    return client.concepts.createAssociation({
      subjectId,
      operationId,
      association: {
        ...association,
        from: create(CognitiveRefSchema, from),
        to: create(CognitiveRefSchema, to),
        relationKind: "tag_attachment",
        polarity: "positive",
        basisClass: "host_explicit",
      },
    });
  }
  if (action === "list" || action === "search") {
    const pagination = page(env);
    return action === "list"
      ? client.concepts.listTags({
          subjectId,
          status: values.status,
          page: pagination,
        })
      : client.concepts.searchTags({
          subjectId,
          text: required(argument, "Search text"),
          page: pagination,
        });
  }
  if (action === "resolve") {
    const result = await client.identity.resolveIdentity({
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
