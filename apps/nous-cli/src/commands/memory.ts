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

const semanticMemory = z
  .strictObject({
    intent: z.enum(["correct", "rephrase", "reinterpret"]),
    cognitive_role: z.enum([
      "experiential",
      "declarative",
      "procedural_experience",
    ]),
    formation_mode: z.enum(["grounded", "synthesized"]),
    grounding_occurrence: z.string().min(1).max(2048).optional(),
    semantic_role: z.string().min(1).max(128),
    text: z.string().min(1).max(65536),
    title: z.string().max(512).optional(),
    basis: z
      .array(
        z.strictObject({
          ref: z.string().min(1).max(2048),
          role: z
            .enum(["direct", "interpretation", "contextual"])
            .default("direct"),
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
        }),
      )
      .min(1)
      .max(256),
    aboutness: z.array(z.string().min(1).max(2048)).max(256).default([]),
    tags: z.array(z.string().min(1).max(2048)).max(256).default([]),
    valid_time: z.unknown().optional(),
    epistemic_class: z.enum([
      "observed",
      "reported",
      "derived",
      "inferred",
      "narrative",
      "simulated",
    ]),
  })
  .strict();

async function memoryCommon(
  env: CliEnvironment,
  action: string,
  argument?: string,
) {
  const common = await typedCognition.commonCommand(
    env,
    "memory",
    action,
    argument,
    { list: true, history: true, accessibility: true },
  );
  if (common.handled) return common.value;
  throw new CliError("INTERNAL_FAILURE", "Memory command is not handled");
}

const memoryList = (env: CliEnvironment) => memoryCommon(env, "list");
const memoryGet = (env: CliEnvironment, argument?: string) =>
  memoryCommon(env, "get", argument);
const memoryRevision = (env: CliEnvironment, argument?: string) =>
  memoryCommon(env, "revision", argument);
const memoryHistory = (env: CliEnvironment, argument?: string) =>
  memoryCommon(env, "history", argument);
const memorySuppress = (env: CliEnvironment, argument?: string) =>
  memoryCommon(env, "suppress", argument);
const memoryRestore = (env: CliEnvironment, argument?: string) =>
  memoryCommon(env, "restore", argument);
const memoryWithdraw = (env: CliEnvironment, argument?: string) =>
  memoryCommon(env, "withdraw", argument);
const memoryReaccept = (env: CliEnvironment, argument?: string) =>
  memoryCommon(env, "reaccept", argument);
const memoryPurge = (env: CliEnvironment, argument?: string) =>
  memoryCommon(env, "purge", argument);
const memoryAccessibility = (env: CliEnvironment, argument?: string) =>
  memoryCommon(env, "accessibility", argument);
const memoryRevise = async (env: CliEnvironment, argument?: string) => {
  const {
    client,
    values,
    subjectId,
    required,
    resolveReference,
    requestPayload,
  } = env;
  const ref = await resolveReference(
    required(argument, "memory reference"),
    "memory",
  );
  const parsed = semanticMemory.parse(
    await requestPayload(values["request-file"]),
  );
  const expectedObjectEpoch = nonnegativeInt64(
    values["expected-epoch"],
    "--expected-epoch",
  );
  const basis = await Promise.all(
    parsed.basis.map((item) => revisionBasis(env, item)),
  );
  const aboutness = await Promise.all(
    parsed.aboutness.map(
      async (text) => (await resolveReference(text, "entity")).value,
    ),
  );
  const tags = await Promise.all(
    parsed.tags.map(
      async (text) => (await resolveReference(text, "tag")).value,
    ),
  );
  const groundingOccurrenceId = parsed.grounding_occurrence
    ? (await resolveReference(parsed.grounding_occurrence, "occurrence")).value
    : undefined;
  const validTime = parsed.valid_time
    ? fromJson(TemporalExtentSchema, parsed.valid_time as JsonValue)
    : undefined;
  const input = {
    cognitiveRole: parsed.cognitive_role,
    formationMode: parsed.formation_mode,
    groundingOccurrenceId,
    semanticRole: parsed.semantic_role,
    text: parsed.text,
    title: parsed.title,
    basis,
    aboutness,
    tags,
    validTime,
    epistemicClass: parsed.epistemic_class,
  };
  return client.memory.revise({
    operationId: values["operation-id"] ?? crypto.randomUUID(),
    subjectId,
    memoryId: ref.value,
    expectedObjectEpoch,
    input,
    intent: parsed.intent,
  });
};
const memoryLink = async (env: CliEnvironment) => {
  const { client, values, subjectId, required, resolveReference } = env;
  const from = await resolveReference(
    required(values.from, "--from exact Memory revision"),
    "memory_revision",
  );
  const to = await resolveReference(
    required(values.to, "--to exact Memory revision"),
    "memory_revision",
  );
  return client.memory.linkRevisions({
    operationId: values["operation-id"] ?? crypto.randomUUID(),
    subjectId,
    fromRevisionId: from.value,
    toRevisionId: to.value,
    relation: required(values.relation, "--relation"),
  });
};

export function createMemoryCommand(leaf: CliLeaf) {
  return defineCommand({
    args: commonArgs,
    subCommands: {
      list: leaf(
        { ...stringArgs("page-size", "page-token", "status") },
        memoryList,
        { description: "List paged Memories for the selected Subject." },
      ),
      get: leaf({}, memoryGet, { parameters: ["<Memory reference>"] }),
      revision: leaf({}, memoryRevision, {
        parameters: ["<exact Memory revision reference>"],
      }),
      history: leaf(
        { ...stringArgs("page-size", "page-token") },
        memoryHistory,
        { parameters: ["<Memory reference>"] },
      ),
      revise: leaf(
        { ...stringArgs("request-file", "expected-epoch", "operation-id") },
        memoryRevise,
        {
          description:
            "Revise Memory using semantic TOML and the expected object epoch.",
          parameters: ["<Memory reference>"],
        },
      ),
      suppress: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        memorySuppress,
        { parameters: ["<Memory reference>"] },
      ),
      restore: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        memoryRestore,
        { parameters: ["<Memory reference>"] },
      ),
      withdraw: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        memoryWithdraw,
        { parameters: ["<Memory reference>"] },
      ),
      reaccept: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        memoryReaccept,
        { parameters: ["<Memory reference>"] },
      ),
      purge: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        memoryPurge,
        { parameters: ["<Memory reference>"] },
      ),
      accessibility: leaf(
        { ...stringArgs("mode", "expected-epoch", "operation-id") },
        memoryAccessibility,
        { parameters: ["<Memory reference>"] },
      ),
      link: leaf(
        { ...stringArgs("from", "to", "relation", "operation-id") },
        memoryLink,
      ),
    },
  });
}
