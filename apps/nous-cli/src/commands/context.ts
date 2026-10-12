// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create } from "@bufbuild/protobuf";
import {
  CognitiveRefSchema,
  CreateWorkContextRequestSchema,
  WorkContextFieldsSchema,
  WorkContextAnchorsSchema,
  WorkContextClearScope,
  type UpdateWorkContextRequest,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { content, page } from "../content.js";
import type { CliEnvironment } from "../runtime.js";
import { boundedInteger, CliError } from "../agent.js";
export async function contextCommands(
  env: CliEnvironment,
  action?: string,
  argument?: string,
) {
  const { client, state, subjectId, values, required, save, resolveReference } =
    env;
  const operationId = values["operation-id"] ?? crypto.randomUUID();
  async function text() {
    let value = values.text;
    if (values.file) value = await env.readText(values.file);
    return value;
  }
  if (action === "list")
    return client.runtime.listWorkContexts({
      subjectId,
      page: page(env),
    });
  if (action === "create") {
    const result = await client.runtime.createWorkContext(
      env.values["request-file"]
        ? {
            ...(await content(env, CreateWorkContextRequestSchema)),
            subjectId,
            operationId,
          }
        : {
            subjectId,
            operationId,
            purpose: required(values.purpose ?? values.text, "--purpose"),
            contextText: await text(),
          },
    );
    if (result.workContext)
      await save({ ...state, workContextId: result.workContext.workContextId });
    return result.workContext;
  }
  if (action === "foreground") {
    const sessionId = required(state.sessionId, "Selected Session");
    const session = await client.runtime.getSession({
      subjectId,
      id: sessionId,
    });
    return client.runtime.setActiveWorkContext({
      subjectId,
      sessionId,
      expectedRuntimeRevision: session.runtimeRevision,
      workContextId: values.clear
        ? undefined
        : argument
          ? (await resolveReference(argument, "work_context")).value
          : required(state.workContextId, "WorkContext"),
      operationId,
    });
  }
  const id = argument
    ? (await resolveReference(argument, "work_context")).value
    : required(state.workContextId, "WorkContext");
  const expected = values["expected-revision"];
  const current =
    expected === undefined || action === "show" || action === "select"
      ? (await client.runtime.getWorkContext({ subjectId, workContextId: id }))
          .workContext
      : undefined;
  if (
    (action === "show" || action === "select" || expected === undefined) &&
    !current
  )
    throw new CliError("NOT_FOUND", "WorkContext not found");
  if (action === "select") {
    await save({ ...state, workContextId: id });
    return current;
  }
  if (action === "show") return current;
  const mutation = {
    subjectId,
    workContextId: id,
    expectedRevision:
      expected === undefined
        ? current!.revision
        : BigInt(
            boundedInteger(
              expected,
              1,
              Number.MAX_SAFE_INTEGER,
              "--expected-revision",
            ),
          ),
    operationId,
  };
  if (action === "pause") return client.runtime.pauseWorkContext(mutation);
  if (action === "resume") return client.runtime.resumeWorkContext(mutation);
  if (action === "end") return client.runtime.endWorkContext(mutation);
  let intent: UpdateWorkContextRequest["intent"];
  if (action === "set") {
    const fields = values["request-file"]
      ? await content(env, WorkContextFieldsSchema)
      : create(WorkContextFieldsSchema);
    const nextText = await text();
    if (nextText !== undefined) fields.contextText = nextText;
    if (values.purpose !== undefined) fields.purpose = values.purpose;
    intent = { case: "fields", value: fields };
  } else if (action === "clear") {
    const scope = values.scope ?? "all";
    if (!["text", "anchors", "all"].includes(scope))
      throw new CliError(
        "INVALID_ARGUMENT",
        "--scope must be text, anchors or all",
      );
    intent = {
      case: "clear",
      value:
        scope === "text"
          ? WorkContextClearScope.TEXT
          : scope === "anchors"
            ? WorkContextClearScope.ANCHORS
            : WorkContextClearScope.ALL,
    };
  } else if (action === "pin" || action === "unpin") {
    const entries = [
      ...(values.cognition
        ?.split(",")
        .map((value) => ({ text: value, kind: "" })) ?? []),
      ...(values.entity
        ?.split(",")
        .map((value) => ({ text: value, kind: "entity" })) ?? []),
      ...(values.tag?.map((value) => ({ text: value, kind: "tag" })) ?? []),
    ];
    if (!entries.length)
      throw new CliError(
        "INVALID_ARGUMENT",
        "Provide --cognition, --entity or --tag",
      );
    const anchors = create(WorkContextAnchorsSchema);
    for (const entry of entries) {
      const ref = await resolveReference(entry.text, entry.kind);
      if (ref.kind === "entity" || ref.kind === "tag") {
        const key = ref.kind === "entity" ? "entityAnchors" : "tagAnchors";
        anchors[key].push(ref.value);
      } else if (
        [
          "memory_revision",
          "cognitive_schema_revision",
          "episode_revision",
          "journal_revision",
          "occurrence",
        ].includes(ref.kind)
      ) {
        anchors.cognitionAnchors.push(create(CognitiveRefSchema, ref));
      } else
        throw new CliError(
          "REFERENCE_TYPE_MISMATCH",
          "Pin exact cognition revisions, occurrences, Entities or Tags",
        );
    }
    intent = { case: action, value: anchors };
  } else throw new CliError("INVALID_ARGUMENT", "Unknown context command");
  return client.runtime.updateWorkContext({ ...mutation, intent });
}
