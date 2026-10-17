// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { fromJson } from "@bufbuild/protobuf";
import type { NousClient } from "@nous-wave/client";
import type { HostPrepareRequest } from "@nous-wave/client/host-bridge";
import {
  EvidenceRefSchema,
  type EvidenceRef,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { HostedPermission } from "./host.js";

function selectedReference(source: EvidenceRef) {
  if (!source.occurrenceId)
    throw Error("Host current sources require an exact actual Occurrence");
  const locator = source.locator;
  switch (locator.case) {
    case "wholeOccurrence":
      if (locator.value)
        return { kind: "occurrence", value: source.occurrenceId };
      break;
    case "sourceRegionId":
      if (locator.value) return { kind: "source_region", value: locator.value };
      break;
    case "derivedRepresentationId":
      if (locator.value)
        return { kind: "derived_representation", value: locator.value };
      break;
    case "derivedRegionId":
      if (locator.value)
        return { kind: "derived_region", value: locator.value };
      break;
  }
  throw Error("Host current sources require an exact Material locator");
}

/** Validate current locators without recording use; the final applied projection owns admission. */
export async function prepareHostCurrentSources(
  client: Pick<NousClient, "cognition">,
  permission: HostedPermission & { identity: HostPrepareRequest },
  signal: AbortSignal,
) {
  const sources = permission.response.currentSources.map((raw) =>
    fromJson(EvidenceRefSchema, raw),
  );
  const references = sources.map(selectedReference);
  const checked = await client.cognition.readSources(
    {
      subjectId: permission.identity.subjectId,
      usage: permission.usage,
      materialSources: sources,
      maxTextBytes: 1048576,
    },
    { signal },
  );
  if (
    checked.degradation.length ||
    sources.some(
      (source, index) =>
        !checked.items.some(
          (item) =>
            item.sourceRefs.some(
              (ref) =>
                ref.kind === references[index]!.kind &&
                ref.value === references[index]!.value,
            ) &&
            item.evidence.some(
              (evidence) =>
                evidence.basisRole === "actual_occurrence" &&
                evidence.reference?.kind === "occurrence" &&
                evidence.reference.value === source.occurrenceId,
            ),
        ),
    )
  )
    throw Error("Actual Host current sources are unavailable or omitted");
  return {
    sources,
    textBytes: checked.items.reduce(
      (sum, item) => sum + Buffer.byteLength(item.text),
      0,
    ),
  };
}
