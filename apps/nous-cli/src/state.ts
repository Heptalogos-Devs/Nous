// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { mkdir, readFile, readdir, unlink } from "node:fs/promises";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { isDeepStrictEqual } from "node:util";
import { parse, stringify } from "devalue";
import { lock } from "proper-lockfile";
import writeAtomic from "write-file-atomic";
import { z } from "zod";
import type { ConsumerStatePolicy } from "@nous-wave/client/consumer-policy";
import { CliError } from "./agent.js";
import {
  protocolSchema,
  protocolSchemaByName,
  restoreProtocolData,
} from "@nous-wave/client/data";

const reference = z.strictObject({
  kind: z.string().max(64),
  value: z.string().max(2048),
});
const selectionSchema = z.strictObject({
  format: z.literal("nous.consumer.selection"),
  subjectId: z.string().optional(),
  sessionId: z.string().optional(),
  workContextId: z.string().optional(),
  lastQuery: z
    .strictObject({
      subjectId: z.string(),
      queryId: z.string(),
      results: z.array(reference).max(2048),
    })
    .optional(),
});
export type Selection = z.infer<typeof selectionSchema>;
const receiptHeader = {
  format: z.literal("nous.consumer.operation"),
  name: z.string().max(128),
  subjectId: z.string().optional(),
  request: z.unknown(),
};
const receiptSchema = z.discriminatedUnion("status", [
  z.strictObject({ ...receiptHeader, status: z.literal("pending") }),
  z.strictObject({
    ...receiptHeader,
    status: z.literal("complete"),
    result: z.unknown(),
  }),
  z.strictObject({
    ...receiptHeader,
    status: z.literal("rejected"),
    rejection: z.strictObject({
      code: z.union([z.string(), z.number()]),
      message: z.string(),
      domainCode: z.string().optional(),
      recovery: z.string().optional(),
      context: z.record(z.string(), z.string()).optional(),
      details: z.array(z.unknown()).optional(),
      candidates: z.array(z.unknown()).optional(),
    }),
  }),
]);
const emptySelection = (): Selection => ({ format: "nous.consumer.selection" });
const selectionKeys = [
  "subjectId",
  "sessionId",
  "workContextId",
  "lastQuery",
] as const;

