// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { Code, ConnectError } from "@connectrpc/connect";
import {
  runRecordedOperation,
  serializeOperation,
} from "../operation-results.js";
import { executionOptions, type ExecutionOptions } from "../execution.js";
import { z } from "zod";
import {
  type MaterializeResourceRequest,
  ResourceMaterializationSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import {
  ObservationInputSchema,
  StableExternalRefSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { KernelClient } from "../kernel-client.js";
import { canonicalDigest } from "../digest.js";
import type { ResourceRegistry } from "./registry.js";
import { ResourceProviderError } from "./adapter.js";

export async function materializeResource(
  kernel: KernelClient,
  registry: ResourceRegistry,
  request: MaterializeResourceRequest,
  options: ExecutionOptions,
) {
  request.subjectId = request.subjectId.toLowerCase();
  request.operationId = request.operationId.toLowerCase();
  const calls = executionOptions(kernel.execution.opportunity, options);
  return serializeOperation(
    kernel,
    `material:${request.subjectId}:resource:${request.operationId}`,
    calls.signal,
    () => materializeResourceOnce(kernel, registry, request, calls),
  );
}

async function materializeResourceOnce(
  kernel: KernelClient,
  registry: ResourceRegistry,
  request: MaterializeResourceRequest,
  options: ExecutionOptions,
) {
  if (
    !request.reference ||
    !request.observedAt ||
    !z.string().uuid().safeParse(request.operationId).success
  )
    throw new ConnectError(
      "Selected resource materialization requires reference, stable operation ID and observed time",
      Code.InvalidArgument,
    );
  const reference = request.reference;
  const referenceJson = toJson(StableExternalRefSchema, reference);
  const referenceDigest = canonicalDigest(referenceJson);
  const identity = {
    subjectId: request.subjectId,
    owner: "material",
    operationKey: `resource:${request.operationId}`,
    semanticDigest: canonicalDigest({
      subject: request.subjectId,
      reference: referenceJson,
      observedAt: {
        seconds: request.observedAt.seconds.toString(),
        nanos: request.observedAt.nanos,
      },
    }),
  };
  const adapterForReference = async () => {
    const descriptor = await kernel.resourceRegistry.getResource(
      { subjectId: request.subjectId, id: reference.resourceRef },
      options,
    );
    if (
      descriptor.adapterKind !== reference.providerKind ||
      descriptor.providerProfile !== reference.providerProfile ||
      descriptor.providerLocator !== reference.accessScope
    )
      throw new ConnectError(
        "Resource binding has changed",
        Code.FailedPrecondition,
      );
    const adapter = registry.resolve(descriptor);
    if (!adapter)
      throw new ConnectError(
        "Resource adapter is not configured",
        Code.FailedPrecondition,
      );
    if (adapter.profileDigest !== reference.profileDigest)
      throw new ConnectError(
        "Resource provider identity has changed",
        Code.FailedPrecondition,
      );
    return adapter;
  };
  return runRecordedOperation<
    { content: string; mediaType: string },
    import("@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js").ResourceMaterialization
  >(kernel, identity, options, {
    replay: (outcome) => {
      if (outcome.purged)
        throw new ConnectError("Resource outcome was purged", Code.NotFound);
      return fromJson(
        ResourceMaterializationSchema,
        JSON.parse(outcome.payloadJson) as JsonValue,
      );
    },
    generate: async () => {
      const material = await (
        await adapterForReference()
      ).materialize(reference, options.signal ?? undefined);
      if (
        material.versionStatus !== "current" ||
        material.accessStatus !== "allowed"
      )
        throw new ConnectError(
          "Resource version or access no longer permits admission",
          Code.FailedPrecondition,
        );
      if (Buffer.byteLength(material.content, "utf8") > 1048576)
        throw new ConnectError(
          "Selected Resource material exceeds 1 MiB UTF-8",
          Code.ResourceExhausted,
        );
      return {
        proposal: { content: material.content, mediaType: material.mediaType },
        dependencies: [{ kind: "resource", value: reference.resourceRef }],
        mutationOperations: [request.operationId],
      };
    },
    commit: async (proposal, _context, reused) => {
      if (reused) {
        const adapter = await adapterForReference();
        const [version, access] = await Promise.all([
          adapter.checkVersion(reference, options.signal ?? undefined),
          adapter.checkAccess(reference, options.signal ?? undefined),
        ]);
        if (version !== "current" || access !== "allowed")
          throw new ConnectError(
            "Resource version or access no longer permits admission",
            Code.FailedPrecondition,
          );
      }
      const observation = await kernel.runtime.recordObservation(
        create(ObservationInputSchema, {
          subjectId: request.subjectId,
          requestId: request.operationId,
          sourceClass: "resource",
          observedAt: request.observedAt,
          externalObjectRef: `object:resource-entry:${referenceDigest}`,
          context: {
            external_resource: referenceJson,
          },
          material: {
            case: "inlineText",
            value: { text: proposal.content, mediaType: proposal.mediaType },
          },
        }),
        options,
      );
      const result = create(ResourceMaterializationSchema, {
        reference: request.reference,
        observation,
        content: proposal.content,
        mediaType: proposal.mediaType,
      });
      return {
        value: result,
        outcome: toJson(ResourceMaterializationSchema, result),
        dependencies: [
          { kind: "occurrence", value: observation.occurrenceId },
          ...(observation.sourceRegionId
            ? [{ kind: "source_region", value: observation.sourceRegionId }]
            : []),
          ...(observation.artifactId
            ? [{ kind: "artifact", value: observation.artifactId }]
            : []),
        ],
      };
    },
    reject: async (error) => {
      if (options.signal?.aborted) throw error;
      if (error instanceof ResourceProviderError)
        throw new ConnectError(
          `Resource material unavailable (${error.status})`,
          Code.FailedPrecondition,
        );
      return undefined;
    },
  });
}
