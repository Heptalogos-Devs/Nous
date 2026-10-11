// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, type MessageInitShape } from "@bufbuild/protobuf";
import {
  WorkflowSnapshotSchema,
  WorkflowPayloadSchema,
  type WorkflowSnapshot,
  type WorkflowPayload,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/workflow_envelope_pb.js";
import {
  ReserveWorkflowRequestSchema,
  SaveWorkflowRequestSchema,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js";
import { coreExecutionSchema } from "../../src/configuration/catalog.js";

import { vi } from "vitest";

import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import {
  MaintenanceNeedSchema,
  MaintenancePlanSchema,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { type KernelClient } from "../../src/kernel-client.js";

import { ModelRuntime } from "../../src/model/runtime.js";

import { type ModelRoleSnapshot } from "../../src/model/execution/snapshot.js";

export const subject = "10000000-0000-4000-8000-000000000001";
export const revision = "20000000-0000-4000-8000-000000000001";
export const need = create(MaintenanceNeedSchema, {
  subjectId: subject,
  needId: "30000000-0000-4000-8000-000000000001",
  kind: "journal_review",
  scopeKind: "track",
  scopeRef: "interaction",
  triggerAuthoritySeq: 4n,
});
export const plan = create(MaintenancePlanSchema, {
  subjectId: subject,
  authoritySeq: 4n,
  cognitiveNow: timestampFromDate(new Date("2026-10-03T00:00:00Z")),
  status: "ready",
  sources: [{ revisionId: revision, expectedEpoch: 1n }],
  basis: [
    {
      key: "source",
      basis: {
        basis: {
          case: "cognitionDependency",
          value: {
            targetRevision: { kind: "episode_revision", value: revision },
            basisRole: "direct",
          },
        },
      },
    },
  ],
});
export function admitStub<T>(value: T) {
  return async (
    _input: string,
    _signal?: AbortSignal,
    _snapshot?: ModelRoleSnapshot,
    beforeAttempt?: () => void,
  ) => {
    beforeAttempt?.();
    return value;
  };
}
export function fixture() {
  const models = new ModelRuntime();
  vi.spyOn(models.invocations, "snapshot").mockReturnValue(
    {} as ModelRoleSnapshot,
  );
  const synthesize = vi
    .spyOn(models, "synthesizeJournal")
    .mockImplementation(async (_input, _signal, _snapshot, beforeAttempt) => {
      beforeAttempt?.();
      return {
        value: {
          action: "commit",
          title: null,
          narrative: "A supported decision.",
          points: [
            {
              role: "decision",
              text: "Continue the plan.",
              basisKeys: ["source"],
            },
          ],
        },
        execution: {
          attempts: [
            {
              executionProfile: "stub",
              modelProfile: "stub",
              status: "succeeded" as const,
              latencyMs: 0,
            },
          ],
          successfulExecutionProfile: "stub",
        },
        producerMetadata: {
          modelRole: "journal_synthesis" as const,
          modelProfile: "stub",
          executionProfile: "stub",
          inferenceControlsDigest: "e".repeat(64),
          rolePolicyDigest: "f".repeat(64),
          implementation: "semantic-stub",
          protocol: "openai-chat",
          model: "stub",
          profileDigest: "a".repeat(64),
          promptId: "program/journal/synthesis.md",
          promptDigest: "b".repeat(64),
          outputSchemaDigest: "c".repeat(64),
          configDigest: "d".repeat(64),
        },
      };
    });
  let snapshot: WorkflowSnapshot | undefined;
  let proposal: WorkflowPayload | undefined;
  let outcome: WorkflowPayload | undefined;
  const commit = vi.fn(
    async (
      _input: Parameters<KernelClient["maintenance"]["commitJournal"]>[0],
    ) => ({}),
  );
  const refresh = vi.fn(async () => ({}));
  const getPlan = vi.fn(async () => plan);
  const kernel = {
    execution: coreExecutionSchema.parse(undefined),
    maintenance: {
      planMaintenance: getPlan,
      commitJournal: commit,
      refreshMaintenance: refresh,
    },
    modelWorkflow: {
      findWorkflow: vi.fn(async () => ({
        found: !!snapshot,
        snapshot,
        proposal,
        outcome,
      })),
      reserveWorkflow: vi.fn(
        async (
          input: MessageInitShape<typeof ReserveWorkflowRequestSchema>,
        ) => {
          snapshot ??= create(WorkflowSnapshotSchema, input.snapshot);
          return {
            snapshot,
            proposal,
            outcome,
            lease: { token: "lease" },
            busy: false,
          };
        },
      ),
      saveWorkflow: vi.fn(
        async (input: MessageInitShape<typeof SaveWorkflowRequestSchema>) => {
          if (input.proposal)
            proposal = create(WorkflowPayloadSchema, input.proposal);
          if (input.outcome)
            outcome = create(WorkflowPayloadSchema, input.outcome);
          return {};
        },
      ),
      releaseWorkflow: vi.fn(async () => ({})),
    },
  } as unknown as KernelClient;
  return { kernel, models, synthesize, commit, refresh, getPlan };
}
