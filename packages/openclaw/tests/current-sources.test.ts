// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create, toJson } from "@bufbuild/protobuf";
import { expect, test, vi } from "vitest";
import type { NousClient } from "@nous-wave/client";
import type { HostPrepareRequest } from "@nous-wave/client/host-bridge";
import {
  CognitionUsageSchema,
  ContextSegmentSchema,
  ReadSourcesResponseSchema,
  ReportUseResponseSchema,
  type ContextSegment,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { HostedPermission } from "../src/host.js";
import { prepareHostCurrentSources } from "../src/current-sources.js";
const occurrence = "actual-occurrence";
function permission(currentSources: unknown[]) {
  return {
    identity: {
      subjectId: "subject",
      nousSessionId: "session",
      contactRequestId: "contact",
    },
    response: { currentSources },
    usage: create(CognitionUsageSchema, {
      sourceRead: {
        subjectId: "subject",
        sourceRefs: [{ kind: "occurrence", value: occurrence }],
      },
    }),
  } as unknown as HostedPermission & { identity: HostPrepareRequest };
}
function material(kind: string, value: string, actual = occurrence) {
  return create(ContextSegmentSchema, {
    text: "Existing READY material description",
    sourceRefs: [{ kind, value }],
    evidence: [
      {
        basisRole: "actual_occurrence",
        reference: { kind: "occurrence", value: actual },
      },
    ],
  });
}
function fixture(items: ContextSegment[]) {
  const readSources = vi.fn<NousClient["cognition"]["readSources"]>(async () =>
    create(ReadSourcesResponseSchema, { items }),
  );
  const reportUse = vi.fn<NousClient["runtime"]["reportUse"]>(async () =>
    create(ReportUseResponseSchema),
  );
  return {
    readSources,
    reportUse,
    client: {
      cognition: { readSources },
      runtime: { reportUse },
    } as unknown as Pick<NousClient, "cognition" | "runtime">,
  };
}
test("whole and exact region/derived locators read back their actual occurrence without expanding scope", async () => {
  const locators = [
    { wholeOccurrence: true },
    { sourceRegionId: "source-region" },
    { derivedRepresentationId: "ready-representation" },
    { derivedRegionId: "derived-region" },
  ];
  const p = permission(
    locators.map((locator) => ({
      occurrenceId: occurrence,
      basisRole: "interpretation",
      ...locator,
    })),
  );
  const before = toJson(CognitionUsageSchema, p.usage);
  const f = fixture([
    material("occurrence", occurrence),
    material("source_region", "source-region"),
    material("derived_representation", "ready-representation"),
    material("derived_region", "derived-region"),
  ]);
  const result = await prepareHostCurrentSources(
    f.client,
    p,
    "consumer",
    new AbortController().signal,
  );
  expect(result.sources.map((source) => source.locator.case)).toEqual([
    "wholeOccurrence",
    "sourceRegionId",
    "derivedRepresentationId",
    "derivedRegionId",
  ]);
  expect(f.readSources).toHaveBeenCalledOnce();
  expect(f.readSources.mock.calls[0]?.[0]).toMatchObject({
    usage: p.usage,
    materialSources: result.sources,
  });
  expect(toJson(CognitionUsageSchema, p.usage)).toEqual(before);
  expect(
    f.reportUse.mock.calls[0]?.[0].events?.map(
      (event) => `${event.reference?.kind}:${event.reference?.value}`,
    ),
  ).toEqual([
    `occurrence:${occurrence}`,
    "source_region:source-region",
    "derived_representation:ready-representation",
    "derived_region:derived-region",
  ]);
  expect(result.textBytes).toBeGreaterThan(0);
});
test("a locator needs exact returned material and occurrence, and owner denial is never promoted", async () => {
  const p = permission([
    {
      occurrenceId: occurrence,
      derivedRepresentationId: "ready-representation",
      basisRole: "interpretation",
    },
  ]);
  for (const items of [
    [],
    [material("derived_representation", "other")],
    [
      material(
        "derived_representation",
        "ready-representation",
        "other-occurrence",
      ),
    ],
  ]) {
    const f = fixture(items);
    await expect(
      prepareHostCurrentSources(
        f.client,
        p,
        "consumer",
        new AbortController().signal,
      ),
    ).rejects.toThrow("unavailable or omitted");
    expect(f.reportUse).not.toHaveBeenCalled();
  }
  const f = fixture([]);
  f.readSources.mockRejectedValueOnce(Error("Material lineage mismatch"));
  await expect(
    prepareHostCurrentSources(
      f.client,
      p,
      "consumer",
      new AbortController().signal,
    ),
  ).rejects.toThrow("Material lineage mismatch");
  expect(f.reportUse).not.toHaveBeenCalled();
});
test("empty occurrence/locator or false wholeOccurrence stops before reading", async () => {
  for (const source of [
    { occurrenceId: occurrence, wholeOccurrence: false },
    { occurrenceId: occurrence },
    { derivedRepresentationId: "representation" },
  ]) {
    const f = fixture([]);
    await expect(
      prepareHostCurrentSources(
        f.client,
        permission([source]),
        "consumer",
        new AbortController().signal,
      ),
    ).rejects.toThrow(/exact/);
    expect(f.readSources).not.toHaveBeenCalled();
  }
});
