// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import { commaSeparatedArgs, commonArgs, stringArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

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

const operationId = (env: CliEnvironment) =>
  env.values["operation-id"] ?? crypto.randomUUID();
const tagPage = (env: CliEnvironment) => ({
  pageToken: env.values["page-token"],
  pageSize: boundedInteger(env.values["page-size"], 1, 200, "--page-size"),
});

const tagCreate = (env: CliEnvironment) =>
  env.client.concepts.createTag({
    subjectId: env.subjectId,
    operationId: operationId(env),
    tag: {
      label: env.required(env.values.name, "--name"),
      description: env.values.description,
      kindHint: env.values["kind-hint"],
      origin: "host_explicit",
    },
  });
const tagRevise = async (env: CliEnvironment) => {
  const input = reviseInput.parse(
    await env.requestPayload(env.values["request-file"]),
  );
  return env.client.concepts.reviseTag({
    subjectId: env.subjectId,
    operationId: operationId(env),
    target: await tagTarget(env, input.target),
    content: tagContent(input),
  });
};
const tagMerge = async (env: CliEnvironment) => {
  const input = mergeInput.parse(
    await env.requestPayload(env.values["request-file"]),
  );
  return env.client.concepts.mergeTags({
    subjectId: env.subjectId,
    operationId: operationId(env),
    survivor: await tagTarget(env, input.survivor),
    retired: await Promise.all(
      input.retired.map((text) => tagTarget(env, text)),
    ),
    basis: await Promise.all(
      input.basis.map((entry) => revisionBasis(env, entry)),
    ),
  });
};
const tagSplit = async (env: CliEnvironment) => {
  const input = splitInput.parse(
    await env.requestPayload(env.values["request-file"]),
  );
  return env.client.concepts.splitTag({
    subjectId: env.subjectId,
    operationId: operationId(env),
    parent: await tagTarget(env, input.parent),
    children: input.children.map(tagContent),
    basis: await Promise.all(
      input.basis.map((entry) => revisionBasis(env, entry)),
    ),
  });
};
const tagAttach = async (env: CliEnvironment, argument?: string) => {
  const input = attachmentInput.parse(
    await env.requestPayload(env.values["association-file"]),
  );
  const to = await env.resolveReference(
    env.required(env.values.tag?.[0], "--tag"),
    "tag",
  );
  const from = await env.resolveReference(
    env.required(argument, "Exact cognition revision"),
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
  return env.client.concepts.associate({
    subjectId: env.subjectId,
    operationId: operationId(env),
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
};
const tagList = (env: CliEnvironment) =>
  env.client.concepts.listTags({
    subjectId: env.subjectId,
    page: tagPage(env),
  });
const tagSearch = (env: CliEnvironment, argument?: string) =>
  env.client.concepts.searchTags({
    subjectId: env.subjectId,
    text: env.required(argument, "Search text"),
    page: tagPage(env),
  });
const tagResolve = async (env: CliEnvironment, argument?: string) => {
  const result = await env.client.identity.resolve({
    subjectId: env.subjectId,
    kind: "tag",
    locator: { case: "name", value: env.required(argument, "Tag name") },
  });
  uniqueReference(result);
  return result;
};
const tagGet = async (env: CliEnvironment, argument?: string) => {
  const ref = await env.resolveReference(
    env.required(argument, "Tag reference"),
    "tag",
  );
  return env.client.concepts.getTag({
    subjectId: env.subjectId,
    id: ref.value,
  });
};

export function createTagCommand(leaf: CliLeaf) {
  return defineCommand({
    args: commonArgs,
    subCommands: {
      create: leaf(
        { ...stringArgs("name", "description", "kind-hint", "operation-id") },
        tagCreate,
      ),
      get: leaf({}, tagGet, {
        parameters: ["<Tag reference>"],
      }),
      list: leaf({ ...stringArgs("page-size", "page-token") }, tagList),
      search: leaf({ ...stringArgs("page-size", "page-token") }, tagSearch, {
        parameters: ["<text>"],
      }),
      resolve: leaf({}, tagResolve, { parameters: ["<name>"] }),
      revise: leaf(
        { ...stringArgs("request-file", "operation-id") },
        tagRevise,
      ),
      merge: leaf({ ...stringArgs("request-file", "operation-id") }, tagMerge),
      split: leaf({ ...stringArgs("request-file", "operation-id") }, tagSplit),
      attach: leaf(
        {
          ...stringArgs("association-file", "operation-id"),
          ...commaSeparatedArgs("tag"),
        },
        tagAttach,
        { parameters: ["<exact cognition revision>"] },
      ),
    },
  });
}
