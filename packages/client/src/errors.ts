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

export class NousError extends Error {
  readonly transportCode: Code;
  readonly errorCode?: string;
  readonly owner?: string;
  readonly recovery?: string;
  readonly context: Readonly<Record<string, string>>;
  readonly details: readonly unknown[];
  readonly candidates: readonly unknown[];
  readonly traceId?: string;

  constructor(error: ConnectError) {
    super(error.rawMessage, { cause: error });
    this.name = "NousError";
    this.transportCode = error.code;
    const typed = error.findDetails(ErrorDetailSchema);
    const identity = error.findDetails(ResolveIdentityResponseSchema);
    const primary = typed[0];
    this.context = primary?.context ?? {};
    this.traceId = primary?.traceId || undefined;

    const codeName = primary && ErrorCode[primary.code];
    if (primary && codeName && primary.code !== ErrorCode.UNSPECIFIED) {
      this.errorCode = codeName;
    }

    const ownerName = primary && ErrorOwner[primary.owner];
    if (primary && ownerName && primary.owner !== ErrorOwner.UNSPECIFIED) {
      this.owner = ownerName;
    }

    if (primary?.recovery && ErrorRecovery[primary.recovery])
      this.recovery = ErrorRecovery[primary.recovery];

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
