// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";
import { Code, ConnectError } from "@connectrpc/connect";

import { runModelMaintenance } from "../../src/maintenance/workflow.js";
import { maintenanceOperationId } from "../../src/maintenance/identity.js";

import { need, fixture } from "./support.js";

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
});
