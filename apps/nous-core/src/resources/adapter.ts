// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { MessageInitShape } from "@bufbuild/protobuf";
import type {
  StableExternalRefSchema,
  ExternalResourceRecordSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { ResourceDescriptor } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";

export type ResourceBinding = Pick<
  ResourceDescriptor,
  "resourceRef" | "adapterKind" | "providerProfile" | "providerLocator"
>;
export type StableExternalRef = MessageInitShape<
  typeof StableExternalRefSchema
>;
export type ResourceRecord = MessageInitShape<
  typeof ExternalResourceRecordSchema
> & { reference: StableExternalRef };
type ResourceMaterial = Pick<
  ResourceRecord,
  "content" | "versionStatus" | "accessStatus"
> & { content: string; mediaType: string };
export interface ExternalResourceAdapter {
  readonly profileDigest: string;
  describe(
    binding: ResourceBinding,
    signal?: AbortSignal,
  ): Promise<{ accessible: boolean }>;
  search(
    binding: ResourceBinding,
    question: string,
    limit: number,
    signal?: AbortSignal,
  ): Promise<ResourceRecord[]>;
  materialize(
    reference: StableExternalRef,
    signal?: AbortSignal,
  ): Promise<ResourceMaterial>;
  checkVersion(
    reference: StableExternalRef,
    signal?: AbortSignal,
  ): Promise<ResourceRecord["versionStatus"]>;
  checkAccess(
    reference: StableExternalRef,
    signal?: AbortSignal,
  ): Promise<ResourceRecord["accessStatus"]>;
}
export class ResourceProviderError extends Error {
  constructor(
    readonly status: "unavailable" | "denied" | "stale" | "failed",
    message: string,
  ) {
    super(message);
  }
}
