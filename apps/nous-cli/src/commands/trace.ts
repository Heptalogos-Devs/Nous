// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { CliError } from "../agent.js";
import type { NousClient } from "@nous-wave/client";
import type { CliEnvironment } from "../runtime.js";
import { restoreProtocolData, clientDataSchemas } from "@nous-wave/client/data";
const { CognitiveRefSchema } = clientDataSchemas;

export async function traceCommands(
  env: CliEnvironment,
  action: string | undefined,
) {
  const { client, subjectId, required, resolveReference } = env;
  const refText = required(action, "Reference");
  const reference = await resolveReference(refText);
  const input = { subjectId, id: reference.value };
  let cognition: unknown;
  let bases: Awaited<ReturnType<NousClient["memory"]["revision"]>>["basis"] =
    [];
  let producerSignatureId: string | undefined;
  switch (reference.kind) {
    case "memory_revision": {
      const value = await client.memory.revision(input);
      cognition = value;
      bases = value.basis;
      producerSignatureId = value.producerSignatureId;
      break;
    }
    case "cognitive_schema_revision": {
      const value = await client.concepts.getSchemaRevision(input);
      cognition = value;
      bases = value.evidenceLinks.flatMap((link) =>
        link.basis ? [link.basis] : [],
      );
      producerSignatureId = value.producerSignatureId;
      break;
    }
    case "episode_revision": {
      const value = await client.memory.getEpisodeRevision(input);
      cognition = value;
      bases = value.basis;
      producerSignatureId = value.producerSignatureId;
      break;
    }
    case "journal_revision": {
      const value = await client.memory.getJournalRevision(input);
      cognition = value;
      bases = value.points.flatMap((point) => point.basis);
      producerSignatureId = value.producerSignatureId;
      break;
    }
    default:
      throw new CliError(
        "REFERENCE_TYPE_MISMATCH",
        "Trace requires an exact cognition revision",
      );
  }
  const sources = [];
  const visited = new Set<string>();
  const derivations: unknown[] = [];
  const traceMaterial = async (materialRef: {
    kind: string;
    value: string;
  }): Promise<void> => {
    const key = `${materialRef.kind}:${materialRef.value}`;
    if (visited.has(key)) return;
    if (visited.size >= 512) throw new Error("Trace material budget exceeded");
    visited.add(key);
    if (materialRef.kind === "derived_representation") {
      const representation = await client.material.representation({
        subjectId,
        id: materialRef.value,
      });
      derivations.push({
        reference: restoreProtocolData({ ...materialRef }, CognitiveRefSchema),
        representation,
      });
      for (const ancestor of representation.inputs)
        if (ancestor.reference) await traceMaterial(ancestor.reference);
    } else if (materialRef.kind === "derived_region") {
      const region = await client.material.derivedRegion({
        subjectId,
        id: materialRef.value,
      });
      derivations.push({
        reference: restoreProtocolData({ ...materialRef }, CognitiveRefSchema),
        region,
      });
      await traceMaterial({
        kind: "derived_representation",
        value: region.representationId,
      });
    } else if (materialRef.kind === "source_region") {
      const region = await client.material.sourceRegion({
        subjectId,
        id: materialRef.value,
      });
      const artifact = await client.material.getArtifact({
        subjectId,
        id: region.artifactId,
      });
      derivations.push({
        reference: restoreProtocolData({ ...materialRef }, CognitiveRefSchema),
        region,
        artifact,
      });
    }
  };
  for (const basis of bases) {
    if (basis.basis.case !== "evidence") continue;
    const evidence = basis.basis.value;
    const occurrence = await client.material.occurrence({
      subjectId,
      id: evidence.occurrenceId,
    });
    const artifact = occurrence.artifactId
      ? await client.material.getArtifact({
          subjectId,
          id: occurrence.artifactId,
        })
      : undefined;
    const representation =
      evidence.locator.case === "derivedRepresentationId"
        ? await client.material.representation({
            subjectId,
            id: evidence.locator.value,
          })
        : undefined;
    if (representation)
      await traceMaterial({
        kind: "derived_representation",
        value: representation.representationId,
      });
    if (evidence.locator.case === "derivedRegionId")
      await traceMaterial({
        kind: "derived_region",
        value: evidence.locator.value,
      });
    if (evidence.locator.case === "sourceRegionId")
      await traceMaterial({
        kind: "source_region",
        value: evidence.locator.value,
      });
    sources.push({ evidence, occurrence, artifact, representation });
  }
  const producer = producerSignatureId
    ? await client.material.producer({
        subjectId,
        id: producerSignatureId,
      })
    : undefined;
  return {
    reference: restoreProtocolData({ ...reference }, CognitiveRefSchema),
    cognition,
    producer,
    sources,
    derivations,
  };
}
