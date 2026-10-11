// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { expect, it } from "vitest";
import { ConnectError, Code } from "@connectrpc/connect";
import { ResolveIdentityResponseSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/identity_pb.js";
import { ErrorDetailSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/errors_pb.js";
import { NousError } from "../src/index.js";
import { domainError, DomainErrorCode, ErrorRecovery } from "../src/errors.js";
it("preserves identity candidates and business recovery independently of display text", () => {
  const result = {
    status: "AMBIGUOUS_REFERENCE",
    candidates: [
      { canonical: { kind: "entity", value: "Alice-1" } },
      { canonical: { kind: "entity", value: "Alice-2" } },
    ],
  };
  const transport = new ConnectError(
    "Choose one identity",
    Code.InvalidArgument,
    undefined,
    [
      {
        desc: ErrorDetailSchema,
        value: {
          code: DomainErrorCode.AMBIGUOUS_REFERENCE,
          recovery: ErrorRecovery.SELECT_CANDIDATE,
          context: { reference: "Alice" },
        },
      },
      { desc: ResolveIdentityResponseSchema, value: result },
    ],
  );
  transport.details.push({
    type: "future.example.Detail",
    value: new Uint8Array([1, 2]),
  });
  const error = new NousError(transport);
  expect(error).toMatchObject({
    message: "Choose one identity",
    domainCode: "AMBIGUOUS_REFERENCE",
    recovery: "SELECT_CANDIDATE",
    context: { reference: "Alice" },
    candidates: result.candidates,
  });
  expect(error.details[1]).toMatchObject({ status: result.status });
  expect(error.details[2]).toMatchObject({ type: "future.example.Detail" });
});

it("does not reinterpret generic or unknown transport details as a domain failure", () => {
  const generic = new NousError(
    new ConnectError("STALE_CONTEXT in display only", Code.Aborted),
  );
  expect(generic.domainCode).toBeUndefined();
  const unknown = new NousError(
    domainError("future owner failure", Code.Aborted, {
      code: 99 as DomainErrorCode,
      recovery: 99 as ErrorRecovery,
      context: { operation_id: "future" },
    }),
  );
  expect(unknown.domainCode).toBeUndefined();
  expect(unknown.recovery).toBeUndefined();
  expect(unknown.context).toEqual({ operation_id: "future" });
  expect(unknown.details).toMatchObject([{ code: 99, recovery: 99 }]);
});
