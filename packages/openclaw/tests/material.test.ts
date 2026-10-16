// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create } from "@bufbuild/protobuf";
import {
  AcceptedObservationSchema,
  ObservationInputSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { expect, test, vi } from "vitest";
import type { NousClient } from "@nous-wave/client";
import type { HostedPermission } from "../src/host.js";
import { prepareCurrentMaterials } from "../src/material.js";
const attachment = "attachment:00000000-0000-4000-8000-000000000001";
const contact = {
  contactRef: attachment,
  observation: create(ObservationInputSchema, {
    subjectId: "subject",
    sourceClass: "message",
    externalObjectRef: "object:heptalogos:" + attachment,
    material: { case: "artifactId", value: "actual-artifact" },
    context: {
      host_selected_media: true,
      message_ref: "message:00000000-0000-4000-8000-000000000002",
      attachment_ref: attachment,
      part_index: 0,
      original_media_type: "image/gif",
    },
  }),
};
const receipt = create(AcceptedObservationSchema, {
  artifactId: "actual-artifact",
  occurrenceId: "actual-occurrence",
  sourceRegionId: "actual-region",
});
const derive = vi.fn(
  async (_input: {
    subjectId: string;
    sourceRegionId: string;
    strategy: string;
  }) => ({
    selectedRepresentationId: "saved-representation",
    representations: [
      {
        representationId: "saved-representation",
        text: "Actual sampled frames description",
        quality: { frame_coverage: "sampled" },
      },
    ],
    degradation: [],
  }),
);
const client = {
  material: { getArtifact: async () => ({ mediaType: "image/gif" }) },
  model: { deriveMaterial: derive },
} as unknown as Pick<NousClient, "material" | "model">;
const permission = (contacts: (typeof contact)[]) =>
  ({ contacts }) as HostedPermission;
test("only actual Host-selected contacts derive; receipts and bounds remain exact", async () => {
  derive.mockClear();
  const selected = await prepareCurrentMaterials(
    client,
    permission([contact, contact]),
    [{ contactRef: attachment, receipt }],
    { maxItems: 1, maxTextBytes: 4096 },
    new AbortController().signal,
  );
  expect(derive).toHaveBeenCalledTimes(1);
  expect(derive.mock.calls[0]?.[0]).toEqual({
    subjectId: "subject",
    sourceRegionId: "actual-region",
    strategy: "description_only",
  });
  expect(selected.sources).toEqual([
    {
      occurrenceId: "actual-occurrence",
      locator: {
        case: "derivedRepresentationId",
        value: "saved-representation",
      },
      basisRole: "interpretation",
    },
  ]);
  expect(selected.omissions[0]?.reason).toBe("current_media_item_budget");
  const ordinary = structuredClone(contact);
  delete ordinary.observation.context!.host_selected_media;
  expect(
    (
      await prepareCurrentMaterials(
        client,
        permission([ordinary]),
        [],
        { maxItems: 1, maxTextBytes: 4096 },
        new AbortController().signal,
      )
    ).sources,
  ).toEqual([]);
  await expect(
    prepareCurrentMaterials(
      client,
      permission([contact]),
      [],
      { maxItems: 1, maxTextBytes: 4096 },
      new AbortController().signal,
    ),
  ).rejects.toThrow(/exact accepted/);
});
