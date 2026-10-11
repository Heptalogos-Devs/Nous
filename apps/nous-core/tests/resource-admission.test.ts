// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { expect, test } from "vitest";
import { coreExecutionSchema } from "../src/configuration/catalog.js";
import type { KernelClient } from "../src/kernel-client.js";
import { ResourceRegistry } from "../src/resources/registry.js";
import { materializeResource } from "../src/resources/materialize.js";
import { MaterializeResourceRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";

test("resource materialization revalidates identity and access before owner commit", async () => {
  let revoked = false;
  let changed = false;
  let fetches = 0;
  let admissions = 0;
  let admittedInput: unknown;
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
        describe: async () => ({ resourceIds: [], accessible: true }),
        search: async () => [],
        materialize: async (reference) => {
          fetches++;
          expect(reference.entryVersion).toBe(null);
          return {
            reference,
            content: "selected material",
            mediaType: "text/plain",
            version: { status: "current", checkedAt: new Date().toISOString() },
            access: { status: "allowed", checkedAt: new Date().toISOString() },
          };
        },
        checkVersion: async () => ({
          status: changed ? "stale" : "current",
          checkedAt: new Date().toISOString(),
        }),
        checkAccess: async () => ({
          status: revoked ? "denied" : "allowed",
          checkedAt: new Date().toISOString(),
        }),
      },
    },
  ]);
  const kernel = {
    execution: coreExecutionSchema.parse(undefined),
    resourceRegistry: { getResource: async () => binding },
    runtime: {
      recordObservation: async (input: unknown) => {
        admissions++;
        admittedInput = input;
        return { occurrenceId: "occurrence" };
      },
    },
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
      providerResourceId: "provider-resource",
      entryId: "entry",
      contentDigest: "content-digest",
      sourceLocator: "document.txt",
      retrievedAt: new Date().toISOString(),
    },
  });
  changed = true;
  await expect(
    materializeResource(kernel, registry, request, {}),
  ).rejects.toMatchObject({
    code: 9,
  });
  changed = false;
  revoked = true;
  await expect(
    materializeResource(kernel, registry, request, {}),
  ).rejects.toMatchObject({
    code: 9,
  });
  expect(admissions).toBe(0);
  revoked = false;
  const accepted = await materializeResource(kernel, registry, request, {});
  expect(accepted.content).toBe("selected material");
  expect(fetches).toBe(3);
  expect(admissions).toBe(1);
  expect(admittedInput).toHaveProperty(
    "externalObjectRef",
    expect.stringMatching(/^object:resource-entry:[a-f0-9]{64}$/),
  );
  expect(admittedInput).toMatchObject({
    requestId: request.operationId,
    context: {
      external_resource: {
        resourceRef: "resource:test",
        entryId: "entry",
      },
    },
    material: {
      case: "inlineText",
      value: { text: "selected material", mediaType: "text/plain" },
    },
  });
  changed = true;
  await expect(
    materializeResource(kernel, registry, request, {}),
  ).rejects.toMatchObject({
    code: 9,
  });
  expect(fetches).toBe(4);
  expect(admissions).toBe(1);
});
