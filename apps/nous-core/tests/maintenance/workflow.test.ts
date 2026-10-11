// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  domainError,
  DomainErrorCode,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import { ExpectedCognitionSchema } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { describe, expect, it, vi } from "vitest";
import { Code, ConnectError } from "@connectrpc/connect";
import { create } from "@bufbuild/protobuf";

import {
  MaintenanceNeedSchema,
  MaintenancePlanSchema,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";

import { consolidationSchema } from "../../src/model/schemas/consolidation.js";

import { runModelMaintenance } from "../../src/maintenance/workflow.js";
import { maintenanceOperationId } from "../../src/maintenance/identity.js";

import {
  subject,
  revision,
  need,
  plan,
  fixture,
  admitStub,
} from "./support.js";

describe("maintenance fixed workflow retry", () => {
  it("reuses the exact proposal after an owner response is lost and replays the outcome", async () => {
    const state = fixture();
    state.commit.mockRejectedValueOnce(
      new ConnectError("Response lost after commit", Code.Unavailable),
    );
    const reserveCall = vi.fn();
    await expect(
      runModelMaintenance(state.kernel, state.models, need, {}, reserveCall),
    ).rejects.toThrow();
    const first = state.commit.mock.calls[0];
    const result = await runModelMaintenance(
      state.kernel,
      state.models,
      need,
      {},
      reserveCall,
    );
    expect(result.status).toBe("committed");
    expect(state.commit.mock.calls[1]?.[0]).toEqual(first?.[0]);
    expect(state.synthesize).toHaveBeenCalledTimes(1);
    expect(reserveCall).toHaveBeenCalledTimes(1);
    expect(state.getPlan).toHaveBeenCalledTimes(1);
    expect(
      (
        await runModelMaintenance(
          state.kernel,
          state.models,
          need,
          {},
          reserveCall,
        )
      ).status,
    ).toBe("committed");
    expect(state.commit).toHaveBeenCalledTimes(2);
    expect(
      maintenanceOperationId({ ...need, triggerAuthoritySeq: 5n }),
    ).not.toBe(maintenanceOperationId(need));
  });
  it("records a stale outcome and refreshes the need; invented basis are terminal", async () => {
    const state = fixture();
    state.commit.mockRejectedValueOnce(
      domainError("Source changed", Code.Aborted, {
        code: DomainErrorCode.STALE_REVISION,
        recovery: ErrorRecovery.REFRESH_STATE,
      }),
    );
    expect(
      (
        await runModelMaintenance(
          state.kernel,
          state.models,
          need,
          {},
          () => {},
        )
      ).status,
    ).toBe("obsolete");
    expect(state.refresh).toHaveBeenCalledTimes(1);
    expect(
      (
        await runModelMaintenance(
          state.kernel,
          state.models,
          need,
          {},
          () => {},
        )
      ).status,
    ).toBe("obsolete");
    expect(state.synthesize).toHaveBeenCalledTimes(1);
    const invalid = fixture();
    const result = await invalid.synthesize("fixture");
    if (typeof result.value === "string" || result.value.action !== "commit")
      throw new Error("Expected commit");
    result.value.points[0]!.basisKeys = ["invented"];
    invalid.synthesize.mockResolvedValue(result);
    expect(
      (
        await runModelMaintenance(
          invalid.kernel,
          invalid.models,
          need,
          {},
          () => {},
        )
      ).status,
    ).toBe("rejected_invalid");
    expect(invalid.commit).not.toHaveBeenCalled();
    for (const code of [
      DomainErrorCode.OPERATION_ID_CONFLICT,
      DomainErrorCode.LEASE_LOST,
    ]) {
      const conflict = fixture();
      const error = domainError(
        "Different owner failure with the same transport category",
        Code.Aborted,
        {
          code,
          recovery:
            code === DomainErrorCode.LEASE_LOST
              ? ErrorRecovery.RETRY_OPERATION
              : ErrorRecovery.NEW_OPERATION,
        },
      );
      conflict.commit.mockRejectedValueOnce(error);
      await expect(
        runModelMaintenance(
          conflict.kernel,
          conflict.models,
          need,
          {},
          () => {},
        ),
      ).rejects.toBe(error);
      expect(conflict.refresh).not.toHaveBeenCalled();
      expect(
        vi
          .mocked(conflict.kernel.modelWorkflow.saveWorkflow)
          .mock.calls.some(([input]) => input.outcome !== undefined),
      ).toBe(false);
    }
  });

  it("executes consolidation through the typed owner and replays a no-change outcome", async () => {
    const state = fixture();
    const consolidationNeed = {
      ...need,
      kind: "memory_consolidate",
      scopeKind: "episode_revision",
      scopeRef: revision,
    };
    const consolidationPlan = create(MaintenancePlanSchema, {
      ...plan,
      consolidationSource: create(ExpectedCognitionSchema, {
        reference: { kind: "episode_revision", value: revision },
        expectedEpoch: 1n,
      }),
      maxConsolidationActions: 8,
    });
    state.getPlan.mockResolvedValue(consolidationPlan);
    const model = vi.spyOn(state.models, "consolidate").mockImplementation(
      admitStub({
        value: {
          actions: [
            {
              action: "skip",
              reason: "The source adds no reusable cognition.",
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
          promptId: "program/memory/consolidation.md",
          promptDigest: "b".repeat(64),
          outputSchemaDigest: "c".repeat(64),
          configDigest: "d".repeat(64),
        },
      }),
    );
    expect(
      (
        await runModelMaintenance(
          state.kernel,
          state.models,
          consolidationNeed,
          {},
          () => {},
        )
      ).status,
    ).toBe("no_change");
    expect(
      (
        await runModelMaintenance(
          state.kernel,
          state.models,
          consolidationNeed,
          {},
          () => {},
        )
      ).status,
    ).toBe("no_change");
    expect(model).toHaveBeenCalledTimes(1);
  });
});

it("routes concepts through bounded input and replays the saved outcome without another model call", async () => {
  const state = fixture();
  const topoNeed = create(MaintenanceNeedSchema, {
    ...need,
    kind: "concept_maintenance",
    scopeKind: "memory_revision",
    scopeRef: revision,
  });
  const modelInput = JSON.stringify({
    focusKey: "c0",
    cognition: [{ key: "c0", text: "Recorded approval before rollout" }],
    tags: [],
    basis: [{ key: "s0", kind: "exact_cognition", targetKey: "c0" }],
  });
  state.getPlan.mockResolvedValue(
    create(MaintenancePlanSchema, {
      subjectId: subject,
      authoritySeq: 1n,
      status: "ready",
      conceptCatalog: { maxSuggestions: 4 },
      conceptModelInputJson: modelInput,
    }),
  );
  const generate = vi
    .spyOn(state.models, "maintainConcepts")
    .mockImplementation(
      admitStub({
        value: { actions: [{ action: "no_change" }] },
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
          promptId: "program/memory/concept-maintenance.md",
          promptDigest: "b".repeat(64),
          outputSchemaDigest: "c".repeat(64),
          configDigest: "d".repeat(64),
        },
      }),
    );
  const reserveCall = vi.fn();
  const first = await runModelMaintenance(
    state.kernel,
    state.models,
    topoNeed,
    {},
    reserveCall,
  );
  expect(first.status).toBe("no_change");
  expect(
    (
      await runModelMaintenance(
        state.kernel,
        state.models,
        topoNeed,
        {},
        reserveCall,
      )
    ).status,
  ).toBe("no_change");
  expect(generate).toHaveBeenCalledTimes(1);
  expect(generate.mock.calls[0]?.[0]).toBe(modelInput);
  expect(reserveCall).toHaveBeenCalledTimes(1);
  expect(state.synthesize).not.toHaveBeenCalled();
});

it("replays a committed consolidation action after a lost response without another model call", async () => {
  const state = fixture();
  const item = {
    cognitiveRole: "declarative",
    formationMode: "synthesized",
    groundingMemberKey: null,
    semanticRole: "statement",
    text: "A durable supported conclusion",
    title: null,
    basisKeys: ["source"],
    entityKeys: [],
    validTime: { kind: "unknown" },
    epistemicClass: "derived",
  };
  state.getPlan.mockResolvedValue(
    create(MaintenancePlanSchema, {
      ...plan,
      maxConsolidationActions: 4,
      consolidationSource: create(ExpectedCognitionSchema, {
        reference: { kind: "episode_revision", value: revision },
        expectedEpoch: 1n,
      }),
    }),
  );
  const metadata = (await state.synthesize("fixture")).producerMetadata;
  const model = vi.spyOn(state.models, "consolidate").mockImplementation(
    admitStub({
      value: consolidationSchema.parse({
        actions: [
          { action: "create_memory", content: item },
          {
            action: "create_memory",
            content: { ...item, basisKeys: ["invented"] },
          },
        ],
      }),
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
      producerMetadata: metadata,
    }),
  );
  const receipts = new Map<string, { revisionId: string }>();
  let lost = true;
  const form = vi.fn(async (request: { operationId: string }) => {
    if (receipts.has(request.operationId))
      return receipts.get(request.operationId)!;
    const result = { revisionId: revision };
    receipts.set(request.operationId, result);
    if (lost) {
      lost = false;
      throw new ConnectError(
        "lost committed action response",
        Code.Unavailable,
      );
    }
    return result;
  });
  Object.assign(state.kernel, { memory: { formMemory: form } });
  const reserve = vi.fn();
  const current = { ...need, kind: "memory_consolidate" };
  const run = () =>
    runModelMaintenance(state.kernel, state.models, current, {}, reserve);
  await expect(run()).rejects.toThrow("lost committed action response");
  const outcome = await run();
  expect(outcome.status).toBe("partial");
  expect(
    "actions" in outcome && outcome.actions?.map((action) => action.status),
  ).toEqual(["committed", "rejected_invalid"]);
  expect(receipts.size).toBe(1);
  expect(form.mock.calls[0]?.[0]).toEqual(form.mock.calls[1]?.[0]);
  expect(model).toHaveBeenCalledTimes(1);
  expect(reserve).toHaveBeenCalledTimes(1);
  expect((await run()).status).toBe("partial");
  expect(form).toHaveBeenCalledTimes(2);
});
