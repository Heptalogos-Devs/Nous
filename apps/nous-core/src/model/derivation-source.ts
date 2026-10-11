// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { DeriveMaterialRequest } from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
import type { ExecutionOptions } from "../execution.js";
import type { KernelClient } from "../kernel-client.js";

export class DerivationSourceTooLarge extends Error {
  constructor(
    readonly degradationCode:
      "media_source_too_large" | "derivation_source_too_large",
  ) {
    super("Select a bounded source region");
  }
}

export function lazyDerivationSource(
  kernel: KernelClient,
  request: DeriveMaterialRequest,
  region: Awaited<ReturnType<KernelClient["material"]["getSourceRegion"]>>,
  artifact: Awaited<ReturnType<KernelClient["material"]["getArtifact"]>>,
  maxMediaBytes: number,
  maxEvidenceBytes: number,
  options: ExecutionOptions,
) {
  const isMedia = /^(audio|video)\//i.test(artifact.mediaType);
  const loadSource = async () => {
    if (isMedia && region.coordinateKind === "whole_artifact") {
      const chunks: Uint8Array[] = [];
      let length = 0;
      for await (const chunk of kernel.artifacts.downloadArtifact(
        { subjectId: request.subjectId, artifactId: region.artifactId },
        options,
      )) {
        length += chunk.content.length;
        if (length > maxMediaBytes)
          throw new DerivationSourceTooLarge("media_source_too_large");
        chunks.push(chunk.content);
      }
      if (length !== Number(artifact.byteLength))
        throw new Error(
          "Artifact stream length changed from its frozen metadata",
        );
      return {
        content: Buffer.concat(chunks),
        mediaType: artifact.mediaType,
        partial: false,
      };
    }
    const loaded = await kernel.material.materializeEvidence(
      {
        subjectId: request.subjectId,
        reference: { kind: "source_region", value: request.sourceRegionId },
        maxBytes: BigInt(maxEvidenceBytes),
      },
      options,
    );
    if (loaded.partial)
      throw new DerivationSourceTooLarge("derivation_source_too_large");
    return loaded;
  };
  let sourcePromise: ReturnType<typeof loadSource> | undefined;
  return () => (sourcePromise ??= loadSource());
}
