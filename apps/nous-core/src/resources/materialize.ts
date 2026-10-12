// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { Code, ConnectError } from "@connectrpc/connect";
import {
  startOperation,
  serializeOperation,
  workflowPayload,
} from "../durable-operation.js";
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
  const operation = await startOperation(
    kernel,
    identity,
    {
      content: workflowPayload({ reference }, [
        { kind: "resource", value: reference.resourceRef },
      ]),
    },
    options,
  );
  const record = operation.record;
  if (record.outcome) {
    if (record.outcome.purged)
      throw new ConnectError("Resource outcome was purged", Code.NotFound);
    return fromJson(
      ResourceMaterializationSchema,
      JSON.parse(record.outcome.payloadJson) as JsonValue,
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
      let proposal = record.proposal?.payloadJson
        ? (JSON.parse(record.proposal.payloadJson) as {
            content: string;
            mediaType: string;
          })
        : undefined;
      let version;
      let access;
      if (!proposal) {
        const material = await adapter.materialize(
          reference,
          options.signal ?? undefined,
        );
        version = material.versionStatus;
        access = material.accessStatus;
        proposal = { content: material.content, mediaType: material.mediaType };
      } else {
        [version, access] = await Promise.all([
          adapter.checkVersion(reference, options.signal ?? undefined),
          adapter.checkAccess(reference, options.signal ?? undefined),
        ]);
      }
      if (version !== "current" || access !== "allowed")
        throw new ConnectError(
          "Resource version or access no longer permits admission",
          Code.FailedPrecondition,
        );
      if (Buffer.byteLength(proposal.content, "utf8") > 1048576)
        throw new ConnectError(
          "Selected Resource material exceeds 1 MiB UTF-8",
          Code.ResourceExhausted,
        );
      if (!record.proposal)
        await operation.saveProposal(proposal, [], undefined, [
          request.operationId,
        ]);
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
