// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { EmptySchema } from "@bufbuild/protobuf/wkt";
import { z } from "zod";
import { boundedInteger, CliError, nonnegativeInt64 } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
import {
  SetCognitionAccessibilitySchema,
  type CognitionMutationRequest,
  type CognitionResult,
} from "@nous-wave/protocol/nous/wave/v1alpha1/cognition_pb.js";
import {
  PageSchema,
  type Page,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { stringArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

const revisionKindByObject = {
  memory: "memory_revision",
  cognitive_schema: "cognitive_schema_revision",
  episode: "episode_revision",
  journal: "journal_revision",
} as const;
type CognitionKind = keyof typeof revisionKindByObject;
type CognitionRevisionKind = (typeof revisionKindByObject)[CognitionKind];
type CognitionReference = {
  [K in CognitionKind]:
    | { kind: K; value: string }
    | { kind: (typeof revisionKindByObject)[K]; value: string };
}[CognitionKind];
export type CognitionRevisionReference = Extract<
  CognitionReference,
  { kind: CognitionRevisionKind }
>;
type CognitionFamilyResult = Exclude<
  CognitionResult["result"],
  { case: undefined }
>;
type CognitionFamilyCase = CognitionFamilyResult["case"];
type CognitionObject = CognitionFamilyResult["value"];
type CognitionMutation =
  "suppress" | "restore" | "withdraw" | "reaccept" | "purge" | "accessibility";
type CognitionLifecycleMutation = Exclude<CognitionMutation, "accessibility">;

type CognitionResultCaseForKind<K extends CognitionKind> = K extends
  "memory" | "episode" | "journal"
  ? K
  : "cognitiveSchema";
type CognitionObjectForKind<K extends CognitionKind> = Extract<
  CognitionFamilyResult,
  { case: CognitionResultCaseForKind<K> }
>["value"];
type CognitionObjectForReference<K extends CognitionReference["kind"]> = {
  [Family in CognitionKind]: K extends
    Family | (typeof revisionKindByObject)[Family]
    ? CognitionObjectForKind<Family>
    : never;
}[CognitionKind];

function isCognitionReference(reference: {
  kind: string;
  value: string;
}): reference is CognitionReference {
  return (
    Object.keys(revisionKindByObject).some((kind) => kind === reference.kind) ||
    Object.values(revisionKindByObject).some((kind) => kind === reference.kind)
  );
}

export function isCognitionRevisionReference(reference: {
  kind: string;
  value: string;
}): reference is CognitionRevisionReference {
  return (
    isCognitionReference(reference) && reference.kind.endsWith("_revision")
  );
}

function familyKind(referenceKind: CognitionReference["kind"]): CognitionKind {
  switch (referenceKind) {
    case "memory":
    case "memory_revision":
      return "memory";
    case "cognitive_schema":
    case "cognitive_schema_revision":
      return "cognitive_schema";
    case "episode":
    case "episode_revision":
      return "episode";
    case "journal":
    case "journal_revision":
      return "journal";
  }
}

function resultCase(kind: CognitionKind): CognitionFamilyCase {
  return kind === "cognitive_schema" ? "cognitiveSchema" : kind;
}

function familyValue(
  result: CognitionResult,
  expected: CognitionFamilyCase,
): CognitionObject {
  if (result.result.case === undefined || result.result.case !== expected) {
    throw new CliError(
      "INTERNAL_FAILURE",
      "Cognition service returned a result with an unexpected type",
    );
  }
  return result.result.value;
}

async function read<Reference extends CognitionReference>(
  env: CliEnvironment,
  reference: Reference,
): Promise<CognitionObjectForReference<Reference["kind"]>> {
  const result = await env.client.cognition.read({
    subjectId: env.subjectId,
    reference,
  });
  return familyValue(
    result,
    resultCase(familyKind(reference.kind)),
  ) as CognitionObjectForReference<Reference["kind"]>; // Runtime case validation preserves the reference-family correlation.
}

async function list(
  env: CliEnvironment,
  kind: CognitionKind,
  page: Page,
  status: string,
) {
  const response = await env.client.cognition.list({
    subjectId: env.subjectId,
    kind,
    page,
    status,
  });
  return {
    items: response.items.map((item) => familyValue(item, resultCase(kind))),
    nextPageToken: response.nextPageToken,
  };
}

async function history(
  env: CliEnvironment,
  reference: CognitionReference & { kind: CognitionKind },
  page: Page,
) {
  const response = await env.client.cognition.history({
    subjectId: env.subjectId,
    objectRef: reference,
    page,
  });
  return {
    items: response.items.map((item) =>
      familyValue(item, resultCase(reference.kind)),
    ),
    nextPageToken: response.nextPageToken,
  };
}

function actionValue(
  action: CognitionMutation,
  mode?: string,
): CognitionMutationRequest["action"] {
  switch (action) {
    case "accessibility":
      return {
        case: "setAccessibility",
        value: create(SetCognitionAccessibilitySchema, { mode: mode ?? "" }),
      };
    case "suppress":
      return { case: "suppress", value: create(EmptySchema) };
    case "restore":
      return { case: "restore", value: create(EmptySchema) };
    case "withdraw":
      return { case: "withdraw", value: create(EmptySchema) };
    case "reaccept":
      return { case: "reaccept", value: create(EmptySchema) };
    case "purge":
      return { case: "purge", value: create(EmptySchema) };
  }
}

async function mutate(
  env: CliEnvironment,
  reference: CognitionReference & { kind: CognitionKind },
  action: CognitionMutation,
  expectedObjectEpoch: bigint,
  operationId: string,
  mode?: string,
) {
  const response = await env.client.cognition.mutate({
    operationId,
    subjectId: env.subjectId,
    objectRef: reference,
    expectedObjectEpoch,
    action: actionValue(action, mode),
  });
  switch (response.outcome.case) {
    case "cognition":
      return familyValue(response.outcome.value, resultCase(reference.kind));
    case "purged":
      if (action === "purge") return response.outcome.value;
      break;
    case undefined:
      break;
  }
  throw new CliError(
    "INTERNAL_FAILURE",
    "Cognition service returned an unexpected mutation outcome",
  );
}

type CognitionCommandOptions = {
  kind: CognitionKind;
  label: string;
  list?: boolean;
  history?: boolean;
  accessibility?: boolean;
};

/** Build the real shared Citty leaves for Cognition families. */
export function cognitionSubcommands(
  leaf: CliLeaf,
  {
    kind,
    label,
    list: hasList,
    history: hasHistory,
    accessibility,
  }: CognitionCommandOptions,
) {
  const page = (env: CliEnvironment) =>
    create(PageSchema, {
      pageSize: boundedInteger(env.values["page-size"], 1, 200, "--page-size"),
      pageToken: env.values["page-token"],
    });
  const resolve = async (env: CliEnvironment, argument: string | undefined) =>
    env.resolveReference(env.required(argument, `${label} reference`), kind);
  const lifecycle = (action: CognitionLifecycleMutation) =>
    leaf(
      stringArgs("expected-epoch", "operation-id"),
      async (env, argument) => {
        const reference = await resolve(env, argument);
        return mutate(
          env,
          { kind, value: reference.value },
          action,
          nonnegativeInt64(env.values["expected-epoch"], "--expected-epoch"),
          env.values["operation-id"] ?? crypto.randomUUID(),
        );
      },
      { parameters: [`<${label} reference>`] },
    );

  const reads = {
    ...(hasList
      ? {
          list: leaf(
            stringArgs("page-size", "page-token", "status"),
            (env) => list(env, kind, page(env), env.values.status ?? ""),
            kind === "memory"
              ? { description: "List paged Memories for the selected Subject." }
              : undefined,
          ),
        }
      : {}),
    get: leaf(
      {},
      async (env, argument) => {
        const reference = await resolve(env, argument);
        return read(env, { kind, value: reference.value });
      },
      { parameters: [`<${label} reference>`] },
    ),
    revision: leaf(
      {},
      async (env, argument) => {
        const reference = await env.resolveReference(
          env.required(argument, `${label} revision reference`),
          revisionKindByObject[kind],
        );
        return read(env, {
          kind: revisionKindByObject[kind],
          value: reference.value,
        });
      },
      { parameters: [`<exact ${label} revision reference>`] },
    ),
    ...(hasHistory
      ? {
          history: leaf(
            stringArgs("page-size", "page-token"),
            async (env, argument) => {
              const reference = await resolve(env, argument);
              return history(env, { kind, value: reference.value }, page(env));
            },
            { parameters: [`<${label} reference>`] },
          ),
        }
      : {}),
  };
  const lifecycleCommands = {
    suppress: lifecycle("suppress"),
    restore: lifecycle("restore"),
    withdraw: lifecycle("withdraw"),
    reaccept: lifecycle("reaccept"),
    purge: lifecycle("purge"),
    ...(accessibility
      ? {
          accessibility: leaf(
            stringArgs("mode", "expected-epoch", "operation-id"),
            async (env, argument) => {
              const reference = await resolve(env, argument);
              const mode = z
                .enum(["auto", "normal", "deep", "explicit"])
                .parse(
                  env.required(
                    env.values.mode,
                    "--mode auto|normal|deep|explicit",
                  ),
                );
              return mutate(
                env,
                { kind, value: reference.value },
                "accessibility",
                nonnegativeInt64(
                  env.values["expected-epoch"],
                  "--expected-epoch",
                ),
                env.values["operation-id"] ?? crypto.randomUUID(),
                mode,
              );
            },
            { parameters: [`<${label} reference>`] },
          ),
        }
      : {}),
  };
  return { reads, lifecycle: lifecycleCommands };
}

/** Shared typed adapters for CognitionService's common object operations. */
export const typedCognition = {
  read,
  list,
  history,
  mutate,
};
