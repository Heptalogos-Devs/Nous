// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { connectNousInstance } from "@nous-wave/client/node";
import { readFile } from "node:fs/promises";
import { extname, resolve } from "node:path";
import { toJson } from "@bufbuild/protobuf";
import { ValueSchema } from "@bufbuild/protobuf/wkt";
import { parse as parseToml } from "smol-toml";
import { CliError, uniqueReference } from "./agent.js";
import { cliState, type Selection } from "./state.js";
import { createReceiptLifecycle } from "./receipt-lifecycle.js";
import {
  consumerStatePolicySchema,
  consumerStatePaths,
} from "@nous-wave/client/consumer-policy";
export const stringFlags = [
  "run-root",
  "instance-root",
  "text",
  "file",
  "purpose",
  "source",
  "media-type",
  "strategy",
  "target",
  "supersedes",
  "representation",
  "operation-id",
  "expected-revision",
  "expected-epoch",
  "value-json",
  "view",
  "status",
  "intent",
  "from",
  "to",
  "relation",
  "aboutness-mode",
  "query-file",
  "subject",
  "session",
  "work-context",
  "kind",
  "name",
  "canonical",
  "lexical-ref",
  "association-file",
  "request-file",
  "description",
  "kind-hint",
  "event-id",
  "occurred-at",
  "query-id",
  "consumer",
  "max-operations",
  "max-model-calls",
  "max-elapsed-ms",
  "page-token",
  "page-size",
  "max-nodes",
  "max-depth",
  "max-batches",
  "max-bytes",
  "output",
  "scope",
  "mode",
  "cognition",
  "entity",
  "entity-ref",
  "binding-state",
  "host-resolution-ref",
  "reason",
  "source-region",
  "artifact",
  "limit",
] as const;
export const booleanFlags = [
  "json",
  "raw",
  "developer",
  "advanced",
  "clear",
] as const;
export const listFlags = ["tag", "aboutness", "alias"] as const;
export type CliValues = Partial<Record<(typeof stringFlags)[number], string>> &
  Partial<Record<(typeof booleanFlags)[number], boolean>> &
  Partial<Record<(typeof listFlags)[number], string[]>>;
