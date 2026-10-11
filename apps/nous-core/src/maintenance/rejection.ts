// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { Code, ConnectError } from "@connectrpc/connect";
import { domainErrorDetail, ErrorRecovery } from "@nous-wave/client/errors";

/** Classify a rejected supplied Authority action; leases and identity conflicts escape. */
export function authorityRejection(error: unknown) {
  if (!(error instanceof ConnectError)) return undefined;
  const detail = domainErrorDetail(error);
  if (detail) {
    switch (detail.recovery) {
      case ErrorRecovery.REFRESH_STATE:
      case ErrorRecovery.REDISCOVER_REFERENCE:
        return "stale";
      case ErrorRecovery.CORRECT_REQUEST:
      case ErrorRecovery.RESOLVE_REFERENCE:
      case ErrorRecovery.SELECT_CANDIDATE:
        return "rejected_invalid";
      default:
        return undefined;
    }
  }
  if ([Code.InvalidArgument, Code.FailedPrecondition].includes(error.code))
    return "rejected_invalid";
  // The action selected an object from the supplied catalog which has disappeared.
  if (error.code === Code.NotFound) return "stale";
  return undefined;
}
