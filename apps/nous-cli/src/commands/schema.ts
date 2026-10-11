// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import { commonArgs, stringArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

import { fromJson, type JsonValue } from "@bufbuild/protobuf";
import { TemporalExtentSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { z } from "zod";
import { nonnegativeInt64, CliError } from "../agent.js";
import { revisionBasis } from "../mutation-input.js";
import type { CliEnvironment } from "../runtime.js";
import { typedCognition } from "./cognition.js";

const reference = z.string().min(1).max(2048);
const epistemicRelation = z
  .enum([
    "supports",
    "contradicts",
    "corroborates",
    "weakens",
    "corrects",
    "counterexample",
    "inferred_from",
  ])
  .optional();
const schemaContentInput = z.strictObject({
  title: z.string().max(512).default(""),
  structural_claim: z.string().min(1).max(8192),
  applicability_scope: z.strictObject({
    description: z.string().min(1).max(4096),
    aboutness: z.array(reference).max(256).default([]),
    tags: z.array(reference).max(256).default([]),
    valid_time: z.unknown().optional(),
  }),
  boundary_definition: z.string().min(1).max(8192),
  formation_kind: z
    .enum(["explicit_import", "synthesized"])
    .default("explicit_import"),
  evidence_links: z
    .array(
      z.strictObject({
        role: z.enum(["support", "counterexample", "boundary_case"]),
        ref: reference,
        basis_role: z
          .enum(["direct", "interpretation", "contextual"])
          .default("direct"),
        epistemic_relation: epistemicRelation,
      }),
    )
    .max(256)
    .default([]),
});

async function content(env: CliEnvironment, raw: unknown) {
  const value = schemaContentInput.parse(raw);
  const aboutness = await Promise.all(
    value.applicability_scope.aboutness.map(
      async (item) => (await env.resolveReference(item, "entity")).value,
    ),
  );
  const tags = await Promise.all(
    value.applicability_scope.tags.map(
      async (item) => (await env.resolveReference(item, "tag")).value,
    ),
  );
  const validTime = value.applicability_scope.valid_time
    ? fromJson(
        TemporalExtentSchema,
        value.applicability_scope.valid_time as JsonValue,
      )
    : undefined;
  const evidenceLinks = await Promise.all(
    value.evidence_links.map(async (item) => ({
      linkId: "",
      role: item.role,
      basis: await revisionBasis(env, {
        ref: item.ref,
        role: item.basis_role,
        epistemic_relation: item.epistemic_relation,
      }),
    })),
  );
  return {
    title: value.title,
    structuralClaim: value.structural_claim,
    applicabilityScope: {
      description: value.applicability_scope.description,
      aboutness,
      tags,
      validTime,
    },
    boundaryDefinition: value.boundary_definition,
    formationKind: value.formation_kind,
    evidenceLinks,
  };
}

async function schemaCommon(
  env: CliEnvironment,
  action: string,
  argument?: string,
) {
  const common = await typedCognition.commonCommand(
    env,
    "cognitive_schema",
    action,
    argument,
  );
  if (common.handled) return common.value;
  throw new CliError(
    "INTERNAL_FAILURE",
    "CognitiveSchema command is not handled",
  );
}

const schemaGet = (env: CliEnvironment, argument?: string) =>
  schemaCommon(env, "get", argument);
const schemaRevision = (env: CliEnvironment, argument?: string) =>
  schemaCommon(env, "revision", argument);
const schemaSuppress = (env: CliEnvironment, argument?: string) =>
  schemaCommon(env, "suppress", argument);
const schemaRestore = (env: CliEnvironment, argument?: string) =>
  schemaCommon(env, "restore", argument);
const schemaWithdraw = (env: CliEnvironment, argument?: string) =>
  schemaCommon(env, "withdraw", argument);
const schemaReaccept = (env: CliEnvironment, argument?: string) =>
  schemaCommon(env, "reaccept", argument);
const schemaPurge = (env: CliEnvironment, argument?: string) =>
  schemaCommon(env, "purge", argument);
const schemaCreate = async (env: CliEnvironment) =>
  env.client.concepts.createSchema({
    operationId: env.values["operation-id"] ?? crypto.randomUUID(),
    subjectId: env.subjectId,
    schema: await content(
      env,
      await env.requestPayload(env.values["request-file"]),
    ),
  });
const schemaEvidence = async (env: CliEnvironment, argument?: string) => {
  const ref = await env.resolveReference(
    env.required(argument, "CognitiveSchema reference"),
    "cognitive_schema",
  );
  const input = z
    .strictObject({
      role: z.enum(["support", "counterexample", "boundary_case"]),
      ref: reference,
      basis_role: z
        .enum(["direct", "interpretation", "contextual"])
        .default("direct"),
      epistemic_relation: epistemicRelation,
    })
    .parse(await env.requestPayload(env.values["request-file"]));
  return env.client.concepts.addSchemaEvidence({
    operationId: env.values["operation-id"] ?? crypto.randomUUID(),
    subjectId: env.subjectId,
    schemaId: ref.value,
    expectedObjectEpoch: nonnegativeInt64(
      env.values["expected-epoch"],
      "--expected-epoch",
    ),
    link: {
      linkId: "",
      role: input.role,
      basis: await revisionBasis(env, {
        ref: input.ref,
        role: input.basis_role,
        epistemic_relation: input.epistemic_relation,
      }),
    },
  });
};
const schemaRevise = async (env: CliEnvironment, argument?: string) => {
  const ref = await env.resolveReference(
    env.required(argument, "CognitiveSchema reference"),
    "cognitive_schema",
  );
  const request = z
    .strictObject({
      intent: z.enum(["correct", "rephrase", "reinterpret"]),
      copy_link_ids: z.array(z.string().min(1).max(128)).max(256).default([]),
      schema: schemaContentInput,
    })
    .parse(await env.requestPayload(env.values["request-file"]));
  return env.client.concepts.reviseSchema({
    operationId: env.values["operation-id"] ?? crypto.randomUUID(),
    subjectId: env.subjectId,
    schemaId: ref.value,
    expectedObjectEpoch: nonnegativeInt64(
      env.values["expected-epoch"],
      "--expected-epoch",
    ),
    intent: request.intent,
    schema: await content(env, request.schema),
    copyLinkIds: request.copy_link_ids,
  });
};
const schemaSplit = async (env: CliEnvironment, argument?: string) => {
  const ref = await env.resolveReference(
    env.required(argument, "CognitiveSchema reference"),
    "cognitive_schema",
  );
  const request = z
    .strictObject({ children: z.array(schemaContentInput).min(2).max(32) })
    .parse(await env.requestPayload(env.values["request-file"]));
  return env.client.concepts.splitSchema({
    operationId: env.values["operation-id"] ?? crypto.randomUUID(),
    subjectId: env.subjectId,
    schemaId: ref.value,
    expectedObjectEpoch: nonnegativeInt64(
      env.values["expected-epoch"],
      "--expected-epoch",
    ),
    children: await Promise.all(
      request.children.map((item) => content(env, item)),
    ),
  });
};
const schemaMerge = async (env: CliEnvironment) => {
  const request = z
    .strictObject({
      schemas: z.array(reference).min(2).max(32),
      merged: schemaContentInput,
    })
    .parse(await env.requestPayload(env.values["request-file"]));
  const refs = await Promise.all(
    request.schemas.map((item) =>
      env.resolveReference(item, "cognitive_schema"),
    ),
  );
  const current = await Promise.all(
    refs.map((ref) =>
      typedCognition.read(env, { kind: "cognitive_schema", value: ref.value }),
    ),
  );
  return env.client.concepts.mergeSchemas({
    operationId: env.values["operation-id"] ?? crypto.randomUUID(),
    subjectId: env.subjectId,
    schemaIds: refs.map((ref) => ref.value),
    expectedObjectEpochs: current.map((item) => item.header?.objectEpoch ?? 0n),
    merged: await content(env, request.merged),
  });
};

export function createSchemaCommand(leaf: CliLeaf) {
  return defineCommand({
    args: commonArgs,
    subCommands: {
      create: leaf(
        { ...stringArgs("request-file", "operation-id") },
        schemaCreate,
      ),
      get: leaf({}, schemaGet, { parameters: ["<CognitiveSchema reference>"] }),
      revision: leaf({}, schemaRevision, {
        parameters: ["<exact CognitiveSchema revision reference>"],
      }),
      evidence: leaf(
        { ...stringArgs("request-file", "expected-epoch", "operation-id") },
        schemaEvidence,
        { parameters: ["<CognitiveSchema reference>"] },
      ),
      revise: leaf(
        { ...stringArgs("request-file", "expected-epoch", "operation-id") },
        schemaRevise,
        { parameters: ["<CognitiveSchema reference>"] },
      ),
      split: leaf(
        { ...stringArgs("request-file", "expected-epoch", "operation-id") },
        schemaSplit,
        { parameters: ["<CognitiveSchema reference>"] },
      ),
      merge: leaf(
        { ...stringArgs("request-file", "operation-id") },
        schemaMerge,
      ),
      suppress: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        schemaSuppress,
        { parameters: ["<CognitiveSchema reference>"] },
      ),
      restore: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        schemaRestore,
        { parameters: ["<CognitiveSchema reference>"] },
      ),
      withdraw: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        schemaWithdraw,
        { parameters: ["<CognitiveSchema reference>"] },
      ),
      reaccept: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        schemaReaccept,
        { parameters: ["<CognitiveSchema reference>"] },
      ),
      purge: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        schemaPurge,
        { parameters: ["<CognitiveSchema reference>"] },
      ),
    },
  });
}