function required(value: string | undefined, name: string): string {
  if (!value) throw new CliError("INVALID_ARGUMENT", `${name} required`);
  return value;
}
export async function createEnvironment(
  values: CliValues,
  connect = connectNousInstance,
  input?: AsyncIterable<string | Uint8Array>,
) {
  if (values.raw && !values.developer)
    throw new CliError("INVALID_ARGUMENT", "--raw requires --developer");
  const original = await connect({
    runRoot: resolve(
      required(values["run-root"], "--run-root or nous launcher"),
    ),
  });
  const policy = await original.configuration.get({
    paths: consumerStatePaths,
  });
  const policyValue = Object.fromEntries(
    consumerStatePaths.map((path) => {
      const entry = policy.entries.find((candidate) => candidate.path === path);
      if (entry?.value === undefined)
        throw new CliError(
          "CONFIGURATION_UNAVAILABLE",
          `Core did not return the active consumer state policy: ${path}`,
        );
      return [
        path.slice("consumer_state.".length),
        toJson(ValueSchema, entry.value),
      ];
    }),
  );
  const local = cliState(
    values["instance-root"] && resolve(values["instance-root"]),
    {
      instanceId: original.instanceId,
      consumer: values.consumer ?? "consumer:nous-cli:default",
    },
    consumerStatePolicySchema.parse(policyValue),
  );
  const selected = await local.selection();
  let savedBaseline = selected;
  async function addressId(kind: string, text: string, subject: string) {
    if (!/^[a-z]+:/.test(text)) return text;
    return uniqueReference(
      await original.identity.resolve({
        subjectId: kind === "subject" ? "" : subject,
        kind,
        locator: { case: "lexicalRef", value: text },
      }),
    ).value;
  }
  const selectedSubject = values.subject
    ? await addressId("subject", values.subject, "")
    : selected.subjectId;
  values = {
    ...values,
    ...(values.subject ? { subject: selectedSubject } : {}),
    ...(values.session
      ? {
          session: await addressId(
            "session",
            values.session,
            selectedSubject ?? "",
          ),
        }
      : {}),
    ...(values["work-context"]
      ? {
          "work-context": await addressId(
            "work_context",
            values["work-context"],
            selectedSubject ?? "",
          ),
        }
      : {}),
  };
  const sameSubject = !values.subject || values.subject === selected.subjectId;
  const state: Selection = {
    format: "nous.consumer.selection",
    ...(sameSubject ? selected : { lastQuery: selected.lastQuery }),
    subjectId: values.subject ?? selected.subjectId,
    sessionId: values.session ?? (sameSubject ? selected.sessionId : undefined),
    workContextId:
      values["work-context"] ??
      (sameSubject ? selected.workContextId : undefined),
  };
  const subjectId = state.subjectId ?? "";
  const notices: { code: string; message: string; receipt?: string }[] = [];
  const lifecycle = createReceiptLifecycle(original, local, subjectId, notices);
  const client = lifecycle.client;
  async function resolveReference(text: string, kind = "") {
    const resultNumber = /^result:(\d+)$/.exec(text);
    if (resultNumber) {
      if (state.lastQuery?.subjectId !== subjectId)
        throw new CliError(
          "RESULT_SUBJECT_MISMATCH",
          "Result belongs to another Subject or no query is saved",
        );
      const ref = state.lastQuery.results[Number(resultNumber[1]) - 1];
      if (!ref)
        throw new CliError(
          "NOT_FOUND",
          "Result number is outside the saved query",
        );
      if (kind && kind !== ref.kind)
        throw new CliError(
          "REFERENCE_TYPE_MISMATCH",
          "Result has another reference kind",
        );
      return ref;
    }
    if (kind && /^[0-9a-f-]{36}$/i.test(text))
      return { kind, value: text.toLowerCase() };
    const canonical =
      /^(subject|session|work_context|association|entity|resource|memory|memory_revision|cognitive_schema|cognitive_schema_revision|episode|episode_revision|journal|journal_revision|tag|occurrence|artifact|source_region|derived_representation|derived_region|external_object):(.+)$/.exec(
        text,
      );
    if (
      canonical &&
      (["entity", "resource", "external_object"].includes(canonical[1]!) ||
        /^[0-9a-f-]{36}$/i.test(canonical[2]!))
    ) {
      if (kind && kind !== canonical[1])
        throw new CliError(
          "REFERENCE_TYPE_MISMATCH",
          "Reference has another kind",
        );
      return {
        kind: canonical[1]!,
        value: ["entity", "resource"].includes(canonical[1]!)
          ? text
          : canonical[2]!,
      };
    }
    return uniqueReference(
      await client.identity.resolve({
        subjectId:
          kind === "subject" ? "" : required(subjectId, "Selected Subject"),
        kind,
        locator: {
          case: /^\w+:/.test(text) ? "lexicalRef" : "name",
          value: text,
        },
      }),
    );
  }
  async function readText(path: string, maximum = 65536) {
    let text = "";
    if (path === "-") {
      if (!input)
        throw new CliError(
          "INVALID_ARGUMENT",
          "This host reserves stdin for its protocol; provide a file or --text",
        );
      for await (const chunk of input) {
        text += String(chunk);
        if (Buffer.byteLength(text) > maximum)
          throw new CliError("INVALID_ARGUMENT", "Input exceeds bound");
      }
    } else text = await readFile(path, "utf8");
    if (Buffer.byteLength(text) > maximum)
      throw new CliError("INVALID_ARGUMENT", "Input exceeds bound");
    return text;
  }
  async function requestPayload(path?: string): Promise<unknown> {
    const text = await readText(required(path, "Semantic TOML file"));
    if (Buffer.byteLength(text) > 65536)
      throw new CliError("INVALID_ARGUMENT", "Semantic TOML exceeds 64 KiB");
    return parseToml(text);
  }
  return {
    client,
    values,
    state,
    subjectId,
    notices,
    required,
    resolveReference,
    requestPayload,
    readText,
    async save(this: void, value: Selection) {
      try {
        await local.save(value, savedBaseline);
        savedBaseline = value;
      } catch (error) {
        notices.push({
          code: "STATE_UNSAVED",
          message:
            error instanceof CliError && error.code === "STATE_CONFLICT"
              ? error.message
              : "Operation result is preserved; local consumer state could not be saved",
        });
      }
    },
    retry: lifecycle.retry,
    inspectOperation: lifecycle.inspectOperation,
    inspectOperationIdentity: lifecycle.inspectOperationIdentity,
    mediaType(this: void, path: string) {
      const types: Record<string, string> = {
        ".txt": "text/plain",
        ".md": "text/markdown",
        ".json": "application/json",
        ".png": "image/png",
        ".jpg": "image/jpeg",
        ".jpeg": "image/jpeg",
        ".webp": "image/webp",
        ".wav": "audio/wav",
        ".mp3": "audio/mpeg",
        ".ogg": "audio/ogg",
        ".mp4": "video/mp4",
        ".webm": "video/webm",
      };
      return (
        values["media-type"] ??
        types[extname(path).toLowerCase()] ??
        "application/octet-stream"
      );
    },
  };
}
export type CliEnvironment = Awaited<ReturnType<typeof createEnvironment>>;
