// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import { commonArgs, stringArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

import { fromJson, type JsonValue } from "@bufbuild/protobuf";
import { TemporalExtentSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { z } from "zod";
import { nonnegativeInt64, CliError } from "../agent.js";
import { revisionBasis } from "../mutation-input.js";
import type { CliEnvironment } from "../runtime.js";
import { typedCognition } from "./cognition.js";

const reference = z.string().min(1).max(2048);
const basisInput = z.strictObject({
  ref: reference,
  role: z.enum(["direct", "interpretation", "contextual"]).default("direct"),
  epistemic_relation: z
    .enum([
      "supports",
      "contradicts",
      "corroborates",
      "weakens",
      "corrects",
      "counterexample",
      "inferred_from",
    ])
    .optional(),
});
const memberInput = z.strictObject({
  ref: reference,
  role: z.string().min(1).max(128),
  ordinal: z.number().int().nonnegative().optional(),
});
const episodeInput = z.strictObject({
  track_key: z.string().min(1).max(256).optional(),
  title: z.string().max(512).optional(),
  parent_episode_revision: reference.optional(),
  experience_time: z.unknown().optional(),
  boundary_explanation: z.string().min(1).max(8192),
  members: z.array(memberInput).min(1).max(512),
  basis: z.array(basisInput).min(1).max(512),
});

async function episodeCommon(
  env: CliEnvironment,
  action: string,
  argument?: string,
) {
  const common = await typedCognition.commonCommand(
    env,
    "episode",
    action,
    argument,
    { list: true, history: true },
  );
  if (common.handled) return common.value;
  throw new CliError("INTERNAL_FAILURE", "Episode command is not handled");
}

async function writeEpisode(
  env: CliEnvironment,
  argument: string | undefined,
  createNew: boolean,
) {
  const {
    client,
    values,
    subjectId,
    required,
    resolveReference,
    requestPayload,
  } = env;
  const parsed = episodeInput.parse(
    await requestPayload(values["request-file"]),
  );
  const operationId = values["operation-id"] ?? crypto.randomUUID();
  const basis = await Promise.all(
    parsed.basis.map((item) => revisionBasis(env, item)),
  );
  const members = await Promise.all(
    parsed.members.map(async (item, index) => {
      const ref = await resolveReference(item.ref);
      return {
        ordinal: item.ordinal ?? index,
        role: item.role,
        reference: {
          kind: ref.kind,
          value: ref.value,
        },
      };
    }),
  );
  const experienceTime = parsed.experience_time
    ? fromJson(TemporalExtentSchema, parsed.experience_time as JsonValue)
    : undefined;
  const parentEpisodeRevisionId = parsed.parent_episode_revision
    ? (
        await resolveReference(
          parsed.parent_episode_revision,
          "episode_revision",
        )
      ).value
    : undefined;
  if (createNew) {
    return client.memory.createEpisode({
      operationId,
      subjectId,
      trackKey: required(parsed.track_key, "track_key in semantic TOML"),
      title: parsed.title,
      parentEpisodeRevisionId,
      experienceTime,
      boundaryExplanation: parsed.boundary_explanation,
      members,
      basis,
    });
  }
  const ref = await resolveReference(
    required(argument, "Episode reference"),
    "episode",
  );
  return client.memory.reviseEpisode({
    operationId,
    subjectId,
    episodeId: ref.value,
    expectedObjectEpoch: nonnegativeInt64(
      values["expected-epoch"],
      "--expected-epoch",
    ),
    intent: z
      .enum(["resegment", "reinterpret"])
      .parse(required(values.intent, "--intent resegment|reinterpret")),
    title: parsed.title,
    parentEpisodeRevisionId,
    experienceTime,
    boundaryExplanation: parsed.boundary_explanation,
    members,
    basis,
  });
}

const episodeList = (env: CliEnvironment) => episodeCommon(env, "list");
const episodeGet = (env: CliEnvironment, argument?: string) =>
  episodeCommon(env, "get", argument);
const episodeRevision = (env: CliEnvironment, argument?: string) =>
  episodeCommon(env, "revision", argument);
const episodeHistory = (env: CliEnvironment, argument?: string) =>
  episodeCommon(env, "history", argument);
const episodeSuppress = (env: CliEnvironment, argument?: string) =>
  episodeCommon(env, "suppress", argument);
const episodeRestore = (env: CliEnvironment, argument?: string) =>
  episodeCommon(env, "restore", argument);
const episodeWithdraw = (env: CliEnvironment, argument?: string) =>
  episodeCommon(env, "withdraw", argument);
const episodeReaccept = (env: CliEnvironment, argument?: string) =>
  episodeCommon(env, "reaccept", argument);
const episodePurge = (env: CliEnvironment, argument?: string) =>
  episodeCommon(env, "purge", argument);
const episodeCreate = (env: CliEnvironment) =>
  writeEpisode(env, undefined, true);
const episodeRevise = (env: CliEnvironment, argument?: string) =>
  writeEpisode(env, argument, false);
const episodeLink = async (env: CliEnvironment) => {
  const { client, values, subjectId, required, resolveReference } = env;
  const from = await resolveReference(
    required(values.from, "--from exact Episode revision"),
    "episode_revision",
  );
  const to = await resolveReference(
    required(values.to, "--to exact Episode revision"),
    "episode_revision",
  );
  return client.memory.linkEpisodeRevisions({
    operationId: values["operation-id"] ?? crypto.randomUUID(),
    subjectId,
    fromRevisionId: from.value,
    toRevisionId: to.value,
    relation: required(values.relation, "--relation"),
  });
};

export function createEpisodeCommand(leaf: CliLeaf) {
  return defineCommand({
    args: commonArgs,
    subCommands: {
      list: leaf(
        { ...stringArgs("page-size", "page-token", "status") },
        episodeList,
      ),
      get: leaf({}, episodeGet, { parameters: ["<Episode reference>"] }),
      revision: leaf({}, episodeRevision, {
        parameters: ["<exact Episode revision reference>"],
      }),
      history: leaf(
        { ...stringArgs("page-size", "page-token") },
        episodeHistory,
        { parameters: ["<Episode reference>"] },
      ),
      create: leaf(
        { ...stringArgs("request-file", "operation-id") },
        episodeCreate,
      ),
      revise: leaf(
        {
          ...stringArgs(
            "request-file",
            "expected-epoch",
            "intent",
            "operation-id",
          ),
        },
        episodeRevise,
        { parameters: ["<Episode reference>"] },
      ),
      link: leaf(
        { ...stringArgs("from", "to", "relation", "operation-id") },
        episodeLink,
      ),
      suppress: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        episodeSuppress,
        { parameters: ["<Episode reference>"] },
      ),
      restore: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        episodeRestore,
        { parameters: ["<Episode reference>"] },
      ),
      withdraw: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        episodeWithdraw,
        { parameters: ["<Episode reference>"] },
      ),
      reaccept: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        episodeReaccept,
        { parameters: ["<Episode reference>"] },
      ),
      purge: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        episodePurge,
        { parameters: ["<Episode reference>"] },
      ),
    },
  });
}
