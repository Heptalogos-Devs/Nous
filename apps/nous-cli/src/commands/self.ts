// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { toJson } from "@bufbuild/protobuf";
import { DomainTemporalExtentSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  SelfScopeSchema,
  SelfContentSchema,
  AddSelfEvidenceRequestSchema,
  type SelfRecord,
} from "@nous-wave/protocol/nous/wave/v1alpha1/self_pb.js";
import { InterpretSelfRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
import { CliError } from "../agent.js";
import { content, objectEpoch, operationId, page } from "../content.js";
import type { CliEnvironment } from "../runtime.js";

function soul(records: SelfRecord[]) {
  return records
    .filter(
      (record) =>
        record.acceptanceState === "accepted" &&
        record.integrityState === "valid" &&
        record.suppressionState === "normal",
    )
    .map((record) => {
      const cognition = record.content?.content;
      if (!cognition?.case) return "";
      const value = cognition.value;
      const body =
        cognition.case === "facet"
          ? [
              `## ${cognition.value.kind}: ${cognition.value.statement}`,
              `立场：${cognition.value.stance}；认识地位：${value.epistemicClass}`,
              `适用范围：${cognition.value.scope ? JSON.stringify(toJson(SelfScopeSchema, cognition.value.scope)) : "general"}`,
              ...cognition.value.exceptions.map(
                (exception) => `反例或边界：${exception.statement}`,
              ),
            ]
          : [
              `## ${cognition.value.theme}`,
              `认识地位：${value.epistemicClass}`,
              cognition.value.interpretation,
            ];
      return [
        ...body,
        `有效时间：${value.validTime ? JSON.stringify(toJson(DomainTemporalExtentSchema, value.validTime)) : "unknown"}`,
        `精确修订：${record.reference!.kind}:${record.reference!.value}`,
      ].join("\n\n");
    })
    .join("\n\n");
}
export async function selfCommands(
  env: CliEnvironment,
  action?: string,
  ref?: string,
) {
  const { client, values } = env;
  const subjectId = await env.subjectId;
  if (action === "list")
    return client.self.list({
      subjectId,
      family: values.kind,
      page: page(env),
    });
  if (action === "soul") {
    const listing = await client.self.list({
      subjectId,
      page: page(env),
    });
    return {
      subjectId,
      text: [
        "# 当前 soul（当前管理页）",
        "此页派生自正式对象；修改需显式提案。条件和有效时间随内容保留，后续页由 nextPageToken 读取。",
        soul(listing.items),
      ].join("\n\n"),
      nextPageToken: listing.nextPageToken,
    };
  }
  if (action === "interpret") {
    const request = await content(env, InterpretSelfRequestSchema);
    return client.model.interpretSelf({
      ...request,
      subjectId,
      operationId: operationId(env),
    });
  }
  if (action === "interpret-seed") {
    const seed = await client.subjects.getCognitiveSeed({ subjectId });
    return client.model.interpretSelf({
      subjectId,
      operationId: operationId(env),
      source: { case: "seedVersionId", value: seed.seedVersionId },
    });
  }
  if (action === "adopt")
    return client.self.adopt({
      subjectId,
      operationId: operationId(env),
      content: await content(env, SelfContentSchema),
      reason: env.required(values.text, "--text <adoption reason>"),
    });
  const reference = await env.resolveReference(
    env.required(ref, "Typed Self object or exact revision reference"),
  );
  const revision = reference.kind.endsWith("_revision");
  const family = revision ? reference.kind.slice(0, -9) : reference.kind;
  if (!["self_facet", "narrative_thread"].includes(family))
    throw new CliError(
      "REFERENCE_TYPE_MISMATCH",
      "Self command requires a SelfFacet or NarrativeThread reference",
    );
  if (action === "revision") {
    if (!revision)
      throw new CliError(
        "REFERENCE_TYPE_MISMATCH",
        "Self revision requires an exact revision reference",
      );
    return client.self.getRevision({ subjectId, id: reference.value });
  }
  const expected = objectEpoch(values["expected-object-epoch"]);
  const needsRecord =
    revision ||
    expected === undefined ||
    ["get", "history", "revise", "evidence"].includes(action ?? "");
  const record = !needsRecord
    ? undefined
    : revision
      ? await client.self.getRevision({ subjectId, id: reference.value })
      : await client.self.get({ subjectId, id: reference.value });
  if (action === "get") return record;
  if (action === "history")
    return client.self.listRevisions({
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
    return client.self.revise({
      ...mutation,
      targetRevision: record!.reference,
      content: await content(env, SelfContentSchema),
      reason: env.required(values.text, "--text <revision reason>"),
    });
  if (action === "evidence") {
    const input = await content(env, AddSelfEvidenceRequestSchema);
    return client.self.addEvidence({
      ...mutation,
      targetRevision: record!.reference,
      role: input.role,
      basis: input.basis,
    });
  }
  if (action === "purge") return client.self.purge(mutation);
  if (["suppress", "restore", "withdraw", "reaccept"].includes(action ?? ""))
    return client.self.setLifecycle({ mutation, action });
  throw new CliError("INVALID_ARGUMENT", "Unknown Self command");
}
