// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { MessageInitShape } from "@bufbuild/protobuf";
import { Code, ConnectError } from "@connectrpc/connect";
import { protocolData as plain } from "./data.js";
import { ResolveIdentityResponseSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/identity_pb.js";
import {
  DomainErrorCode,
  ErrorRecovery,
  ErrorDetailSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/errors_pb.js";
export { DomainErrorCode, ErrorRecovery };

export function domainErrorDetail(error: unknown) {
  return error instanceof ConnectError
    ? error.findDetails(ErrorDetailSchema)[0]
    : undefined;
}

/** The semantic owner chooses both transport category and business recovery. */
export function domainError(
  message: string,
  transportCode: Code,
  detail: MessageInitShape<typeof ErrorDetailSchema>,
) {
  return new ConnectError(message, transportCode, undefined, [
    { desc: ErrorDetailSchema, value: detail },
  ]);
}

export class NousError extends Error {
  readonly code: Code;
  readonly domainCode?: string;
  readonly recovery?: string;
  readonly context: Readonly<Record<string, string>>;
  readonly details: readonly unknown[];
  readonly candidates: readonly unknown[];

  constructor(error: ConnectError) {
    super(error.rawMessage, { cause: error });
    this.name = "NousError";
    this.code = error.code;
    const domain = error.findDetails(ErrorDetailSchema);
    const identity = error.findDetails(ResolveIdentityResponseSchema);
    const primary = domain[0];
    // Unknown enum values retain their typed detail without inventing semantics.
    if (primary?.code && DomainErrorCode[primary.code])
      this.domainCode = DomainErrorCode[primary.code];
    if (primary?.recovery && ErrorRecovery[primary.recovery])
      this.recovery = ErrorRecovery[primary.recovery];
    this.context = primary?.context ?? {};
    const decodedTypes = new Set<string>([
      ErrorDetailSchema.typeName,
      ResolveIdentityResponseSchema.typeName,
    ]);
    this.details = [
      ...[...domain, ...identity].map((value) => plain(value)),
      ...error.details.filter(
        (detail) =>
          !decodedTypes.has(
            "desc" in detail ? detail.desc.typeName : detail.type,
          ),
      ),
    ];
    this.candidates = identity.flatMap((detail) =>
      detail.candidates.map((value) => plain(value)),
    );
  }
}
