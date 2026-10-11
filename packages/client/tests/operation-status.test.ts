// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, type DescMessage } from "@bufbuild/protobuf";
import type { Transport } from "@connectrpc/connect";
import { expect, test, vi } from "vitest";
import {
  GetOperationStatusRequestSchema,
  OperationState,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { ErrorOwner } from "@nous-wave/protocol/nous/wave/v1alpha1/errors_pb.js";
import { createNousClient } from "../src/index.js";

test("system.operationStatus calls the typed status RPC", async () => {
  const unary = vi.fn(
    async (method: { name: string; output: DescMessage }) => ({
      message: create(method.output, {
        state: OperationState.COMMITTED,
        owner: ErrorOwner.SUBJECT,
        subjectId: "4b44be4d-1992-4f39-b8cb-f7a593204f24",
      }),
    }),
  );
  const client = createNousClient({ unary } as unknown as Transport);
  const subjectId = "4b44be4d-1992-4f39-b8cb-f7a593204f24";
  const request = create(GetOperationStatusRequestSchema, {
    identity: {
      case: "subjectCreate",
      value: {
        subjectId,
        operationId: "4f7f1ff0-bcdc-4c07-8142-43a42a7761bb",
      },
    },
  });

  const result = await client.system.operationStatus(request);

  expect(unary).toHaveBeenCalledOnce();
  expect(unary.mock.calls[0]?.[0].name).toBe("GetOperationStatus");
  expect(request.identity.case).toBe("subjectCreate");
  expect(result).toMatchObject({
    state: OperationState.COMMITTED,
    owner: ErrorOwner.SUBJECT,
    subjectId,
  });
});
