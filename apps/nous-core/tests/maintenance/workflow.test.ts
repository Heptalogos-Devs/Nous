// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";
import { runModelMaintenance } from "../../src/maintenance/workflow.js";
import { maintenanceOperationId } from "../../src/maintenance/identity.js";
import { need, fixture, revision } from "./support.js";

describe("maintenance model execution", () => {
  it("plans, proposes and writes directly through the canonical owner", async () => {
    const state = fixture();
    const reserveCall = vi.fn();

    const result = await runModelMaintenance(
      state.kernel,
      state.models,
      need,
      {},
      reserveCall,
    );

    expect(result.status).toBe("committed");
    expect(state.getPlan).toHaveBeenCalledTimes(1);
    expect(state.synthesize).toHaveBeenCalledTimes(1);
    expect(reserveCall).toHaveBeenCalledTimes(1);
    expect(state.commit).toHaveBeenCalledTimes(1);
    expect(state.refresh).toHaveBeenCalledTimes(1);
    expect(
      maintenanceOperationId({ ...need, triggerAuthoritySeq: 5n }),
    ).not.toBe(maintenanceOperationId(need));
    expect(state.commit.mock.calls[0]?.[0]).toMatchObject({
      operationId: maintenanceOperationId(need),
      subjectId: need.subjectId,
      expectedAuthoritySeq: 4n,
      sources: [{ revisionId: revision, expectedEpoch: 1n }],
      narrative: "A supported decision.",
      points: [
        {
          ordinal: 0,
          text: "Continue the plan.",
          basis: [
            {
              basis: {
                case: "cognitionDependency",
                value: {
                  targetRevision: { kind: "episode_revision", value: revision },
                  basisRole: "direct",
                },
              },
            },
          ],
        },
      ],
      producer: {
        modelRole: "journal_synthesis",
        modelProfile: "stub",
        implementation: "semantic-stub",
      },
    });
  });
});
