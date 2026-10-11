// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import type { CliLeaf } from "./command-types.js";

import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
import { create } from "@bufbuild/protobuf";
import {
  CognitiveRefSchema,
  type RevisionBasis,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { isCognitionRevisionReference, typedCognition } from "./cognition.js";

async function traceCommands(env: CliEnvironment, argument?: string) {
  const { client, subjectId, required, resolveReference } = env;
  const refText = required(argument, "Reference");
  const reference = await resolveReference(refText);
  if (!isCognitionRevisionReference(reference))
    throw new CliError(
      "REFERENCE_TYPE_MISMATCH",
      "Trace requires an exact cognition revision",
    );
  let cognition: unknown;
  let bases: RevisionBasis[] = [];
  let producerSignatureId: string | undefined;
  switch (reference.kind) {
    case "memory_revision": {
      const value = await typedCognition.read(env, {
        kind: "memory_revision",
        value: reference.value,
      });
      cognition = value;
      bases = value.revision?.details?.basis ?? [];
      producerSignatureId = value.revision?.header?.producerSignatureId;
      break;
    }
    case "cognitive_schema_revision": {
      const value = await typedCognition.read(env, {
        kind: "cognitive_schema_revision",
        value: reference.value,
      });
      cognition = value;
      bases =
        value.revision?.details?.evidenceLinks.flatMap((link) =>
          link.basis ? [link.basis] : [],
        ) ?? [];
      producerSignatureId = value.revision?.header?.producerSignatureId;
      break;
    }
    case "episode_revision": {
      const value = await typedCognition.read(env, {
        kind: "episode_revision",
        value: reference.value,
      });
      cognition = value;
      bases = value.revision?.basis ?? [];
      producerSignatureId = value.revision?.header?.producerSignatureId;
      break;
    }
    case "journal_revision": {
      const value = await typedCognition.read(env, {
        kind: "journal_revision",
        value: reference.value,
      });
      cognition = value;
      bases = value.revision?.points.flatMap((point) => point.basis) ?? [];
      producerSignatureId = value.revision?.header?.producerSignatureId;
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
        reference: create(CognitiveRefSchema, materialRef),
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
        reference: create(CognitiveRefSchema, materialRef),
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
        reference: create(CognitiveRefSchema, materialRef),
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
    reference: create(CognitiveRefSchema, reference),
    cognition,
    producer,
    sources,
    derivations,
  };
}

export function createTraceCommand(leaf: CliLeaf) {
  return leaf({}, traceCommands, {
    parameters: ["<canonical or lexical reference>"],
  });
}
