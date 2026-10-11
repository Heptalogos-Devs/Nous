// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { Code, ConnectError } from "@connectrpc/connect";
import { reserveOperation, workflowPayload } from "../durable-operation.js";
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
  const calls = executionOptions(kernel.execution.opportunity, options);
  options = calls;
  if (
    !request.reference ||
    !request.observedAt ||
    !z.string().uuid().safeParse(request.operationId).success
  )
    throw new ConnectError(
      "Selected resource materialization requires reference, stable operation ID and observed time",
      Code.InvalidArgument,
    );
  const exactReference = request.reference;
  const reference = {
    ...request.reference,
    entryVersion: request.reference.entryVersion ?? null,
  };
  const identity = {
    subjectId: request.subjectId,
    owner: "material",
    operationKey: `resource:${request.operationId}`,
    semanticDigest: canonicalDigest({
      subject: request.subjectId,
      reference: toJson(StableExternalRefSchema, exactReference),
      observedAt: {
        seconds: request.observedAt.seconds.toString(),
        nanos: request.observedAt.nanos,
      },
    }),
  };
  const operation = await reserveOperation(
    kernel,
    identity,
    {
      content: workflowPayload({ reference }, [
        { kind: "resource", value: reference.resourceRef },
      ]),
    },
    options,
  );
  const reservation = operation.record;
  if (reservation.outcome) {
    if (reservation.outcome.purged)
      throw new ConnectError("Resource outcome was purged", Code.NotFound);
    return fromJson(
      ResourceMaterializationSchema,
      JSON.parse(reservation.outcome.payloadJson) as JsonValue,
    );
  }
  return operation.run(async () => {
    try {
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
      let proposal = reservation.proposal?.payloadJson
        ? z
            .strictObject({
              content: z.string().max(1048576),
              mediaType: z.string().max(128),
            })
            .parse(JSON.parse(reservation.proposal?.payloadJson))
        : undefined;
      if (!proposal) {
        const material = await adapter.materialize(
          reference,
          options.signal ?? undefined,
        );
        proposal = { content: material.content, mediaType: material.mediaType };
        await operation.saveProposal(proposal);
      }
      const [version, access] = await Promise.all([
        adapter.checkVersion(reference, options.signal ?? undefined),
        adapter.checkAccess(reference, options.signal ?? undefined),
      ]);
      if (version.status !== "current" || access.status !== "allowed")
        throw new ConnectError(
          "Resource version or access no longer permits admission",
          Code.FailedPrecondition,
        );
      const observation = await kernel.runtime.recordObservation(
        create(ObservationInputSchema, {
          subjectId: request.subjectId,
          requestId: request.operationId,
          sourceClass: "resource",
          observedAt: request.observedAt,
          externalObjectRef: `object:resource-entry:${canonicalDigest(toJson(StableExternalRefSchema, exactReference))}`,
          context: {
            external_resource: toJson(StableExternalRefSchema, exactReference),
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
      await operation.complete(toJson(ResourceMaterializationSchema, result), [
        { kind: "occurrence", value: observation.occurrenceId },
        ...(observation.sourceRegionId
          ? [{ kind: "source_region", value: observation.sourceRegionId }]
          : []),
        ...(observation.artifactId
          ? [{ kind: "artifact", value: observation.artifactId }]
          : []),
      ]);
      return result;
    } catch (error) {
      if (options.signal?.aborted) throw error;
      if (error instanceof ResourceProviderError)
        throw new ConnectError(
          `Resource material unavailable (${error.status})`,
          Code.FailedPrecondition,
        );
      throw error;
    }
  });
}
