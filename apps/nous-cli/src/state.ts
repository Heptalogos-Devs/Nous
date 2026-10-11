// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { mkdir, readFile, readdir, unlink } from "node:fs/promises";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { isDeepStrictEqual } from "node:util";
import { parse as parseDevalue, stringify as stringifyDevalue } from "devalue";
import { lock } from "proper-lockfile";
import writeAtomic from "write-file-atomic";
import { z } from "zod";
import type { ConsumerStatePolicy } from "@nous-wave/client/consumer-policy";
import { CliError } from "./agent.js";

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

const requestSchema = z.strictObject({
  typeName: z.string(),
  protoJson: z.unknown(),
});
const resultSchema = z.strictObject({
  typeName: z.string(),
  protoJson: z.unknown(),
});
const safeErrorSchema = z.strictObject({
  transportCode: z.number().int().optional(),
  code: z.string().optional(),
  owner: z.string().optional(),
  message: z.string().max(2048),
  recovery: z.string().optional(),
  context: z.record(z.string(), z.string()).default({}),
  traceId: z.string().optional(),
});
const receiptHeader = {
  format: z.literal("nous.consumer.operation.v2"),
  receiptId: z.string().min(1).max(128),
  command: z.string().min(1).max(128),
  owner: z.string().min(1).max(32),
  operationKind: z.string().min(1).max(64),
  subjectId: z.string().optional(),
  operationId: z.string().min(1).max(128),
  requestDigest: z.string().regex(/^[0-9a-f]{64}$/),
  request: requestSchema,
  createdAt: z.string().min(1).max(40),
  updatedAt: z.string().min(1).max(40),
};
export const receiptSchema = z.discriminatedUnion("status", [
  z.strictObject({ ...receiptHeader, status: z.literal("pending") }),
  z.strictObject({ ...receiptHeader, status: z.literal("outcome_unknown") }),
  z.strictObject({
    ...receiptHeader,
    status: z.literal("complete"),
    result: resultSchema,
  }),
  z.strictObject({
    ...receiptHeader,
    status: z.literal("rejected"),
    error: safeErrorSchema,
  }),
]);
export type OperationReceipt = z.infer<typeof receiptSchema>;

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
  const readBytes = async (path: string) => {
    const bytes = await readFile(path);
    if (bytes.byteLength > policy.file_max_bytes)
      throw new CliError(
        "INVALID_ARGUMENT",
        "Consumer state exceeds its configured byte budget",
      );
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  };
  const writeText = async (path: string, value: string) => {
    if (Buffer.byteLength(value) > policy.file_max_bytes)
      throw new CliError(
        "RESOURCE_EXHAUSTED",
        "Consumer state exceeds its configured byte budget",
      );
    await writeAtomic(path, value, { mode: 0o600 });
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
      return selectionSchema.parse(
        parseDevalue(await readBytes(selectionPath())),
      );
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT")
        return emptySelection();
      throw error;
    }
  };
  const receipt = async (id: string): Promise<OperationReceipt> => {
    try {
      return receiptSchema.parse(JSON.parse(await readBytes(receiptPath(id))));
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") throw error;
      if (error instanceof SyntaxError || error instanceof z.ZodError)
        throw new CliError(
          "INVALID_ARGUMENT",
          "Receipt is not a valid v2 operation receipt and cannot be replayed",
        );
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
        await writeText(selectionPath(), stringifyDevalue(merged));
      });
    },
    receipt,
    async writeReceipt(id: string, value: OperationReceipt) {
      const next = receiptSchema.parse(value);
      if (next.receiptId !== id)
        throw new CliError(
          "INVALID_ARGUMENT",
          "Receipt file identity mismatch",
        );
      await transaction(async () => {
        const path = receiptPath(id);
        const operations = join(directory(), "operations");
        await mkdir(operations, { recursive: true, mode: 0o700 });
        const files = (await readdir(operations)).filter((name) =>
          /^[a-zA-Z0-9_-]+\.json$/.test(name),
        );
        if (files.includes(`${id}.json`)) {
          const previous = await receipt(id);
          const immutable = (item: OperationReceipt) => ({
            receiptId: item.receiptId,
            command: item.command,
            owner: item.owner,
            operationKind: item.operationKind,
            subjectId: item.subjectId,
            operationId: item.operationId,
            requestDigest: item.requestDigest,
            request: item.request,
            createdAt: item.createdAt,
          });
          if (!isDeepStrictEqual(immutable(previous), immutable(next)))
            throw new CliError(
              "STATE_CONFLICT",
              "Receipt identity already belongs to another frozen request",
            );
          if (
            previous.status === "complete" ||
            previous.status === "rejected"
          ) {
            if (isDeepStrictEqual(previous, next)) return;
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
            try {
              const old = await receipt(file.slice(0, -5));
              if (old.status === "complete" || old.status === "rejected") {
                await unlink(join(operations, file));
                if (--remaining < policy.receipt_limit) break;
              }
            } catch {
              // Unknown or malformed files remain as preserved evidence.
            }
          }
          if (remaining >= policy.receipt_limit)
            throw new CliError(
              "RESOURCE_EXHAUSTED",
              "Unresolved operation receipts reached the configured limit; recover them before another mutation",
            );
        }
        await writeText(path, JSON.stringify(next));
      });
    },
  };
}
