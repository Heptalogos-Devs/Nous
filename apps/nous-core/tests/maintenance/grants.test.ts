// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { coreExecutionSchema } from "../../src/configuration/catalog.js";

import { describe, expect, it, vi } from "vitest";
import { Code, ConnectError } from "@connectrpc/connect";

import { type KernelClient } from "../../src/kernel-client.js";

import { ModelRuntime } from "../../src/model/runtime.js";
import { GenerationFailure } from "../../src/model/execution/routes.js";

import {
  grantMaintenance,
  SubjectMaintenanceScheduler,
} from "../../src/maintenance/grants.js";

import { subject, need, fixture } from "./support.js";

describe("bounded maintenance grants", () => {
  it("does not lease model work without executable roles or model budget", async () => {
    const state = fixture();
    let attempts = 0;
    const claim = vi.fn(async (input: { allowedKinds: string[] }) => {
      if (!input.allowedKinds.includes(need.kind)) return { needs: [] };
      attempts++;
      return { needs: [need] };
    });
    Object.assign(state.kernel.maintenance, {
      getMaintenancePolicy: vi.fn(async () => ({
        enabled: true,
        maxOperations: 4,
        experienceBatchSize: 256,
        workerLeaseSeconds: 120,
      })),
      claimMaintenance: claim,
      finishMaintenance: vi.fn(),
    });
    const input = {
      $typeName: "nous.wave.v1alpha1.MaintenanceGrantRequest" as const,
      subjectId: subject,
      maxOperations: 1,
      maxModelCalls: 1,
      maxElapsedMs: 1000,
    };
    for (let tick = 0; tick < 10; tick++)
      await grantMaintenance(state.kernel, state.models, input);
    expect(attempts).toBe(0);
    vi.spyOn(state.models.invocations, "capabilities", "get").mockReturnValue([
      { name: "model.journal_synthesis", state: "READY", detail: "Configured" },
    ]);
    await grantMaintenance(state.kernel, state.models, {
      ...input,
      maxModelCalls: 0,
    });
    expect(attempts).toBe(0);
    expect(state.synthesize).not.toHaveBeenCalled();
  });
  it("retains lease ownership while acknowledging an expired opportunity", async () => {
    const state = fixture();
    const claim = vi.fn(async () => ({ needs: [need] }));
    const finish = vi.fn(async () => ({}));
    Object.assign(state.kernel.maintenance, {
      getMaintenancePolicy: vi.fn(async () => ({
        enabled: true,
        maxOperations: 1,
        experienceBatchSize: 256,
        workerLeaseSeconds: 120,
        retryInitialSeconds: 1,
        retryMaxSeconds: 5,
        retryMaxAttempts: 4,
      })),
      claimMaintenance: claim,
      finishMaintenance: finish,
    });
    state.synthesize.mockRejectedValue(
      new ConnectError("Opportunity expired", Code.DeadlineExceeded),
    );
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const result = await grantMaintenance(state.kernel, state.models, {
        $typeName: "nous.wave.v1alpha1.MaintenanceGrantRequest",
        subjectId: subject,
        maxOperations: 1,
        maxModelCalls: 1,
        maxElapsedMs: 120000,
      });
      expect(claim).toHaveBeenCalledWith(
        expect.objectContaining({ leaseSeconds: 136 }),
        expect.anything(),
      );
      expect(finish).toHaveBeenCalledWith(
        expect.objectContaining({ disposition: "retry" }),
        expect.objectContaining({ timeoutMs: 5000 }),
      );
      expect(result.results[0]?.status).toBe("retry");
    } finally {
      log.mockRestore();
    }
  });
  it("defers unfinished work when the granted opportunity actually expires", async () => {
    const state = fixture();
    const originalSave = state.kernel.modelWorkflow.saveWorkflow;
    const save = vi.fn(async (...args: Parameters<typeof originalSave>) => {
      expect(args[1]?.signal?.aborted).toBe(false);
      await new Promise((resolve) => setTimeout(resolve, 10));
      return originalSave(...args);
    });
    state.kernel.modelWorkflow.saveWorkflow = save;
    const finish = vi.fn(
      async (_input: { nextDue?: { seconds: bigint } }) => ({}),
    );
    Object.assign(state.kernel.maintenance, {
      getMaintenancePolicy: vi.fn(async () => ({
        enabled: true,
        maxOperations: 1,
        experienceBatchSize: 256,
        workerLeaseSeconds: 120,
        retryMaxAttempts: 4,
      })),
      claimMaintenance: vi.fn(async () => ({ needs: [need] })),
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
      expect(typeof finish.mock.calls[0]?.[0].nextDue?.seconds).toBe("bigint");
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
        maxOperations: 4,
        experienceBatchSize: 256,
        workerLeaseSeconds: 120,
        retryInitialSeconds: 2,
        retryMaxSeconds: 5,
        retryMaxAttempts: 4,
      })),
      claimMaintenance: vi.fn(async () => ({
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
  it("continues through more than one Subject page under sustained due work", async () => {
    const ids = Array.from({ length: 75 }, (_, index) => `subject-${index}`);
    const opportunities: string[] = [];
    const list = vi.fn(async (input: { page?: { pageToken?: string } }) => {
      const offset = Number(input.page?.pageToken || 0);
      const items = ids
        .slice(offset, offset + 50)
        .map((subjectId) => ({ subjectId, capabilities: { memory: true } }));
      return {
        items,
        nextPageToken: offset + 50 < ids.length ? String(offset + 50) : "",
      };
    });
    const kernel = {
      execution: coreExecutionSchema.parse(undefined),
      subjects: { listSubjects: list },
      maintenance: {
        getMaintenancePolicy: vi.fn(async () => ({
          enabled: true,
          maxOperations: 4,
          experienceBatchSize: 7,
          workerLeaseSeconds: 120,
          maxModelCalls: 4,
          maxElapsedMs: 60000,
          pollIntervalSeconds: 30,
        })),
        claimMaintenance: vi.fn(async (input: { subjectId: string }) => ({
          needs: [
            { ...need, subjectId: input.subjectId, kind: "episode_segment" },
          ],
        })),
        organizeExperience: vi.fn(async (input: { subjectId: string }) => {
          opportunities.push(input.subjectId);
          return { episodes: [], nextDue: undefined };
        }),
        finishMaintenance: vi.fn(async () => ({})),
      },
    } as unknown as KernelClient;
    const scheduler = new SubjectMaintenanceScheduler(
      kernel,
      new ModelRuntime(),
    );
    for (let tick = 0; tick < 20; tick++)
      await scheduler.poll(new AbortController().signal);
    expect(kernel.maintenance.organizeExperience).toHaveBeenCalledWith(
      expect.objectContaining({ limit: 7 }),
      expect.anything(),
    );
    expect(opportunities.slice(0, 75)).toEqual(ids);
    expect(opportunities.slice(75)).toEqual(ids.slice(0, 5));
    expect(list).toHaveBeenCalledWith(
      expect.objectContaining({ page: { pageToken: "50" } }),
      expect.anything(),
    );
    list.mockRejectedValueOnce(
      new ConnectError("expired page token", Code.InvalidArgument),
    );
    for (let tick = 0; tick < 40; tick++)
      await scheduler.poll(new AbortController().signal);
    expect(
      opportunities.filter((id) => id === ids[74]).length,
    ).toBeGreaterThanOrEqual(2);
  });
  it("exposes an owner invariant failure and stops the grant without a provider retry", async () => {
    const state = fixture();
    state.commit.mockRejectedValue(
      new ConnectError("owner invariant", Code.Internal),
    );
    const finish = vi.fn(async () => ({}));
    const claim = vi.fn(async () => ({ needs: [need] }));
    Object.assign(state.kernel.maintenance, {
      getMaintenancePolicy: vi.fn(async () => ({
        enabled: true,
        maxOperations: 4,
        experienceBatchSize: 256,
        workerLeaseSeconds: 120,
        retryInitialSeconds: 1,
        retryMaxSeconds: 5,
        retryMaxAttempts: 4,
      })),
      claimMaintenance: claim,
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
      expect(claim).toHaveBeenCalledTimes(1);
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
