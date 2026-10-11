// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, toJson, type MessageInitShape } from "@bufbuild/protobuf";
import { Code, ConnectError } from "@connectrpc/connect";
import {
  IdentityBindingSchema,
  ResolveIdentityResponseSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/identity_pb.js";
import {
  ErrorCode,
  ErrorOwner,
  ErrorRecovery,
  ErrorDetailSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/errors_pb.js";
export { ErrorCode, ErrorOwner, ErrorRecovery };

export function domainErrorDetail(error: unknown) {
  return error instanceof ConnectError
    ? error.findDetails(ErrorDetailSchema)[0]
    : undefined;
}

type OwnedErrorDetailInit = Omit<
  MessageInitShape<typeof ErrorDetailSchema>,
  "$typeName" | "code" | "owner" | "traceId"
> & {
  code: Exclude<ErrorCode, ErrorCode.UNSPECIFIED>;
  owner: Exclude<ErrorOwner, ErrorOwner.UNSPECIFIED>;
};

/** The semantic owner supplies the wire code; the helper adds one correlation ID. */
export function domainError(
  message: string,
  transportCode: Code,
  detail: OwnedErrorDetailInit,
) {
  return new ConnectError(message, transportCode, undefined, [
    {
      desc: ErrorDetailSchema,
      value: create(ErrorDetailSchema, {
        ...detail,
        owner: detail.owner as ErrorOwner,
        traceId: globalThis.crypto.randomUUID(),
      }),
    },
  ]);
}

function expectedTransportCode(code: ErrorCode): Code | undefined {
  switch (code) {
    case ErrorCode.UNKNOWN_REFERENCE:
      return Code.NotFound;
    case ErrorCode.AMBIGUOUS_REFERENCE:
    case ErrorCode.REFERENCE_TYPE_MISMATCH:
    case ErrorCode.UNRESOLVED_MACHINE_PLACEHOLDER:
    case ErrorCode.INVALID_REQUEST:
      return Code.InvalidArgument;
    case ErrorCode.REFERENCE_TOMBSTONED:
    case ErrorCode.STALE_CONTEXT:
    case ErrorCode.FAILED_PRECONDITION:
      return Code.FailedPrecondition;
    case ErrorCode.STALE_REVISION:
    case ErrorCode.OPERATION_ID_CONFLICT:
    case ErrorCode.OPERATION_IN_PROGRESS:
    case ErrorCode.LEASE_LOST:
    case ErrorCode.CONFLICT:
      return Code.Aborted;
    case ErrorCode.CAPABILITY_UNAVAILABLE:
    case ErrorCode.UNAVAILABLE:
      return Code.Unavailable;
    case ErrorCode.NOT_FOUND:
      return Code.NotFound;
    case ErrorCode.INTERNAL_FAILURE:
      return Code.Internal;
    case ErrorCode.CAPACITY_EXHAUSTED:
      return Code.ResourceExhausted;
    case ErrorCode.INSTANCE_ALREADY_RUNNING:
      return Code.AlreadyExists;
    default:
      return undefined;
  }
}

function expectedRecovery(code: ErrorCode): ErrorRecovery | undefined {
  switch (code) {
    case ErrorCode.UNKNOWN_REFERENCE:
      return ErrorRecovery.RESOLVE_REFERENCE;
    case ErrorCode.AMBIGUOUS_REFERENCE:
      return ErrorRecovery.SELECT_CANDIDATE;
    case ErrorCode.REFERENCE_TYPE_MISMATCH:
    case ErrorCode.INVALID_REQUEST:
      return ErrorRecovery.CORRECT_REQUEST;
    case ErrorCode.REFERENCE_TOMBSTONED:
      return ErrorRecovery.REDISCOVER_REFERENCE;
    case ErrorCode.UNRESOLVED_MACHINE_PLACEHOLDER:
      return ErrorRecovery.RESOLVE_REFERENCE;
    case ErrorCode.STALE_CONTEXT:
    case ErrorCode.STALE_REVISION:
    case ErrorCode.CAPACITY_EXHAUSTED:
      return ErrorRecovery.REFRESH_STATE;
    case ErrorCode.OPERATION_ID_CONFLICT:
      return ErrorRecovery.NEW_OPERATION;
    case ErrorCode.OPERATION_IN_PROGRESS:
    case ErrorCode.LEASE_LOST:
      return ErrorRecovery.RETRY_OPERATION;
    case ErrorCode.CAPABILITY_UNAVAILABLE:
    case ErrorCode.INSTANCE_ALREADY_RUNNING:
      return ErrorRecovery.CHECK_CONFIGURATION;
    case ErrorCode.NOT_FOUND:
    case ErrorCode.CONFLICT:
    case ErrorCode.FAILED_PRECONDITION:
    case ErrorCode.UNAVAILABLE:
    case ErrorCode.INTERNAL_FAILURE:
      return ErrorRecovery.UNSPECIFIED;
    default:
      return undefined;
  }
}

function protocolError(reason: string, error: ConnectError, detail?: unknown) {
  const record = detail && typeof detail === "object" ? detail : undefined;
  const owner = record && "owner" in record ? record.owner : undefined;
  const code = record && "code" in record ? record.code : undefined;
  console.error(
    JSON.stringify({
      event: "protocol_error_detail_invalid",
      reason,
      transportCode: Code[error.code] ?? "unknown",
      owner,
      code,
    }),
  );
}

export class NousError extends Error {
  readonly transportCode: Code;
  readonly errorCode?: string;
  readonly owner?: string;
  readonly recovery?: string;
  readonly context: Readonly<Record<string, string>>;
  readonly details: readonly unknown[];
  readonly candidates: readonly unknown[];
  readonly traceId?: string;
  readonly semanticComplete: boolean;

  constructor(error: ConnectError) {
    super(error.rawMessage, { cause: error });
    this.name = "NousError";
    this.transportCode = error.code;
    const typed = error.findDetails(ErrorDetailSchema);
    const identity = error.findDetails(ResolveIdentityResponseSchema);
    const primary = typed[0];
    let semanticComplete = false;
    this.context = primary?.context ?? {};
    this.traceId = primary?.traceId || undefined;

    const codeName = primary && ErrorCode[primary.code];
    if (primary && codeName && primary.code !== ErrorCode.UNSPECIFIED) {
      this.errorCode = codeName;
      const expected = expectedTransportCode(primary.code);
      if (expected === undefined || expected !== error.code) {
        protocolError("code_status_mismatch", error, primary);
      } else {
        semanticComplete = true;
      }
    } else if (primary) {
      protocolError("unknown_or_unspecified_code", error, primary);
    } else {
      protocolError("missing_error_detail", error);
    }

    const ownerName = primary && ErrorOwner[primary.owner];
    if (primary && ownerName && primary.owner !== ErrorOwner.UNSPECIFIED) {
      this.owner = ownerName;
    } else if (primary) {
      protocolError("missing_or_unknown_owner", error, primary);
      semanticComplete = false;
    }

    if (primary?.recovery && ErrorRecovery[primary.recovery])
      this.recovery = ErrorRecovery[primary.recovery];
    if (primary) {
      const expected = expectedRecovery(primary.code);
      if (expected === undefined || primary.recovery !== expected) {
        protocolError("recovery_code_mismatch", error, primary);
        semanticComplete = false;
      }
    }
    this.semanticComplete = semanticComplete;

    const decodedTypes = new Set<string>([
      ErrorDetailSchema.typeName,
      ResolveIdentityResponseSchema.typeName,
    ]);
    this.details = [
      ...typed.map((value) => toJson(ErrorDetailSchema, value)),
      ...identity.map((value) => toJson(ResolveIdentityResponseSchema, value)),
      ...error.details.filter(
        (detail) =>
          !decodedTypes.has(
            "desc" in detail ? detail.desc.typeName : detail.type,
          ),
      ),
    ];
    this.candidates = identity.flatMap((detail) =>
      detail.candidates.map((value) => toJson(IdentityBindingSchema, value)),
    );
  }
}
