// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { ExternalResourceResultSchema } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js";
import {
  ExternalResourceRecordSchema,
  StableExternalRefSchema,
  type ResourceAction,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { ResourceProviderError } from "./adapter.js";
import type { ResourceRegistry } from "./registry.js";

export async function executeResourceActions(
  registry: ResourceRegistry,
  actions: ResourceAction[],
  signal?: AbortSignal,
) {
  const results = [];
  for (const action of actions) {
    if (signal?.aborted) throw signal.reason;
    const binding = {
      resourceRef: action.resourceRef,
      adapterKind: action.adapterKind,
      providerProfile: action.providerProfile,
      providerLocator: action.providerLocator,
    };
    const adapter = registry.resolve(binding);
    if (!adapter) {
      results.push(
        create(ExternalResourceResultSchema, {
          actionId: action.actionId,
          resourceRef: action.resourceRef,
          status: "unavailable",
        }),
      );
      continue;
    }
    try {
      if (action.action === "inspect_synopsis") {
        const descriptor = await adapter.describe(binding, signal);
        results.push(
          create(ExternalResourceResultSchema, {
            actionId: action.actionId,
            resourceRef: action.resourceRef,
            status: descriptor.accessible ? "success" : "denied",
          }),
        );
        continue;
      }
      const records = await adapter.search(
        binding,
        action.queryText,
        action.limit,
        signal,
      );
      if (action.currentAuthority || action.materialize) {
        for (const record of records) {
          const [version, access] = await Promise.all([
            adapter.checkVersion(record.reference, signal),
            adapter.checkAccess(record.reference, signal),
          ]);
          record.versionStatus = version.status;
          record.accessStatus = access.status;
        }
      }
      results.push(
        create(ExternalResourceResultSchema, {
          actionId: action.actionId,
          resourceRef: action.resourceRef,
          status: "success",
          records: records.map((record) =>
            create(ExternalResourceRecordSchema, {
              ...record,
              reference: create(StableExternalRefSchema, {
                ...record.reference,
                entryVersion: record.reference.entryVersion ?? undefined,
              }),
            }),
          ),
        }),
      );
    } catch (error) {
      if (signal?.aborted) throw error;
      results.push(
        create(ExternalResourceResultSchema, {
          actionId: action.actionId,
          resourceRef: action.resourceRef,
          status:
            error instanceof ResourceProviderError ? error.status : "failed",
        }),
      );
    }
  }
  return results;
}
