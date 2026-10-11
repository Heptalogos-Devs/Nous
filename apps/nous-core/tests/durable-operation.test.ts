// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, type MessageInitShape } from "@bufbuild/protobuf";
import { ReserveWorkflowRequestSchema } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js";
import { WorkflowSnapshotSchema } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/workflow_envelope_pb.js";
import { coreExecutionSchema } from "../src/configuration/catalog.js";
import {
  workflowPayload,
  openDurableOperation,
} from "../src/durable-operation.js";
import { expect, it, vi } from "vitest";
import type { KernelClient } from "../src/kernel-client.js";

const identity = {
  subjectId: "10000000-0000-4000-8000-000000000001",
  owner: "material",
  operationKey: "operation-key",
  semanticDigest: "frozen-input-digest",
};

it("uses the reservation winner snapshot and checkpoints each owner ID before commit", async () => {
  const events: string[] = [];
  const persistedSnapshot = create(WorkflowSnapshotSchema, {
    content: workflowPayload({ source: "reservation winner" }),
  });
  const reserveWorkflow = vi.fn(
    async (_input: MessageInitShape<typeof ReserveWorkflowRequestSchema>) => ({
      state: {
        case: "acquired",
        value: {
          pending: { snapshot: persistedSnapshot },
          lease: { token: "child-token" },
        },
      },
    }),
  );
  const kernel = {
    execution: coreExecutionSchema.parse(undefined),
    modelWorkflow: {
      findWorkflow: vi.fn(async () => ({ state: { case: undefined } })),
      reserveWorkflow,
      saveWorkflow: vi.fn(async (input: { mutationOperationId?: string }) => {
        events.push(`checkpoint:${input.mutationOperationId}`);
      }),
      releaseWorkflow: vi.fn(async () => ({})),
    },
  } as unknown as KernelClient;
  const snapshotFactory = vi.fn(async () => ({
    snapshot: {
      content: workflowPayload({ source: "local candidate" }),
    },
  }));
  const opened = await openDurableOperation(kernel, identity, {
    execution: {},
    snapshotFactory,
  });

  expect(opened.state).toBe("acquired");
  if (opened.state !== "acquired") throw new Error("Expected acquisition");
  expect(opened.snapshot.content?.payloadJson).toBe(
    persistedSnapshot.content?.payloadJson,
  );
  expect(
    reserveWorkflow.mock.calls[0]?.[0].snapshot?.content?.payloadJson,
  ).toBe(JSON.stringify({ source: "local candidate" }));
  await opened.operation.mutate("stable-owner-id", async () => {
    events.push("owner-commit");
  });
  expect(events).toEqual(["checkpoint:stable-owner-id", "owner-commit"]);
});

it("replays a found terminal outcome without rebuilding a snapshot or reserving", async () => {
  const outcome = workflowPayload({ revisionId: "exact-revision" });
  const snapshotFactory = vi.fn();
  const reserveWorkflow = vi.fn();
  const kernel = {
    execution: coreExecutionSchema.parse(undefined),
    modelWorkflow: {
      findWorkflow: vi.fn(async () => ({
        state: { case: "completed", value: { outcome } },
      })),
      reserveWorkflow,
    },
  } as unknown as KernelClient;

  const opened = await openDurableOperation(kernel, identity, {
    execution: {},
    snapshotFactory,
  });

  expect(opened).toEqual({
    state: "completed",
    outcome,
    executionTelemetry: undefined,
  });
  expect(snapshotFactory).not.toHaveBeenCalled();
  expect(reserveWorkflow).not.toHaveBeenCalled();
});
