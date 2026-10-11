// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { EmptySchema } from "@bufbuild/protobuf/wkt";
import { z } from "zod";
import { boundedInteger, CliError, nonnegativeInt64 } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
import {
  CognitionHistoryRequestSchema,
  CognitionListRequestSchema,
  CognitionMutationRequestSchema,
  ReadCognitionRequestSchema,
  SetCognitionAccessibilitySchema,
  type CognitionMutationRequest,
  type CognitionResult,
} from "@nous-wave/protocol/nous/wave/v1alpha1/cognition_pb.js";
import {
  CognitiveRefSchema,
  type Episode,
  type Memory,
  type Page,
  PageSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { CognitiveSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import type { Journal } from "@nous-wave/protocol/nous/wave/v1alpha1/journal_pb.js";

type CognitionKind = "memory" | "cognitive_schema" | "episode" | "journal";
type CognitionReference = {
  kind:
    | CognitionKind
    | "memory_revision"
    | "cognitive_schema_revision"
    | "episode_revision"
    | "journal_revision";
  value: string;
};
export type CognitionRevisionReference = {
  kind:
    | "memory_revision"
    | "cognitive_schema_revision"
    | "episode_revision"
    | "journal_revision";
  value: string;
};
type CognitionObject = Memory | CognitiveSchema | Episode | Journal;
type CognitionMutation =
  "suppress" | "restore" | "withdraw" | "reaccept" | "purge" | "accessibility";
type CognitionLifecycleMutation = Exclude<CognitionMutation, "accessibility">;
type CognitionFamilyCase = Exclude<
  CognitionResult["result"]["case"],
  undefined
>;
const revisionKindByObject: Record<CognitionKind, CognitionReference["kind"]> =
  {
    memory: "memory_revision",
    cognitive_schema: "cognitive_schema_revision",
    episode: "episode_revision",
    journal: "journal_revision",
  };

function isCognitionReference(reference: {
  kind: string;
  value: string;
}): reference is CognitionReference {
  return [
    "memory",
    "memory_revision",
    "cognitive_schema",
    "cognitive_schema_revision",
    "episode",
    "episode_revision",
    "journal",
    "journal_revision",
  ].includes(reference.kind);
}

export function isCognitionRevisionReference(reference: {
  kind: string;
  value: string;
}): reference is CognitionRevisionReference {
  return (
    isCognitionReference(reference) && reference.kind.endsWith("_revision")
  );
}

function isCognitionLifecycleMutation(
  action: string | undefined,
): action is CognitionLifecycleMutation {
  return ["suppress", "restore", "withdraw", "reaccept", "purge"].includes(
    action ?? "",
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
  switch (kind) {
    case "memory":
      return "memory";
    case "cognitive_schema":
      return "cognitiveSchema";
    case "episode":
      return "episode";
    case "journal":
      return "journal";
  }
}

function familyValue(result: CognitionResult, expected: "memory"): Memory;
function familyValue(
  result: CognitionResult,
  expected: "cognitiveSchema",
): CognitiveSchema;
function familyValue(result: CognitionResult, expected: "episode"): Episode;
function familyValue(result: CognitionResult, expected: "journal"): Journal;
function familyValue(
  result: CognitionResult,
  expected: CognitionFamilyCase,
): CognitionObject;
function familyValue(
  result: CognitionResult,
  expected: CognitionFamilyCase,
): CognitionObject {
  switch (result.result.case) {
    case "memory":
      if (expected === "memory") return result.result.value;
      break;
    case "cognitiveSchema":
      if (expected === "cognitiveSchema") return result.result.value;
      break;
    case "episode":
      if (expected === "episode") return result.result.value;
      break;
    case "journal":
      if (expected === "journal") return result.result.value;
      break;
    case undefined:
      break;
  }
  throw new CliError(
    "INTERNAL_FAILURE",
    "Cognition service returned a result with an unexpected type",
  );
}

function cognitionRef(reference: CognitionReference) {
  return create(CognitiveRefSchema, {
    kind: reference.kind,
    value: reference.value,
  });
}

function read(
  env: CliEnvironment,
  reference: { kind: "memory" | "memory_revision"; value: string },
): Promise<Memory>;
function read(
  env: CliEnvironment,
  reference: {
    kind: "cognitive_schema" | "cognitive_schema_revision";
    value: string;
  },
): Promise<CognitiveSchema>;
function read(
  env: CliEnvironment,
  reference: { kind: "episode" | "episode_revision"; value: string },
): Promise<Episode>;
function read(
  env: CliEnvironment,
  reference: { kind: "journal" | "journal_revision"; value: string },
): Promise<Journal>;
function read(
  env: CliEnvironment,
  reference: CognitionReference,
): Promise<CognitionObject>;
async function read(
  env: CliEnvironment,
  reference: CognitionReference,
): Promise<CognitionObject> {
  const result = await env.client.cognition.read(
    create(ReadCognitionRequestSchema, {
      subjectId: env.subjectId,
      reference: cognitionRef(reference),
    }),
  );
  return familyValue(result, resultCase(familyKind(reference.kind)));
}

async function list(
  env: CliEnvironment,
  kind: CognitionKind,
  page: Page,
  status: string,
) {
  const response = await env.client.cognition.list(
    create(CognitionListRequestSchema, {
      subjectId: env.subjectId,
      kind,
      page,
      status,
    }),
  );
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
  const response = await env.client.cognition.history(
    create(CognitionHistoryRequestSchema, {
      subjectId: env.subjectId,
      objectRef: cognitionRef(reference),
      page,
    }),
  );
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
  const response = await env.client.cognition.mutate(
    create(CognitionMutationRequestSchema, {
      operationId,
      subjectId: env.subjectId,
      objectRef: cognitionRef(reference),
      expectedObjectEpoch,
      action: actionValue(action, mode),
    }),
  );
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

async function commonCommand(
  env: CliEnvironment,
  kind: CognitionKind,
  action: string | undefined,
  argument: string | undefined,
  features: { list?: boolean; history?: boolean; accessibility?: boolean } = {},
): Promise<{ handled: false } | { handled: true; value: unknown }> {
  const name =
    kind === "cognitive_schema"
      ? "CognitiveSchema"
      : kind[0]!.toUpperCase() + kind.slice(1);
  const page = () =>
    create(PageSchema, {
      pageSize: boundedInteger(env.values["page-size"], 1, 200, "--page-size"),
      pageToken: env.values["page-token"],
    });

  if (action === "list" && features.list) {
    return {
      handled: true,
      value: await list(env, kind, page(), env.values.status ?? ""),
    };
  }
  if (action === "get" || action === "revision") {
    const referenceKind = action === "get" ? kind : revisionKindByObject[kind];
    const ref = await env.resolveReference(
      env.required(
        argument,
        action === "get" ? `${name} reference` : `${name} revision reference`,
      ),
      referenceKind,
    );
    return {
      handled: true,
      value: await read(env, { kind: referenceKind, value: ref.value }),
    };
  }
  if (action === "history" && features.history) {
    const ref = await env.resolveReference(
      env.required(argument, `${name} reference`),
      kind,
    );
    return {
      handled: true,
      value: await history(env, { kind, value: ref.value }, page()),
    };
  }
  if (
    isCognitionLifecycleMutation(action) ||
    (action === "accessibility" && features.accessibility)
  ) {
    const ref = await env.resolveReference(
      env.required(argument, `${name} reference`),
      kind,
    );
    const expectedEpoch = nonnegativeInt64(
      env.values["expected-epoch"],
      "--expected-epoch",
    );
    const mode =
      action === "accessibility"
        ? z
            .enum(["auto", "normal", "deep", "explicit"])
            .parse(
              env.required(env.values.mode, "--mode auto|normal|deep|explicit"),
            )
        : undefined;
    return {
      handled: true,
      value: await mutate(
        env,
        { kind, value: ref.value },
        action,
        expectedEpoch,
        env.values["operation-id"] ?? crypto.randomUUID(),
        mode,
      ),
    };
  }
  return { handled: false };
}

/** Shared, typed adapters for CognitionService's common object operations. */
export const typedCognition = {
  read,
  list,
  history,
  mutate,
  commonCommand,
};
