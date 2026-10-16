// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import {
  SocialContentSchema,
  AddSocialEvidenceRequestSchema,
  SocialSelectionRequestSchema,
  SocialPairRequestSchema,
  ResolveConventionsRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/social_pb.js";
import { CliError } from "../agent.js";
import { content, objectEpoch, operationId, page } from "../content.js";
import type { CliEnvironment } from "../runtime.js";
export async function socialCommands(
  env: CliEnvironment,
  action?: string,
  ref?: string,
) {
  const { client, values } = env;
  const subjectId = await env.subjectId;
  if (action === "list")
    return client.social.list({
      subjectId,
      family: values.kind,
      page: page(env),
    });
  if (["select", "neighborhood"].includes(action ?? ""))
    return client.social[action as "select" | "neighborhood"]({
      ...(await content(env, SocialSelectionRequestSchema)),
      subjectId,
    });
  if (action === "pair")
    return client.social.pair(await content(env, SocialPairRequestSchema));
  if (action === "resolve")
    return client.social.resolveConventions(
      await content(env, ResolveConventionsRequestSchema),
    );
  if (action === "adopt")
    return client.social.adopt({
      subjectId,
      operationId: operationId(env),
      content: await content(env, SocialContentSchema),
      reason: env.required(values.text, "--text <adoption reason>"),
    });
  const reference = await env.resolveReference(
    env.required(ref, "Typed Social object or exact revision reference"),
  );
  const revision = reference.kind.endsWith("_revision");
  const family = revision ? reference.kind.slice(0, -9) : reference.kind;
  if (!["social_relation", "language_convention"].includes(family))
    throw new CliError(
      "REFERENCE_TYPE_MISMATCH",
      "Social command requires a SocialRelation or LanguageConvention reference",
    );
  if (action === "revision") {
    if (!revision)
      throw new CliError(
        "REFERENCE_TYPE_MISMATCH",
        "Social revision requires an exact revision reference",
      );
    return client.social.getRevision({ subjectId, id: reference.value });
  }
  const expected = objectEpoch(values["expected-object-epoch"]);
  const needsRecord =
    revision ||
    expected === undefined ||
    ["get", "history", "revise", "evidence"].includes(action ?? "");
  const record = !needsRecord
    ? undefined
    : revision
      ? await client.social.getRevision({ subjectId, id: reference.value })
      : await client.social.get({ subjectId, id: reference.value });
  if (action === "get") return record;
  if (action === "history")
    return client.social.listRevisions({
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
    return client.social.revise({
      ...mutation,
      targetRevision: record!.reference,
      content: await content(env, SocialContentSchema),
      reason: env.required(values.text, "--text <revision reason>"),
    });
  if (action === "evidence") {
    const input = await content(env, AddSocialEvidenceRequestSchema);
    return client.social.addEvidence({
      ...mutation,
      targetRevision: record!.reference,
      role: input.role,
      basis: input.basis,
    });
  }
  if (action === "purge") return client.social.purge(mutation);
  if (["suppress", "restore", "withdraw", "reaccept"].includes(action ?? ""))
    return client.social.setLifecycle({ mutation, action });
  throw new CliError("INVALID_ARGUMENT", "Unknown Social command");
}
