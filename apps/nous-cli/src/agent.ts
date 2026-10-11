// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { NousError } from "@nous-wave/client";

export class CliError extends Error {
  readonly owner = "CLI";
  constructor(
    readonly code: string,
    message: string,
    readonly details: unknown[] = [],
    readonly candidates: unknown[] = [],
    readonly recovery?: string,
    readonly context: Readonly<Record<string, string>> = {},
  ) {
    super(message);
  }
}

export function uniqueReference(result: {
  status: string;
  candidates: { canonical?: { kind: string; value: string } }[];
}) {
  const canonical = result.candidates[0]?.canonical;
  if (result.status !== "BOUND" || result.candidates.length !== 1 || !canonical)
    throw new CliError(
      result.status,
      "Reference must resolve uniquely",
      [],
      result.candidates,
    );
  return canonical;
}

export function boundedInteger(
  value: string | undefined,
  min: number,
  max: number,
  name: string,
) {
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < min || parsed > max)
    throw new CliError("INVALID_ARGUMENT", `${name} must be ${min}..${max}`);
  return parsed;
}

/** Parse an unsigned identity counter carried by the signed int64 protocol field. */
export function nonnegativeInt64(
  value: string | undefined,
  name: string,
): bigint {
  if (
    !value ||
    !/^(0|[1-9][0-9]*)$/.test(value) ||
    BigInt(value) > 9223372036854775807n
  )
    throw new CliError(
      "INVALID_ARGUMENT",
      `${name} must be a nonnegative int64`,
    );
  return BigInt(value);
}

export function cliErrorPayload(error: unknown) {
  const record =
    error && typeof error === "object"
      ? (error as Record<string, unknown>)
      : {};
  const receipt =
    typeof record.receiptId === "string" ? { receiptId: record.receiptId } : {};
  if (error instanceof CliError)
    return {
      ...(typeof record.transportCode === "number"
        ? { transportCode: record.transportCode }
        : {}),
      code: error.code,
      owner: error.owner,
      message: error.message,
      ...receipt,
      details: error.details,
      candidates: error.candidates,
      recovery: error.recovery,
      context: error.context,
    };
  const message =
    error instanceof Error ? error.message : "CLI operation failed";
  return {
    ...(typeof record.transportCode === "number"
      ? { transportCode: record.transportCode }
      : {}),
    ...(typeof record.errorCode === "string" ? { code: record.errorCode } : {}),
    ...(typeof record.owner === "string" ? { owner: record.owner } : {}),
    message,
    ...receipt,
    details:
      error instanceof NousError && Array.isArray(error.details)
        ? error.details
        : [],
    candidates:
      error instanceof NousError && Array.isArray(error.candidates)
        ? error.candidates
        : [],
    ...(typeof record.recovery === "string"
      ? { recovery: record.recovery }
      : {}),
    ...(record.context && typeof record.context === "object"
      ? { context: record.context }
      : {}),
    ...(typeof record.traceId === "string" ? { traceId: record.traceId } : {}),
  };
}
