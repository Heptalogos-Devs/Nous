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
type VersionCheck = {
  status: ResourceRecord["versionStatus"];
  checkedAt: string;
};
type AccessCheck = {
  status: ResourceRecord["accessStatus"];
  checkedAt: string;
};
type ResourceMaterial = {
  reference: StableExternalRef;
  content: string;
  mediaType: string;
  version: VersionCheck;
  access: AccessCheck;
};
export interface ExternalResourceAdapter {
  readonly profileDigest: string;
  describe(
    binding: ResourceBinding,
    signal?: AbortSignal,
  ): Promise<{ resourceIds: string[]; accessible: boolean }>;
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
  ): Promise<VersionCheck>;
  checkAccess(
    reference: StableExternalRef,
    signal?: AbortSignal,
  ): Promise<AccessCheck>;
}
export class ResourceProviderError extends Error {
  constructor(
    readonly status: "unavailable" | "denied" | "stale" | "failed",
    message: string,
    readonly missing = false,
  ) {
    super(message);
  }
}
