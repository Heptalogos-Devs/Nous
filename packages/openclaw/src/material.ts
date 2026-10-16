// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { z } from "zod";
import type { NousClient } from "@nous-wave/client";
import { HOST_SELECTED_MEDIA_CONTEXT_KEY } from "@nous-wave/client/host-bridge";
import type { AcceptedObservation } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { MessageInitShape } from "@bufbuild/protobuf";
import { EvidenceRefSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { HostedPermission } from "./host.js";
const uuid =
  "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}";
const metadata = z.object({
  message_ref: z.string().regex(new RegExp(`^message:${uuid}$`)),
  attachment_ref: z.string().regex(new RegExp(`^attachment:${uuid}$`)),
  part_index: z.number().int().min(0).max(4095),
  original_media_type: z.string().regex(/^[a-zA-Z0-9.+-]+\/[a-zA-Z0-9.+-]+$/),
});
export const materialInputConfiguration = z
  .strictObject({
    maxItems: z.number().int().min(1).max(128).default(8),
    maxTextBytes: z.number().int().min(1024).max(1048576).default(65536),
  })
  .default({ maxItems: 8, maxTextBytes: 65536 });
export interface CurrentMaterialPreparation {
  sources: MessageInitShape<typeof EvidenceRefSchema>[];
  textBytes: number;
  omissions: { contactRef: string; reason: string }[];
}
/** The caller passes only the authenticated Host contacts and their real accepted receipts. */
export async function prepareCurrentMaterials(
  client: Pick<NousClient, "material" | "model">,
  permission: HostedPermission,
  receipts: readonly { contactRef: string; receipt: AcceptedObservation }[],
  policy: z.infer<typeof materialInputConfiguration>,
  signal: AbortSignal,
): Promise<CurrentMaterialPreparation> {
  const result: CurrentMaterialPreparation = {
    sources: [],
    textBytes: 0,
    omissions: [],
  };
  const selected = permission.contacts.filter(
    (contact) =>
      contact.observation.context?.[HOST_SELECTED_MEDIA_CONTEXT_KEY] === true,
  );
  const accepted = new Map(
    receipts.map((item) => [item.contactRef, item.receipt]),
  );
  for (const [index, contact] of selected.entries()) {
    if (index >= policy.maxItems) {
      result.omissions.push({
        contactRef: contact.contactRef,
        reason: "current_media_item_budget",
      });
      continue;
    }
    const observation = contact.observation;
    const fields = metadata.parse(observation.context);
    if (
      observation.sourceClass !== "message" ||
      observation.material.case !== "artifactId" ||
      contact.contactRef !== fields.attachment_ref ||
      observation.externalObjectRef !==
        "object:heptalogos:" + fields.attachment_ref
    )
      throw new Error(
        "Host selected media is not bound to its actual attachment contact",
      );
    const receipt = accepted.get(contact.contactRef);
    if (
      !receipt?.sourceRegionId ||
      !receipt.occurrenceId ||
      receipt.artifactId !== observation.material.value
    )
      throw new Error(
        "Host selected media has no exact accepted artifact/region receipt",
      );
    const artifact = await client.material.getArtifact(
      { subjectId: observation.subjectId, id: receipt.artifactId },
      { signal },
    );
    const mime = artifact.mediaType.split(";")[0]!.trim().toLowerCase();
    if (mime !== fields.original_media_type.toLowerCase())
      throw new Error(
        "Selected media type differs from the actual registered artifact",
      );
    if (
      !["image/", "audio/", "video/"].some((prefix) => mime.startsWith(prefix))
    ) {
      result.omissions.push({
        contactRef: contact.contactRef,
        reason: "current_media_type_unsupported",
      });
      continue;
    }
    const derived = await client.model.deriveMaterial(
      {
        subjectId: observation.subjectId,
        sourceRegionId: receipt.sourceRegionId,
        strategy: "description_only",
      },
      { signal },
    );
    const representation = derived.representations.find(
      (item) => item.representationId === derived.selectedRepresentationId,
    );
    if (!representation?.text) {
      result.omissions.push({
        contactRef: contact.contactRef,
        reason:
          derived.degradation.map((d) => d.code).join(",") ||
          "current_media_description_unavailable",
      });
      continue;
    }
    const size =
      new TextEncoder().encode(representation.text).length +
      new TextEncoder().encode(JSON.stringify(representation.quality ?? {}))
        .length +
      512;
    if (result.textBytes + size > policy.maxTextBytes) {
      result.omissions.push({
        contactRef: contact.contactRef,
        reason: "current_media_text_budget",
      });
      continue;
    }
    result.sources.push({
      occurrenceId: receipt.occurrenceId,
      locator: {
        case: "derivedRepresentationId",
        value: representation.representationId,
      },
      basisRole: "interpretation",
    });
    result.textBytes += size;
  }
  return result;
}
