// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson, type JsonValue } from "@bufbuild/protobuf";
import { TemporalExtentSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  AddSchemaEvidenceRequestSchema,
  CognitiveSchemaContentSchema,
  CognitiveSchemaMutationRequestSchema,
  CreateCognitiveSchemaRequestSchema,
  GetCognitiveSchemaRequestSchema,
  MergeCognitiveSchemasRequestSchema,
  ReviseCognitiveSchemaRequestSchema,
  SchemaEvidenceLinkSchema,
  SchemaScopeSchema,
  SplitCognitiveSchemaRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { ObjectRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { z } from "zod";
import { nonnegativeInt64, CliError } from "../agent.js";
import { revisionBasis } from "../mutation-input.js";
import type { CliEnvironment } from "../runtime.js";

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
    value.evidence_links.map(async (item) =>
      create(SchemaEvidenceLinkSchema, {
        linkId: "",
        role: item.role,
        basis: await revisionBasis(env, {
          ref: item.ref,
          role: item.basis_role,
          epistemic_relation: item.epistemic_relation,
        }),
      }),
    ),
  );
  return create(CognitiveSchemaContentSchema, {
    title: value.title,
    structuralClaim: value.structural_claim,
    applicabilityScope: create(SchemaScopeSchema, {
      description: value.applicability_scope.description,
      aboutness,
      tags,
      validTime,
    }),
    boundaryDefinition: value.boundary_definition,
    formationKind: value.formation_kind,
    evidenceLinks,
  });
}

export async function schemaCommands(
  env: CliEnvironment,
  action?: string,
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
    return client.concepts.createSchema(
      create(CreateCognitiveSchemaRequestSchema, {
        operationId,
        subjectId,
        schema: await content(
          env,
          await requestPayload(values["request-file"]),
        ),
      }),
    );
  if (action === "get" || action === "revision") {
    const kind =
      action === "get" ? "cognitive_schema" : "cognitive_schema_revision";
    const ref = await resolveReference(
      required(argument, `${kind} reference`),
      kind,
    );
    if (action === "get")
      return client.concepts.getSchema(
        create(GetCognitiveSchemaRequestSchema, {
          subjectId,
          schemaId: ref.value,
        }),
      );
    return client.concepts.getSchemaRevision(
      create(ObjectRequestSchema, { subjectId, id: ref.value }),
    );
  }
  if (action === "evidence") {
    const ref = await resolveReference(
      required(argument, "CognitiveSchema reference"),
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
      .parse(await requestPayload(values["request-file"]));
    return client.concepts.addSchemaEvidence(
      create(AddSchemaEvidenceRequestSchema, {
        operationId,
        subjectId,
        schemaId: ref.value,
        expectedObjectEpoch: nonnegativeInt64(
          values["expected-epoch"],
          "--expected-epoch",
        ),
        link: create(SchemaEvidenceLinkSchema, {
          linkId: "",
          role: input.role,
          basis: await revisionBasis(env, {
            ref: input.ref,
            role: input.basis_role,
            epistemic_relation: input.epistemic_relation,
          }),
        }),
      }),
    );
  }
  if (action === "revise") {
    const ref = await resolveReference(
      required(argument, "CognitiveSchema reference"),
      "cognitive_schema",
    );
    const request = z
      .strictObject({
        intent: z.enum(["correct", "rephrase", "reinterpret"]),
        copy_link_ids: z.array(z.string().min(1).max(128)).max(256).default([]),
        schema: schemaContentInput,
      })
      .parse(await requestPayload(values["request-file"]));
    return client.concepts.reviseSchema(
      create(ReviseCognitiveSchemaRequestSchema, {
        operationId,
        subjectId,
        schemaId: ref.value,
        expectedObjectEpoch: nonnegativeInt64(
          values["expected-epoch"],
          "--expected-epoch",
        ),
        intent: request.intent,
        schema: await content(env, request.schema),
        copyLinkIds: request.copy_link_ids,
      }),
    );
  }
  if (action === "split") {
    const ref = await resolveReference(
      required(argument, "CognitiveSchema reference"),
      "cognitive_schema",
    );
    const request = z
      .strictObject({ children: z.array(schemaContentInput).min(2).max(32) })
      .parse(await requestPayload(values["request-file"]));
    return client.concepts.splitSchema(
      create(SplitCognitiveSchemaRequestSchema, {
        operationId,
        subjectId,
        schemaId: ref.value,
        expectedObjectEpoch: nonnegativeInt64(
          values["expected-epoch"],
          "--expected-epoch",
        ),
        children: await Promise.all(
          request.children.map((item) => content(env, item)),
        ),
      }),
    );
  }
  if (action === "merge") {
    const request = z
      .strictObject({
        schemas: z.array(reference).min(2).max(32),
        merged: schemaContentInput,
      })
      .parse(await requestPayload(values["request-file"]));
    const refs = await Promise.all(
      request.schemas.map((item) => resolveReference(item, "cognitive_schema")),
    );
    const current = await Promise.all(
      refs.map((ref) =>
        client.concepts.getSchema(
          create(GetCognitiveSchemaRequestSchema, {
            subjectId,
            schemaId: ref.value,
          }),
        ),
      ),
    );
    return client.concepts.mergeSchemas(
      create(MergeCognitiveSchemasRequestSchema, {
        operationId,
        subjectId,
        schemaIds: refs.map((ref) => ref.value),
        expectedObjectEpochs: current.map((item) => item.objectEpoch),
        merged: await content(env, request.merged),
      }),
    );
  }
  if (
    ["suppress", "restore", "withdraw", "reaccept", "purge"].includes(
      action ?? "",
    )
  ) {
    const ref = await resolveReference(
      required(argument, "CognitiveSchema reference"),
      "cognitive_schema",
    );
    const mutation = create(CognitiveSchemaMutationRequestSchema, {
      operationId,
      subjectId,
      schemaId: ref.value,
      expectedObjectEpoch: nonnegativeInt64(
        values["expected-epoch"],
        "--expected-epoch",
      ),
    });
    switch (action) {
      case "suppress":
        return client.concepts.suppressSchema(mutation);
      case "restore":
        return client.concepts.restoreSchema(mutation);
      case "withdraw":
        return client.concepts.withdrawSchema(mutation);
      case "reaccept":
        return client.concepts.reacceptSchema(mutation);
      case "purge":
        return client.concepts.purgeSchema(mutation);
    }
  }
  throw new CliError(
    "INVALID_ARGUMENT",
    "Use schema get|revision|create|evidence|revise|split|merge|suppress|restore|withdraw|reaccept|purge",
  );
}
