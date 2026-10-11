// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { ProducerSignatureSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { type ModelProducerMetadata } from "./execution/snapshot.js";

/** The actual successful execution supplies the producer fields for every model owner. */
export function modelProducer(
  metadata: ModelProducerMetadata,
  operation: string,
) {
  return create(ProducerSignatureSchema, {
    providerClass: metadata.protocol,
    operation,
    implementation: metadata.implementation,
    modelIdentity: metadata.model,
    modelRevision: metadata.modelRevision,
    modelRole: metadata.modelRole,
    modelProfile: metadata.modelProfile,
    executionProfile: metadata.executionProfile,
    inferenceControlsDigest: metadata.inferenceControlsDigest,
    rolePolicyDigest: metadata.rolePolicyDigest,
    promptId: metadata.promptId,
    promptDigest: metadata.promptDigest,
    outputSchemaDigest: metadata.outputSchemaDigest,
    preprocessingIdentity: metadata.promptId,
    preprocessingRevision: metadata.promptDigest,
    configDigest: metadata.configDigest,
  });
}
