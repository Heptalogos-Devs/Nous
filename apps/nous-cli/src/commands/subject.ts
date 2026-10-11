// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import { commonArgs, stringArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
import { z } from "zod";

const subjectInput = z.strictObject({
  cognitive_seed: z.strictObject({
    text: z.string().min(1).max(65536),
    format: z.string().min(1).max(256),
    provenance: z.record(z.string(), z.json()).optional(),
  }),
  metadata: z.record(z.string(), z.json()).optional(),
  capabilities: z.strictObject({ memory: z.boolean() }).optional(),
});
const adoptionInput = z.strictObject({
  text: z.string().min(1).max(65536),
  format: z.string().min(1).max(256).optional(),
  provenance: z.record(z.string(), z.json()).optional(),
  kind: z.enum(["initial", "import"]),
});

async function subjectId(env: CliEnvironment, argument?: string) {
  return (
    await env.resolveReference(
      env.required(argument, "Subject reference"),
      "subject",
    )
  ).value;
}

const listSubjects = (env: CliEnvironment) => env.client.subjects.list({});
const getSubject = async (env: CliEnvironment, argument?: string) =>
  env.client.subjects.get({ subjectId: await subjectId(env, argument) });
const getSubjectSeed = async (env: CliEnvironment, argument?: string) =>
  env.client.subjects.seed({ subjectId: await subjectId(env, argument) });
const adoptSubjectSeed = async (env: CliEnvironment, argument?: string) => {
  const input = adoptionInput.parse(
    await env.requestPayload(env.values["request-file"]),
  );
  return env.client.subjects.adoptSeed({
    subjectId: await subjectId(env, argument),
    operationId: env.values["operation-id"] ?? crypto.randomUUID(),
    seed: {
      text: input.text,
      format:
        input.format ??
        "application/vnd.nous-wave.cognitive-seed+toml;version=1",
      provenance: input.provenance,
    },
    kind: input.kind,
  });
};
const createSubject = async (env: CliEnvironment) => {
  const { client, save, values, requestPayload } = env;
  const requestFile = values["request-file"];
  if (!requestFile)
    throw new CliError(
      "INVALID_ARGUMENT",
      "subject create requires --request-file with a [cognitive_seed] section containing text and format",
    );
  const parsed = subjectInput.safeParse(await requestPayload(requestFile));
  if (!parsed.success)
    throw new CliError(
      "INVALID_ARGUMENT",
      "subject request TOML must include [cognitive_seed].text and [cognitive_seed].format, plus optional metadata and capabilities",
    );
  const request = parsed.data;
  const subject = await client.subjects.create({
    subjectId: crypto.randomUUID(),
    operationId: crypto.randomUUID(),
    ...(request.capabilities ? { capabilities: request.capabilities } : {}),
    cognitiveSeed: {
      text: request.cognitive_seed.text,
      format: request.cognitive_seed.format,
      provenance: request.cognitive_seed.provenance,
    },
    ...(request.metadata ? { metadata: request.metadata } : {}),
  });
  await save({
    subjectId: subject.subjectId,
  });
  return subject;
};
const useSubject = async (env: CliEnvironment, argument?: string) => {
  const subject = await env.client.subjects.get({
    subjectId: await subjectId(env, argument),
  });
  await env.save({
    subjectId: subject.subjectId,
  });
  return subject;
};

export function createSubjectCommand(leaf: CliLeaf) {
  return defineCommand({
    args: commonArgs,
    subCommands: {
      create: leaf({ ...stringArgs("request-file") }, createSubject, {
        description:
          "Create a Subject with a stable ID and an explicit valid Cognitive Seed frozen before the operation receipt; metadata and capabilities are optional.",
        parameters: ["--request-file <semantic TOML> (required)"],
      }),
      list: leaf({}, listSubjects),
      get: leaf({}, getSubject, {
        parameters: ["<Subject reference>"],
      }),
      seed: leaf({}, getSubjectSeed, {
        parameters: ["<Subject reference>"],
      }),
      "adopt-seed": leaf(
        { ...stringArgs("request-file", "operation-id") },
        adoptSubjectSeed,
        {
          description:
            "Adopt a TOML Cognitive Seed using an explicit initial or import kind.",
          parameters: ["<Subject reference>"],
        },
      ),
      use: leaf({}, useSubject, {
        parameters: ["<Subject reference>"],
      }),
    },
  });
}
