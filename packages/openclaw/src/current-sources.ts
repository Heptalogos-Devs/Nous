// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { fromJson, type MessageInitShape } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import type { NousClient } from "@nous-wave/client";
import type { HostPrepareRequest } from "@nous-wave/client/host-bridge";
import { eventIdentity } from "@nous-wave/client/managed";
import {
  EvidenceRefSchema,
  UseEventSchema,
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

/** Locator selections never expand SourceRead; the Material owner checks scope and exact lineage. */
export async function prepareHostCurrentSources(
  client: Pick<NousClient, "cognition" | "runtime">,
  permission: HostedPermission & { identity: HostPrepareRequest },
  consumerId: string,
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
  const events = new Map<string, MessageInitShape<typeof UseEventSchema>>();
  for (const [index, source] of sources.entries())
    for (const reference of [
      { kind: "occurrence", value: source.occurrenceId },
      references[index]!,
    ]) {
      const key = `${source.occurrenceId}:${reference.kind}:${reference.value}`;
      if (events.has(key)) continue;
      events.set(key, {
        eventId: await eventIdentity(
          `${permission.identity.contactRequestId}:completion:${source.occurrenceId}${
            reference.kind === "occurrence"
              ? ""
              : `:${reference.kind}:${reference.value}`
          }`,
        ),
        reference,
        kind: "presented",
        occurredAt: timestampFromDate(new Date()),
        context: { native_completion: true },
      });
    }
  await client.runtime.reportUse(
    {
      subjectId: permission.identity.subjectId,
      sessionId: permission.identity.nousSessionId,
      consumerRef: consumerId,
      events: [...events.values()],
    },
    { signal },
  );
  return {
    sources,
    textBytes: checked.items.reduce(
      (sum, item) => sum + Buffer.byteLength(item.text),
      0,
    ),
  };
}
