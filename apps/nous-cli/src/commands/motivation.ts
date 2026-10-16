// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import {
  MotivationContentSchema,
  EvaluateConditionRequestSchema,
  AddMotivationEvidenceRequestSchema,
  MotivationSelectionRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/motivation_pb.js";
import { CliError } from "../agent.js";
import { content, objectEpoch, operationId, page } from "../content.js";
import type { CliEnvironment } from "../runtime.js";
export async function motivationCommands(
  env: CliEnvironment,
  action?: string,
  ref?: string,
) {
  const { client, values } = env;
  const subjectId = await env.subjectId;
  if (action === "list")
    return client.motivation.list({
      subjectId,
      family: values.kind,
      page: page(env),
    });
  if (action === "select")
    return client.motivation.select({
      ...(await content(env, MotivationSelectionRequestSchema)),
      subjectId,
    });
  if (action === "evaluate")
    return client.motivation.evaluate({
      ...(await content(env, EvaluateConditionRequestSchema)),
      subjectId,
      operationId: operationId(env),
    });
  if (action === "evaluation")
    return client.motivation.getEvaluation({
      subjectId,
      id: env.required(ref, "Evaluation id"),
    });
  if (action === "adopt")
    return client.motivation.adopt({
      subjectId,
      operationId: operationId(env),
      content: await content(env, MotivationContentSchema),
      reason: env.required(values.text, "--text <adoption reason>"),
    });
  const reference = await env.resolveReference(
    env.required(ref, "Typed Motivation object or exact revision reference"),
  );
  const revision = reference.kind.endsWith("_revision");
  const family = revision ? reference.kind.slice(0, -9) : reference.kind;
  if (!["desire", "concern", "desired_condition"].includes(family))
    throw new CliError(
      "REFERENCE_TYPE_MISMATCH",
      "Motivation command requires a MotivationRelation or LanguageConvention reference",
    );
  if (action === "evaluations")
    return client.motivation.listEvaluations({
      subjectId,
      conditionRevision: reference,
      page: page(env),
    });
  if (action === "revision") {
    if (!revision)
      throw new CliError(
        "REFERENCE_TYPE_MISMATCH",
        "Motivation revision requires an exact revision reference",
      );
    return client.motivation.getRevision({ subjectId, id: reference.value });
  }
  const expected = objectEpoch(values["expected-object-epoch"]);
  const needsRecord =
    revision ||
    expected === undefined ||
    ["get", "history", "revise", "evidence"].includes(action ?? "");
  const record = !needsRecord
    ? undefined
    : revision
      ? await client.motivation.getRevision({ subjectId, id: reference.value })
      : await client.motivation.get({ subjectId, id: reference.value });
  if (action === "get") return record;
  if (action === "history")
    return client.motivation.listRevisions({
      subjectId,
      objectRef: record?.objectRef ?? reference,
      page: page(env),
    });
  const mutation = {
    subjectId,
    operationId: operationId(env),
    objectRef: record?.objectRef ?? reference,
    expectedObjectEpoch: expected ?? record!.objectEpoch,
  };
  if (action === "revise")
    return client.motivation.revise({
      ...mutation,
      targetRevision: record!.reference,
      content: await content(env, MotivationContentSchema),
      reason: env.required(values.text, "--text <revision reason>"),
    });
  if (action === "evidence") {
    const input = await content(env, AddMotivationEvidenceRequestSchema);
    return client.motivation.addEvidence({
      ...mutation,
      targetRevision: record!.reference,
      role: input.role,
      basis: input.basis,
    });
  }
  if (action === "purge") return client.motivation.purge(mutation);
  if (["suppress", "restore", "withdraw", "reaccept"].includes(action ?? ""))
    return client.motivation.setLifecycle({ mutation, action });
  throw new CliError("INVALID_ARGUMENT", "Unknown Motivation command");
}
