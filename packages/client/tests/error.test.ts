// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { expect, it } from "vitest";
import { ConnectError, Code } from "@connectrpc/connect";
import { create } from "@bufbuild/protobuf";
import { ResolveIdentityResponseSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/identity_pb.js";
import {
  ErrorCode,
  ErrorDetailSchema,
  ErrorOwner,
  ErrorRecovery,
} from "@nous-wave/protocol/nous/wave/v1alpha1/errors_pb.js";
import { NousError } from "../src/index.js";
import { domainError } from "../src/errors.js";

it("preserves identity candidates and owner recovery independently of display text", () => {
  const result = {
    status: "AMBIGUOUS_REFERENCE",
    candidates: [
      { canonical: { kind: "entity", value: "Alice-1" } },
      { canonical: { kind: "entity", value: "Alice-2" } },
    ],
  };
  const transport = domainError("Choose one identity", Code.InvalidArgument, {
    code: ErrorCode.AMBIGUOUS_REFERENCE,
    recovery: ErrorRecovery.SELECT_CANDIDATE,
    owner: ErrorOwner.IDENTITY,
    context: { reference: "Alice" },
  });
  transport.details.push({
    desc: ResolveIdentityResponseSchema,
    value: create(ResolveIdentityResponseSchema, result),
  });
  transport.details.push({
    type: "future.example.Detail",
    value: new Uint8Array([1, 2]),
  });
  const error = new NousError(transport);
  expect(error).toMatchObject({
    message: "Choose one identity",
    transportCode: Code.InvalidArgument,
    errorCode: "AMBIGUOUS_REFERENCE",
    owner: "IDENTITY",
    recovery: "SELECT_CANDIDATE",
    context: { reference: "Alice" },
    candidates: result.candidates,
  });
  expect(error.traceId).toMatch(/^[0-9a-f-]{36}$/);
  expect(error.details[1]).toMatchObject({ status: result.status });
  expect(error.details[2]).toMatchObject({ type: "future.example.Detail" });
});

it("preserves typed error fields without validating them against transport status", () => {
  const generic = new NousError(
    new ConnectError("STALE_CONTEXT in display only", Code.Aborted),
  );
  expect(generic.errorCode).toBeUndefined();

  const unknown = new NousError(
    domainError("future owner failure", Code.Aborted, {
      code: 99 as Exclude<ErrorCode, ErrorCode.UNSPECIFIED>,
      recovery: 99 as ErrorRecovery,
      owner: ErrorOwner.RUNTIME,
      context: { operation_id: "future" },
    }),
  );
  expect(unknown.errorCode).toBeUndefined();
  expect(unknown.owner).toBe("RUNTIME");
  expect(unknown.recovery).toBeUndefined();
  expect(unknown.context).toEqual({ operation_id: "future" });
  expect(unknown.details).toMatchObject([{ code: 99, recovery: 99 }]);

  const mismatch = new NousError(
    new ConnectError("typed detail", Code.Unavailable, undefined, [
      {
        desc: ErrorDetailSchema,
        value: create(ErrorDetailSchema, {
          code: ErrorCode.STALE_REVISION,
          recovery: ErrorRecovery.REFRESH_STATE,
          owner: ErrorOwner.MEMORY,
          traceId: "trace-1",
        }),
      },
    ]),
  );
  expect(mismatch.errorCode).toBe("STALE_REVISION");
  expect(mismatch.transportCode).toBe(Code.Unavailable);
  expect(mismatch.owner).toBe("MEMORY");
  expect(mismatch.recovery).toBe("REFRESH_STATE");
  expect(mismatch.traceId).toBe("trace-1");
  expect(mismatch.details).toMatchObject([
    { code: "ERROR_CODE_STALE_REVISION" },
  ]);
});