export function cliState(
  root: string | undefined,
  identity: { instanceId: string; consumer: string },
  policy: ConsumerStatePolicy,
) {
  if (!identity.instanceId || !identity.consumer)
    throw new CliError(
      "INVALID_ARGUMENT",
      "Instance and consumer identity are required",
    );
  const namespace = createHash("sha256")
    .update(JSON.stringify([identity.instanceId, identity.consumer]))
    .digest("hex");
  const directory = () => {
    if (!root)
      throw new CliError(
        "INVALID_ARGUMENT",
        "Provide a consumer state root for durable selection and receipts",
      );
    return join(root, "consumers", namespace);
  };
  const selectionPath = () => join(directory(), "selection.json");
  const receiptPath = (id: string) => {
    if (!/^[a-zA-Z0-9_-]{1,128}$/.test(id))
      throw new CliError("INVALID_ARGUMENT", "Invalid receipt ID");
    return join(directory(), "operations", `${id}.json`);
  };
  const read = async (path: string): Promise<unknown> => {
    const bytes = await readFile(path);
    if (bytes.byteLength > policy.file_max_bytes)
      throw new CliError(
        "INVALID_ARGUMENT",
        "Consumer state exceeds its configured byte budget",
      );
    return parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes), {
      ProtocolData: ([name, value]: [string, unknown]) => {
        const schema = protocolSchemaByName(name);
        if (!schema)
          throw new CliError(
            "INVALID_ARGUMENT",
            "Stored protocol data type is unavailable",
          );
        return restoreProtocolData(value, schema);
      },
    });
  };
  const write = async (path: string, value: unknown) => {
    const text = stringify(value, {
      ProtocolData: (member) => {
        const schema = protocolSchema(member);
        return schema
          ? [schema.typeName, { ...(member as object) }]
          : undefined;
      },
    });
    if (Buffer.byteLength(text) > policy.file_max_bytes)
      throw new CliError(
        "RESOURCE_EXHAUSTED",
        "Consumer state exceeds its configured byte budget",
      );
    await writeAtomic(path, text, { mode: 0o600 });
  };
  const transaction = async <T>(work: () => Promise<T>) => {
    const path = directory();
    await mkdir(path, { recursive: true, mode: 0o700 });
    const release = await lock(path, {
      stale: policy.lock_stale_ms,
      retries: {
        retries: Math.ceil(policy.lock_wait_ms / 100),
        factor: 1,
        minTimeout: 100,
        maxTimeout: 100,
      },
    });
    try {
      return await work();
    } finally {
      await release();
    }
  };
  const selection = async (): Promise<Selection> => {
    if (!root) return emptySelection();
    try {
      return selectionSchema.parse(await read(selectionPath()));
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT")
        return emptySelection();
      throw error;
    }
  };
  return {
    selection,
    async save(value: Selection, baseline: Selection) {
      const desired = selectionSchema.parse(value);
      await transaction(async () => {
        const current = await selection();
        const changed = selectionKeys.filter(
          (name) => !isDeepStrictEqual(desired[name], baseline[name]),
        );
        if (changed.includes("subjectId")) {
          for (const name of [
            "sessionId",
            "workContextId",
            "lastQuery",
          ] as const)
            if (!changed.includes(name)) changed.push(name);
        }
        const scoped = changed.some((name) => name !== "subjectId");
        const queryChanged = changed.includes("lastQuery");
        const scopeConflict =
          (scoped &&
            current.subjectId !== baseline.subjectId &&
            current.subjectId !== desired.subjectId) ||
          (queryChanged &&
            ["sessionId", "workContextId"].some((name) => {
              const key = name as "sessionId" | "workContextId";
              return (
                current[key] !== baseline[key] && current[key] !== desired[key]
              );
            }));
        if (
          scopeConflict ||
          changed.some(
            (name) =>
              !isDeepStrictEqual(current[name], baseline[name]) &&
              !isDeepStrictEqual(current[name], desired[name]),
          )
        )
          throw new CliError(
            "STATE_CONFLICT",
            "Consumer selection changed concurrently; continue using the returned exact references",
          );
        const merged = { ...current };
        for (const name of changed)
          Object.assign(merged, { [name]: desired[name] });
        await write(selectionPath(), merged);
      });
    },
    async receipt(id: string) {
      return receiptSchema.parse(await read(receiptPath(id)));
    },
    async writeReceipt(id: string, value: z.infer<typeof receiptSchema>) {
      const receipt = receiptSchema.parse(value);
      await transaction(async () => {
        const path = receiptPath(id);
        const operations = join(directory(), "operations");
        await mkdir(operations, { recursive: true, mode: 0o700 });
        const files = (await readdir(operations)).filter((name) =>
          /^[a-zA-Z0-9_-]+\.json$/.test(name),
        );
        if (files.includes(`${id}.json`)) {
          const previous = receiptSchema.parse(await read(path));
          if (
            previous.name !== receipt.name ||
            previous.subjectId !== receipt.subjectId ||
            !isDeepStrictEqual(previous.request, receipt.request)
          )
            throw new CliError(
              "STATE_CONFLICT",
              "Receipt identity already belongs to another frozen request",
            );
          if (previous.status !== "pending") {
            if (
              receipt.status === "pending" ||
              isDeepStrictEqual(previous, receipt)
            )
              return;
            throw new CliError(
              "STATE_CONFLICT",
              "Receipt already has a different terminal outcome",
            );
          }
        }
        if (
          !files.includes(`${id}.json`) &&
          files.length >= policy.receipt_limit
        ) {
          let remaining = files.length;
          for (const file of files) {
            const old = receiptSchema.parse(await read(join(operations, file)));
            if (old.status !== "pending") {
              await unlink(join(operations, file));
              if (--remaining < policy.receipt_limit) break;
            }
          }
          if (remaining >= policy.receipt_limit)
            throw new CliError(
              "RESOURCE_EXHAUSTED",
              "Unresolved operation receipts reached the configured limit; recover them before another mutation",
            );
        }
        await write(path, receipt);
      });
    },
  };
}
