// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import type { CliEnvironment } from "../runtime.js";
import { CliError } from "../agent.js";
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
    if (value !== undefined && Buffer.byteLength(value) > 65536)
      throw new CliError("INVALID_ARGUMENT", "Context text exceeds 64 KiB");
    return value;
  }
  if (action === "list")
    return client.cognition.listWorkContexts({
      subjectId,
      page: { pageSize: 50 },
    });
  if (action === "create") {
    const result = await client.cognition.createWorkContext({
      subjectId,
      operationId,
      purpose: required(values.purpose ?? values.text, "--purpose"),
      contextText: await text(),
    });
    if (result.workContext)
      await save({ ...state, workContextId: result.workContext.workContextId });
    return result.workContext;
  }
  if (action === "foreground") {
    const sessionId = required(state.sessionId, "Selected Session");
    const session = await client.cognition.getSession({
      subjectId,
      id: sessionId,
    });
    return client.cognition.setActiveWorkContext({
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
  const result = await client.cognition.getWorkContext({
    subjectId,
    workContextId: id,
  });
  const current = result.workContext;
  if (!current) throw new CliError("NOT_FOUND", "WorkContext not found");
  if (action === "select") {
    await save({ ...state, workContextId: id });
    return current;
  }
  if (action === "show") return current;
  const mutation = {
    subjectId,
    workContextId: id,
    expectedRevision: current.revision,
    operationId,
  };
  if (action === "pause") return client.cognition.pauseWorkContext(mutation);
  if (action === "resume") return client.cognition.resumeWorkContext(mutation);
  if (action === "end") return client.cognition.endWorkContext(mutation);
  const patch = {
    ...mutation,
    purpose: current.purpose,
    contextText: current.contextText,
    cognitionAnchors: [...current.cognitionAnchors],
    entityAnchors: [...current.entityAnchors],
    tagAnchors: [...current.tagAnchors],
    unresolvedQuestions: current.unresolvedQuestions,
    constraints: current.constraints,
    resumeConditions: current.resumeConditions,
    budgetSummary: current.budgetSummary,
  };
  if (action === "set") {
    const nextText = await text();
    if (nextText === undefined && !values.purpose)
      throw new CliError(
        "INVALID_ARGUMENT",
        "Provide --text, --file or --purpose",
      );
    if (nextText !== undefined) patch.contextText = nextText;
    if (values.purpose) patch.purpose = values.purpose;
  } else if (action === "clear") {
    const scope = values.scope ?? "all";
    if (!["text", "anchors", "all"].includes(scope))
      throw new CliError(
        "INVALID_ARGUMENT",
        "--scope must be text, anchors or all",
      );
    if (scope !== "anchors") patch.contextText = "";
    if (scope !== "text") {
      patch.cognitionAnchors = [];
      patch.entityAnchors = [];
      patch.tagAnchors = [];
    }
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
    for (const entry of entries) {
      const ref = await resolveReference(entry.text, entry.kind);
      if (ref.kind === "entity" || ref.kind === "tag") {
        const key = ref.kind === "entity" ? "entityAnchors" : "tagAnchors";
        patch[key] = patch[key].filter((value) => value !== ref.value);
        if (action === "pin") patch[key].push(ref.value);
      } else if (
        [
          "memory_revision",
          "cognitive_schema_revision",
          "episode_revision",
          "journal_revision",
          "occurrence",
        ].includes(ref.kind)
      ) {
        patch.cognitionAnchors = patch.cognitionAnchors.filter(
          (value) => value.kind !== ref.kind || value.value !== ref.value,
        );
        if (action === "pin") patch.cognitionAnchors.push(ref);
      } else
        throw new CliError(
          "REFERENCE_TYPE_MISMATCH",
          "Pin exact cognition revisions, occurrences, Entities or Tags",
        );
    }
  } else throw new CliError("INVALID_ARGUMENT", "Unknown context command");
  return client.cognition.updateWorkContext(patch);
}
