// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import {
  episodePartitionSchema,
  journalSynthesisSchema,
  partitionIndices,
  journalBasisKeys,
} from "../../../src/model/schemas/longitudinal.js";

describe("longitudinal proposal catalog contracts", () => {
  it("requires a complete ordered partition and rejects invented or repeated members", () => {
    const proposal = episodePartitionSchema.parse({
      action: "partition",
      segments: [
        {
          memberKeys: ["a", "b"],
          title: null,
          boundaryExplanation: "Shared context",
        },
        {
          memberKeys: ["c"],
          title: "New purpose",
          boundaryExplanation: "Purpose changed",
        },
      ],
    });
    expect(
      partitionIndices(proposal, ["a", "b", "c"])?.map(
        (segment) => segment.memberIndices,
      ),
    ).toEqual([[0, 1], [2]]);
    for (const keys of [
      ["a", "c", "b"],
      ["a", "b"],
      ["a", "b", "other"],
      ["a", "b", "a"],
    ])
      expect(() => partitionIndices(proposal, keys)).toThrow();
  });
  it("requires supported Journal points and resolves only distinct supplied support keys", () => {
    const proposal = journalSynthesisSchema.parse({
      action: "commit",
      title: null,
      narrative: "Supported decision",
      points: [
        {
          role: "decision",
          text: "Continue the current plan",
          basisKeys: ["episode-a", "observation-b"],
        },
      ],
    });
    expect(() =>
      journalBasisKeys(proposal, new Set(["episode-a", "observation-b"])),
    ).not.toThrow();
    expect(() => journalBasisKeys(proposal, new Set(["episode-a"]))).toThrow();
    if (proposal.action !== "commit")
      throw new Error("Expected commit proposal");
    proposal.points[0]!.basisKeys = ["episode-a", "episode-a"];
    expect(() => journalBasisKeys(proposal, new Set(["episode-a"]))).toThrow();
    expect(
      journalSynthesisSchema.safeParse({
        ...proposal,
        points: [{ ...proposal.points[0], basisKeys: [] }],
      }).success,
    ).toBe(false);
  });
});
