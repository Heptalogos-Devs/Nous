// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";
import { Code, ConnectError } from "@connectrpc/connect";

import { GenerationFailure } from "../../src/model/execution/routes.js";

import { grantMaintenance } from "../../src/maintenance/grants.js";

import { subject, need, fixture } from "./support.js";

describe("bounded maintenance grants", () => {
  it("defers unfinished work when the granted opportunity actually expires", async () => {
    const state = fixture();
    const originalSave = state.kernel.modelResults.saveOperation;
    const save = vi.fn(async (...args: Parameters<typeof originalSave>) => {
      expect(args[1]?.signal?.aborted).toBe(false);
      await new Promise((resolve) => setTimeout(resolve, 10));
      return originalSave(...args);
    });
    state.kernel.modelResults.saveOperation = save;
    const finish = vi.fn(
      async (_input: { nextDue?: { seconds: bigint } }) => ({}),
    );
    Object.assign(state.kernel.maintenance, {
      getMaintenancePolicy: vi.fn(async () => ({
        enabled: true,
        maxModelCalls: 4,
        maxOperations: 1,
        experienceBatchSize: 256,
        retryMaxAttempts: 4,
      })),
      selectMaintenance: vi.fn(async () => ({ needs: [need] })),
      finishMaintenance: finish,
    });
    state.synthesize.mockImplementation(
      async (_input, signal, _snapshot, beforeAttempt) => {
        beforeAttempt?.();
        if (!signal) throw new Error("missing opportunity signal");
        return new Promise<never>((_done, reject) =>
          signal.addEventListener(
            "abort",
            () =>
              reject(
                new GenerationFailure("journal_synthesis", "caller_cancelled", {
                  attempts: [
                    {
                      executionProfile: "stub",
                      modelProfile: "stub",
                      status: "unknown",
                      latencyMs: 30,
                      failureClass: "caller_cancelled",
                    },
                  ],
                }),
              ),
            {
              once: true,
            },
          ),
        );
      },
    );
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const result = await grantMaintenance(state.kernel, state.models, {
        $typeName: "nous.wave.v1alpha1.MaintenanceGrantRequest",
        subjectId: subject,
        maxOperations: 1,
        maxModelCalls: 1,
        maxElapsedMs: 30,
      });
      expect(result.results[0]).toMatchObject({
        status: "deferred",
        problemCode: "opportunity_budget_exhausted",
      });
      expect(finish).toHaveBeenCalledWith(
        expect.objectContaining({
          disposition: "pending",
          retryDelaySeconds: 0,
        }),
        expect.anything(),
      );
      expect(finish.mock.calls[0]?.[0].nextDue).toBeUndefined();
      const saved = save.mock.calls[0];
      expect(saved![0].executionTelemetry).toMatchObject({
        attempts: [{ status: "unknown" }],
      });
      expect(saved?.[1]?.signal).toBeInstanceOf(AbortSignal);
    } finally {
      log.mockRestore();
    }
  });
  it("uses bounded exponential retry for provider failure and then blocks", async () => {
    const state = fixture();
    vi.spyOn(state.models.invocations, "capabilities", "get").mockReturnValue([
      { name: "model.journal_synthesis", state: "READY", detail: "Configured" },
    ]);
    state.synthesize.mockRejectedValue(
      new ConnectError("provider unavailable", Code.Unavailable),
    );
    let retryCount = 0;
    const finish = vi.fn(async (input: { disposition: string }) => {
      if (input.disposition === "retry") retryCount++;
      return {};
    });
    Object.assign(state.kernel.maintenance, {
      getMaintenancePolicy: vi.fn(async () => ({
        enabled: true,
        maxModelCalls: 4,
        maxOperations: 4,
        experienceBatchSize: 256,
        retryInitialSeconds: 2,
        retryMaxSeconds: 5,
        retryMaxAttempts: 4,
      })),
      selectMaintenance: vi.fn(async () => ({
        needs: [{ ...need, retryCount }],
      })),
      finishMaintenance: finish,
    });
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    for (let tick = 0; tick < 4; tick++)
      await grantMaintenance(state.kernel, state.models, {
        $typeName: "nous.wave.v1alpha1.MaintenanceGrantRequest",
        subjectId: subject,
        maxOperations: 1,
        maxModelCalls: 1,
        maxElapsedMs: 1000,
      });
    expect(finish.mock.calls.map(([input]) => input)).toMatchObject([
      { disposition: "retry", retryDelaySeconds: 2 },
      { disposition: "retry", retryDelaySeconds: 4 },
      { disposition: "retry", retryDelaySeconds: 5 },
      { disposition: "blocked", problemCode: "maintenance_retry_exhausted" },
    ]);
    expect(log).toHaveBeenCalled();
    log.mockRestore();
  });
  it("exposes an owner invariant failure and stops the grant without a provider retry", async () => {
    const state = fixture();
    state.commit.mockRejectedValue(
      new ConnectError("owner invariant", Code.Internal),
    );
    const finish = vi.fn(async () => ({}));
    const select = vi.fn(async () => ({ needs: [need] }));
    Object.assign(state.kernel.maintenance, {
      getMaintenancePolicy: vi.fn(async () => ({
        enabled: true,
        maxModelCalls: 4,
        maxOperations: 4,
        experienceBatchSize: 256,
        retryInitialSeconds: 1,
        retryMaxSeconds: 5,
        retryMaxAttempts: 4,
      })),
      selectMaintenance: select,
      finishMaintenance: finish,
    });
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const result = await grantMaintenance(state.kernel, state.models, {
        $typeName: "nous.wave.v1alpha1.MaintenanceGrantRequest",
        subjectId: subject,
        maxOperations: 4,
        maxModelCalls: 4,
        maxElapsedMs: 1000,
      });
      expect(result.results[0]?.status).toBe("internal_failure");
      expect(select).toHaveBeenCalledTimes(1);
      expect(finish).toHaveBeenCalledWith(
        expect.objectContaining({
          disposition: "blocked",
          retryDelaySeconds: 0,
        }),
        expect.anything(),
      );
    } finally {
      log.mockRestore();
    }
  });
});
