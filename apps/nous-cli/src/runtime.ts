// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { toJson } from "@bufbuild/protobuf";
import {
  ValueSchema,
  MethodOptions_IdempotencyLevel,
} from "@bufbuild/protobuf/wkt";
import { connectNousInstance } from "@nous-wave/client/node";
import { NousError, serviceDescriptors } from "@nous-wave/client";
import { readFile } from "node:fs/promises";
import { extname, resolve } from "node:path";
import { parse as parseToml } from "smol-toml";
import { CliError, uniqueReference } from "./agent.js";
import { cliState, type Selection } from "./state.js";
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
  "supersedes",
  "representation",
  "operation-id",
  "expected-revision",
  "expected-object-epoch",
  "aboutness-mode",
  "query-file",
  "subject",
  "session",
  "work-context",
  "kind",
  "name",
  "canonical",
  "lexical-ref",
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
  "status",
  "page-token",
  "page-size",
  "max-nodes",
  "max-depth",
  "max-batches",
  "max-bytes",
  "max-items",
  "output",
  "scope",
  "cognition",
  "entity",
] as const;
export const booleanFlags = [
  "json",
  "raw",
  "developer",
  "desired",
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
  let localState: Promise<ReturnType<typeof cliState>> | undefined;
  async function local() {
    return (localState ??= (async () => {
      const policy = await original.configuration.getConfiguration({
        paths: consumerStatePaths,
      });
      const policyValue = Object.fromEntries(
        consumerStatePaths.map((path) => {
          const entry = policy.entries.find(
            (candidate) => candidate.path === path,
          );
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
      return cliState(
        values["instance-root"] && resolve(values["instance-root"]),
        {
          instanceId: original.instanceId,
          consumer: values.consumer ?? "default",
        },
        consumerStatePolicySchema.parse(policyValue),
      );
    })());
  }
  let baseline: Promise<Selection> | undefined;
  const selection = () =>
    (baseline ??= values["instance-root"]
      ? local().then((state) => state.selection())
      : Promise.resolve({ format: "nous.consumer.selection" } as Selection));
  let savedBaseline: Selection | undefined;
  async function bindReference(text: string, kind: string, subject: string) {
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
      await original.identity.resolveIdentity({
        subjectId:
          kind === "subject" ? "" : required(subject, "Selected Subject"),
        kind,
        locator: {
          case: /^\w+:/.test(text) ? "lexicalRef" : "name",
          value: text,
        },
      }),
    );
  }
  async function addressId(kind: string, text: string, subject: string) {
    return (await bindReference(text, kind, subject)).value;
  }
  let subject: Promise<string> | undefined;
  const selectedSubject = () =>
    (subject ??= values.subject
      ? addressId("subject", values.subject, "")
      : selection().then((state) => state.subjectId ?? ""));
  let stateView: Promise<Selection> | undefined;
  async function selectedState(): Promise<Selection> {
    return (stateView ??= (async () => {
      const selected = await selection();
      const subjectId = await selectedSubject();
      return {
        format: "nous.consumer.selection" as const,
        ...(subjectId === selected.subjectId
          ? selected
          : { lastQuery: selected.lastQuery }),
        subjectId,
      };
    })());
  }
  async function contextId(
    kind: "session" | "work_context",
    flag: "session" | "work-context",
    field: "sessionId" | "workContextId",
  ) {
    return values[flag]
      ? addressId(kind, values[flag], await selectedSubject())
      : (await selectedState())[field];
  }
  const notices: { code: string; message: string; receipt?: string }[] = [];
  async function executeReceipt(
    id: string,
    receipt: Awaited<ReturnType<ReturnType<typeof cliState>["receipt"]>>,
  ) {
    const [domain, name] = receipt.name.split(".");
    const methods = original[
      domain as keyof typeof original
    ] as unknown as Record<string, (request: object) => Promise<unknown>>;
    if (!methods?.[name!])
      throw new CliError("INVALID_ARGUMENT", "Saved operation is unavailable");
    try {
      const result = await methods[name!]!(receipt.request as object);
      await (await local()).removeReceipt(id).catch(() =>
        notices.push({
          code: "REQUEST_UNSAVED",
          message:
            "Operation completed; saved retry request could not be removed",
          receipt: id,
        }),
      );
      return result;
    } catch (error) {
      const rejected =
        error instanceof NousError &&
        error.context.commit_status === "rejected";
      if (rejected) {
        await (await local()).removeReceipt(id).catch(() =>
          notices.push({
            code: "REQUEST_UNSAVED",
            message:
              "Rejected request could not be removed; forget this saved request",
            receipt: id,
          }),
        );
        throw error;
      }
      const failure =
        error instanceof NousError || error instanceof CliError
          ? error
          : new CliError(
              "OUTCOME_UNKNOWN",
              `Response unavailable; retry ${id} with the saved request`,
            );
      Object.assign(failure, { receipt: id });
      throw failure;
    }
  }
  const client = { ...original };
  {
    for (const [domain, service] of Object.entries(serviceDescriptors)) {
      const source = original[
        domain as keyof typeof original
      ] as unknown as Record<string, (request: object) => Promise<unknown>>;
      const wrapped = { ...source };
      for (const method of service.methods) {
        if (method.idempotency !== MethodOptions_IdempotencyLevel.IDEMPOTENT)
          continue;
        const name = method.localName;
        wrapped[name] = async (request) => {
          const id = crypto.randomUUID();
          const receipt = {
            format: "nous.consumer.operation" as const,
            name: `${domain}.${name}`,
            subjectId:
              "subjectId" in request ? String(request.subjectId) : undefined,
            request,
          };
          await (await local()).writeReceipt(id, receipt);
          return executeReceipt(id, receipt);
        };
      }
      Object.assign(client, { [domain]: wrapped });
    }
  }
  async function resolveReference(text: string, kind = "") {
    const resultNumber = /^result:(\d+)$/.exec(text);
    if (resultNumber) {
      const state = await selectedState();
      if (state.lastQuery?.subjectId !== (await selectedSubject()))
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
    return bindReference(
      text,
      kind,
      kind === "subject" ? "" : await selectedSubject(),
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
    get state() {
      return selectedState();
    },
    get subjectId() {
      return selectedSubject();
    },
    get sessionId() {
      return contextId("session", "session", "sessionId");
    },
    get workContextId() {
      return contextId("work_context", "work-context", "workContextId");
    },
    notices,
    required,
    resolveReference,
    requestPayload,
    readText,
    async save(this: void, value: Selection) {
      try {
        await (await local()).save(value, savedBaseline ?? (await selection()));
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
    async forget(id: string) {
      await (await local()).removeReceipt(id);
      return { forgottenRequest: id };
    },
    async retry(id: string) {
      const receipt = await (await local()).receipt(id);
      if (
        receipt.name !== "subjects.createSubject" &&
        receipt.subjectId &&
        receipt.subjectId !== (await selectedSubject())
      )
        throw new CliError(
          "RESULT_SUBJECT_MISMATCH",
          "Receipt belongs to another Subject",
        );
      return executeReceipt(id, receipt);
    },
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
