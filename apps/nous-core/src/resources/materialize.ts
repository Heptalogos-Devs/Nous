// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, toJson } from "@bufbuild/protobuf";
import { Code, ConnectError } from "@connectrpc/connect";
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
  options = executionOptions(kernel.execution.opportunity, options);
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
    const material = await adapter.materialize(
      reference,
      options.signal ?? undefined,
    );
    if (Buffer.byteLength(material.content, "utf8") > 1048576)
      throw new ConnectError(
        "Resource material exceeds the 1 MiB text bound",
        Code.ResourceExhausted,
      );
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
          value: { text: material.content, mediaType: material.mediaType },
        },
      }),
      options,
    );
    return create(ResourceMaterializationSchema, {
      reference: request.reference,
      observation,
      content: material.content,
      mediaType: material.mediaType,
    });
  } catch (error) {
    if (options.signal?.aborted) throw error;
    if (error instanceof ResourceProviderError)
      throw new ConnectError(
        `Resource material unavailable (${error.status})`,
        Code.FailedPrecondition,
      );
    throw error;
  }
}
