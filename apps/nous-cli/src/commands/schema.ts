// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import {
  CognitiveSchemaContentSchema,
  MergeCognitiveSchemasRequestSchema,
  SchemaEvidenceLinkSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { lifecycle } from "../cognition-owner.js";
import { CliError } from "../agent.js";
import { content, objectEpoch, operationId, target } from "../content.js";
import type { CliEnvironment } from "../runtime.js";
export async function schemaCommands(
  env: CliEnvironment,
  action?: string,
  ref?: string,
) {
  const { client, subjectId } = env;
  if (action === "create")
    return client.concepts.createCognitiveSchema({
      subjectId,
      operationId: operationId(env),
      schema: await content(env, CognitiveSchemaContentSchema),
    });
  if (action === "revision")
    return client.concepts.getCognitiveSchemaRevision({
      subjectId,
      id: await target(env, ref, "cognitive_schema_revision"),
    });
  if (action === "merge") {
    const request = await content(env, MergeCognitiveSchemasRequestSchema);
    if (!request.expectedObjectEpochs.length)
      request.expectedObjectEpochs = await Promise.all(
        request.schemaIds.map(
          async (schemaId) =>
            (await client.concepts.getCognitiveSchema({ subjectId, schemaId }))
              .objectEpoch,
        ),
      );
    return client.concepts.mergeCognitiveSchemas({
      ...request,
      subjectId,
      operationId: operationId(env),
    });
  }
  const schemaId = await target(env, ref, "cognitive_schema");
  if (action === "get")
    return client.concepts.getCognitiveSchema({ subjectId, schemaId });
  const expectedObjectEpoch =
    objectEpoch(env.values["expected-object-epoch"]) ??
    (await client.concepts.getCognitiveSchema({ subjectId, schemaId }))
      .objectEpoch;
  const mutation = {
    subjectId,
    schemaId,
    expectedObjectEpoch,
    operationId: operationId(env),
  };
  if (action === "revise")
    return client.concepts.reviseCognitiveSchema({
      ...mutation,
      intent: env.required(env.values.kind, "--kind"),
      schema: await content(env, CognitiveSchemaContentSchema),
    });
  if (action === "evidence")
    return client.concepts.addSchemaEvidence({
      ...mutation,
      link: await content(env, SchemaEvidenceLinkSchema),
    });
  if (action === "split") {
    const file = (await env.requestPayload(env.values["request-file"])) as {
      children?: unknown[];
    };
    if (!Array.isArray(file.children))
      throw new CliError(
        "INVALID_ARGUMENT",
        "Schema split requires children content",
      );
    return client.concepts.splitCognitiveSchema({
      ...mutation,
      children: await Promise.all(
        file.children.map((value) =>
          content(env, CognitiveSchemaContentSchema, value),
        ),
      ),
    });
  }
  return lifecycle(action, mutation, {
    suppress: client.concepts.suppressCognitiveSchema,
    restore: client.concepts.restoreCognitiveSchema,
    withdraw: client.concepts.withdrawCognitiveSchema,
    reaccept: client.concepts.reacceptCognitiveSchema,
    purge: client.concepts.purgeCognitiveSchema,
  });
}
