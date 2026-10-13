// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { resultPayload } from "../src/operation-results.js";
import type { ResultPayload } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_results_pb.js";
import { coreExecutionSchema } from "../src/configuration/catalog.js";
import { create } from "@bufbuild/protobuf";
import { MaterializeResourceRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { expect, test } from "vitest";
import type { KernelClient } from "../src/kernel-client.js";
import { ResourceRegistry } from "../src/resources/registry.js";
import { materializeResource } from "../src/resources/materialize.js";

test("a saved external proposal is revalidated before admission while an accepted snapshot replays independently", async () => {
  let revoked = false;
  let changed = false;
  let admissions = 0;
  const binding = {
    adapterKind: "local-documents",
    providerProfile: "test",
    providerLocator: "scope",
  };
  const registry = new ResourceRegistry([
    {
      ...binding,
      adapter: {
        profileDigest: "profile",
        describe: async () => ({ accessible: true }),
        search: async () => [],
        materialize: async () => {
          throw new Error("A saved proposal must not fetch its contents again");
        },
        checkVersion: async () => (changed ? "stale" : "current"),
        checkAccess: async () => (revoked ? "denied" : "allowed"),
      },
    },
  ]);
  let outcome: ResultPayload | undefined;
  const kernel = {
    modelResults: {
      readOperation: async () => ({
        proposal: resultPayload({
          content: "saved material",
          mediaType: "text/plain",
        }),
        outcome,
      }),
      saveOperation: async (value: { outcome?: ResultPayload }) => {
        outcome = value.outcome;
      },
    },
    resourceRegistry: { getResource: async () => binding },
    runtime: {
      recordObservation: async () => {
        admissions++;
        return {};
      },
    },
    execution: coreExecutionSchema.parse(undefined),
  } as unknown as KernelClient;
  const request = create(MaterializeResourceRequestSchema, {
    subjectId: crypto.randomUUID(),
    operationId: crypto.randomUUID(),
    observedAt: { seconds: 1n },
    reference: {
      providerKind: binding.adapterKind,
      providerProfile: binding.providerProfile,
      accessScope: binding.providerLocator,
      profileDigest: "profile",
      resourceRef: "resource:test",
    },
  });
  changed = true;
  await expect(
    materializeResource(kernel, registry, request, {}),
  ).rejects.toMatchObject({ code: 9 });
  changed = false;
  revoked = true;
  await expect(
    materializeResource(kernel, registry, request, {}),
  ).rejects.toMatchObject({ code: 9 });
  expect(admissions).toBe(0);
  revoked = false;
  const accepted = await materializeResource(kernel, registry, request, {});
  expect(admissions).toBe(1);
  revoked = true;
  changed = true;
  expect(await materializeResource(kernel, registry, request, {})).toEqual(
    accepted,
  );
  expect(admissions).toBe(1);
});
