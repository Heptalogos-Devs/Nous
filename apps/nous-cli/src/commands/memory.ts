// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import { commonArgs, stringArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

import { fromJson, type JsonValue } from "@bufbuild/protobuf";
import { TemporalExtentSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { z } from "zod";
import { nonnegativeInt64 } from "../agent.js";
import { revisionBasis, revisionBasisInput } from "../mutation-input.js";
import type { CliEnvironment } from "../runtime.js";
import { cognitionSubcommands } from "./cognition.js";

const semanticMemory = z.strictObject({
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
  basis: z.array(revisionBasisInput).min(1).max(256),
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
});

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
  const cognition = cognitionSubcommands(leaf, {
    kind: "memory",
    label: "Memory",
    list: true,
    history: true,
    accessibility: true,
  });
  return defineCommand({
    args: commonArgs,
    subCommands: {
      ...cognition.reads,
      revise: leaf(
        { ...stringArgs("request-file", "expected-epoch", "operation-id") },
        memoryRevise,
        {
          description:
            "Revise Memory using semantic TOML and the expected object epoch.",
          parameters: ["<Memory reference>"],
        },
      ),
      ...cognition.lifecycle,
      link: leaf(
        { ...stringArgs("from", "to", "relation", "operation-id") },
        memoryLink,
      ),
    },
  });
}
