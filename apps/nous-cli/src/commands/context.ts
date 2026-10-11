// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import {
  booleanArgs,
  commaSeparatedArgs,
  commonArgs,
  stringArgs,
} from "./command-args.js";
import type { CliLeaf } from "./command-types.js";
import type { CliEnvironment } from "../runtime.js";
import { CliError } from "../agent.js";
import { create } from "@bufbuild/protobuf";
import { CognitiveRefSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
const contextOperationId = (env: CliEnvironment) =>
  env.values["operation-id"] ?? crypto.randomUUID();

async function contextText(env: CliEnvironment) {
  let value = env.values.text;
  if (env.values.file) value = await env.readText(env.values.file);
  if (value !== undefined && Buffer.byteLength(value) > 65536)
    throw new CliError("INVALID_ARGUMENT", "Context text exceeds 64 KiB");
  return value;
}

async function workContext(env: CliEnvironment, argument?: string) {
  const id = argument
    ? (await env.resolveReference(argument, "work_context")).value
    : env.required(env.state.workContextId, "WorkContext");
  const result = await env.client.cognition.getWorkContext({
    subjectId: env.subjectId,
    workContextId: id,
  });
  if (!result.workContext)
    throw new CliError("NOT_FOUND", "WorkContext not found");
  return { id, current: result.workContext };
}

async function workContextMutation(env: CliEnvironment, argument?: string) {
  const { id, current } = await workContext(env, argument);
  return {
    client: env.client,
    mutation: {
      subjectId: env.subjectId,
      workContextId: id,
      expectedRevision: current.revision,
      operationId: contextOperationId(env),
    },
  };
}

async function workContextPatch(env: CliEnvironment, argument?: string) {
  const { id, current } = await workContext(env, argument);
  return {
    subjectId: env.subjectId,
    workContextId: id,
    expectedRevision: current.revision,
    operationId: contextOperationId(env),
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
}

async function changeAnchors(
  env: CliEnvironment,
  patch: Awaited<ReturnType<typeof workContextPatch>>,
  add: boolean,
) {
  const { values, resolveReference } = env;
  const entries = [
    ...(values.cognition?.split(",").map((text) => ({ text, kind: "" })) ?? []),
    ...(values.entity?.split(",").map((text) => ({ text, kind: "entity" })) ??
      []),
    ...(values.tag?.map((text) => ({ text, kind: "tag" })) ?? []),
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
      if (add) patch[key].push(ref.value);
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
      if (add)
        patch.cognitionAnchors.push(
          create(CognitiveRefSchema, { kind: ref.kind, value: ref.value }),
        );
    } else
      throw new CliError(
        "REFERENCE_TYPE_MISMATCH",
        "Pin exact cognition revisions, occurrences, Entities or Tags",
      );
  }
}

const contextList = (env: CliEnvironment) =>
  env.client.cognition.listWorkContexts({
    subjectId: env.subjectId,
    page: { pageSize: 50 },
  });
const contextCreate = async (env: CliEnvironment) => {
  const result = await env.client.cognition.createWorkContext({
    subjectId: env.subjectId,
    operationId: contextOperationId(env),
    purpose: env.required(env.values.purpose ?? env.values.text, "--purpose"),
    contextText: await contextText(env),
  });
  if (result.workContext)
    await env.save({
      ...env.state,
      workContextId: result.workContext.workContextId,
    });
  return result.workContext;
};
const contextForeground = async (env: CliEnvironment, argument?: string) => {
  const sessionId = env.required(env.state.sessionId, "Selected Session");
  const session = await env.client.cognition.getSession({
    subjectId: env.subjectId,
    id: sessionId,
  });
  return env.client.cognition.setActiveWorkContext({
    subjectId: env.subjectId,
    sessionId,
    expectedRuntimeRevision: session.runtimeRevision,
    workContextId: env.values.clear
      ? undefined
      : argument
        ? (await env.resolveReference(argument, "work_context")).value
        : env.required(env.state.workContextId, "WorkContext"),
    operationId: contextOperationId(env),
  });
};
const contextSelect = async (env: CliEnvironment, argument?: string) => {
  const { id, current } = await workContext(env, argument);
  await env.save({ ...env.state, workContextId: id });
  return current;
};
const contextShow = async (env: CliEnvironment, argument?: string) =>
  (await workContext(env, argument)).current;
const contextPause = async (env: CliEnvironment, argument?: string) => {
  const { client, mutation } = await workContextMutation(env, argument);
  return client.cognition.pauseWorkContext(mutation);
};
const contextResume = async (env: CliEnvironment, argument?: string) => {
  const { client, mutation } = await workContextMutation(env, argument);
  return client.cognition.resumeWorkContext(mutation);
};
const contextEnd = async (env: CliEnvironment, argument?: string) => {
  const { client, mutation } = await workContextMutation(env, argument);
  return client.cognition.endWorkContext(mutation);
};
const contextSet = async (env: CliEnvironment, argument?: string) => {
  const patch = await workContextPatch(env, argument);
  const nextText = await contextText(env);
  if (nextText === undefined && !env.values.purpose)
    throw new CliError(
      "INVALID_ARGUMENT",
      "Provide --text, --file or --purpose",
    );
  if (nextText !== undefined) patch.contextText = nextText;
  if (env.values.purpose) patch.purpose = env.values.purpose;
  return env.client.cognition.updateWorkContext(patch);
};
const contextClear = async (env: CliEnvironment, argument?: string) => {
  const patch = await workContextPatch(env, argument);
  const scope = env.values.scope ?? "all";
  if (!(scope === "text" || scope === "anchors" || scope === "all"))
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
  return env.client.cognition.updateWorkContext(patch);
};
const contextPin = async (env: CliEnvironment, argument?: string) => {
  const patch = await workContextPatch(env, argument);
  await changeAnchors(env, patch, true);
  return env.client.cognition.updateWorkContext(patch);
};
const contextUnpin = async (env: CliEnvironment, argument?: string) => {
  const patch = await workContextPatch(env, argument);
  await changeAnchors(env, patch, false);
  return env.client.cognition.updateWorkContext(patch);
};

export function createContextCommand(leaf: CliLeaf) {
  return defineCommand({
    args: commonArgs,
    subCommands: {
      create: leaf(
        { ...stringArgs("purpose", "text", "file", "operation-id") },
        contextCreate,
      ),
      list: leaf({}, contextList),
      show: leaf({}, contextShow, { parameters: ["[WorkContext reference]"] }),
      set: leaf(
        { ...stringArgs("purpose", "text", "file", "operation-id") },
        contextSet,
        { parameters: ["[WorkContext reference]"] },
      ),
      pin: leaf(
        {
          ...stringArgs("cognition", "entity", "operation-id"),
          ...commaSeparatedArgs("tag"),
        },
        contextPin,
      ),
      unpin: leaf(
        {
          ...stringArgs("cognition", "entity", "operation-id"),
          ...commaSeparatedArgs("tag"),
        },
        contextUnpin,
      ),
      clear: leaf({ ...stringArgs("scope", "operation-id") }, contextClear, {
        description:
          "Clear text, anchors, or both with --scope text|anchors|all.",
      }),
      pause: leaf({ ...stringArgs("operation-id") }, contextPause),
      resume: leaf({ ...stringArgs("operation-id") }, contextResume),
      end: leaf({ ...stringArgs("operation-id") }, contextEnd),
      select: leaf({}, contextSelect, {
        parameters: ["[WorkContext reference]"],
      }),
      foreground: leaf(
        { ...booleanArgs("clear"), ...stringArgs("operation-id") },
        contextForeground,
        { parameters: ["[WorkContext reference]"] },
      ),
    },
  });
}
