// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import { commonArgs, stringArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { boundedInteger } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
import { z } from "zod";

const descriptorInput = z.strictObject({
  resource_ref: z.string().min(1).max(1024),
  display_label: z.string().min(1).max(512),
  authority_class: z.string().min(1).max(128),
  coverage: z.record(z.string(), z.json()).default({}),
  query_dimensions: z.array(z.string().min(1).max(128)).max(64).default([]),
  modalities: z.array(z.string().min(1).max(128)).max(32).default([]),
  freshness_policy: z.record(z.string(), z.json()).default({}),
  access_cost_class: z.string().min(1).max(128),
  readiness: z.string().min(1).max(128),
  adapter_kind: z.string().min(1).max(128),
  provider_profile: z.string().min(1).max(256),
  provider_locator: z.string().min(1).max(2048),
});
const materializeInput = z.strictObject({
  provider_kind: z.string().min(1).max(128),
  provider_profile: z.string().min(1).max(256),
  profile_digest: z.string().regex(/^[0-9a-f]{64}$/),
  resource_ref: z.string().min(1).max(1024),
  provider_resource_id: z.string().min(1).max(2048),
  entry_id: z.string().min(1).max(2048),
  entry_version: z.string().max(512).optional(),
  content_digest: z.string().regex(/^[0-9a-f]{64}$/),
  source_locator: z.string().min(1).max(2048),
  retrieved_at: z.string().datetime({ offset: true }),
  access_scope: z.string().min(1).max(2048),
  observed_at: z.string().datetime({ offset: true }),
});

const resourceList = (env: CliEnvironment) =>
  env.client.resources.list({
    subjectId: env.subjectId,
    page: {
      pageSize: boundedInteger(env.values["page-size"], 1, 200, "--page-size"),
      pageToken: env.values["page-token"],
    },
    status: env.values.status ?? "",
  });
const resourcePut = async (env: CliEnvironment) => {
  const { client, values, subjectId, requestPayload } = env;
  const input = descriptorInput.parse(
    await requestPayload(values["request-file"]),
  );
  const descriptor = {
    resourceRef: input.resource_ref,
    displayLabel: input.display_label,
    authorityClass: input.authority_class,
    coverage: input.coverage,
    queryDimensions: input.query_dimensions,
    modalities: input.modalities,
    freshnessPolicy: input.freshness_policy,
    accessCostClass: input.access_cost_class,
    readiness: input.readiness,
    adapterKind: input.adapter_kind,
    providerProfile: input.provider_profile,
    providerLocator: input.provider_locator,
  };
  return client.resources.put({ subjectId, descriptor });
};
const resourceMaterialize = async (env: CliEnvironment) => {
  const { client, values, subjectId, requestPayload } = env;
  const input = materializeInput.parse(
    await requestPayload(values["request-file"]),
  );
  const reference = {
    providerKind: input.provider_kind,
    providerProfile: input.provider_profile,
    profileDigest: input.profile_digest,
    resourceRef: input.resource_ref,
    providerResourceId: input.provider_resource_id,
    entryId: input.entry_id,
    entryVersion: input.entry_version,
    contentDigest: input.content_digest,
    sourceLocator: input.source_locator,
    retrievedAt: input.retrieved_at,
    accessScope: input.access_scope,
  };
  return client.resources.materialize({
    subjectId,
    operationId: values["operation-id"] ?? crypto.randomUUID(),
    reference,
    observedAt: timestampFromDate(new Date(input.observed_at)),
  });
};
const resourceGet = async (env: CliEnvironment, argument?: string) => {
  const ref = await env.resolveReference(
    env.required(argument, "Resource reference"),
    "resource",
  );
  return env.client.resources.get({ subjectId: env.subjectId, id: ref.value });
};
const resourceRemove = async (env: CliEnvironment, argument?: string) => {
  const ref = await env.resolveReference(
    env.required(argument, "Resource reference"),
    "resource",
  );
  return env.client.resources.remove({
    subjectId: env.subjectId,
    id: ref.value,
  });
};

export function createResourceCommand(leaf: CliLeaf) {
  return defineCommand({
    args: commonArgs,
    subCommands: {
      list: leaf(
        { ...stringArgs("page-size", "page-token", "status") },
        resourceList,
      ),
      put: leaf({ ...stringArgs("request-file") }, resourcePut),
      get: leaf({}, resourceGet, {
        parameters: ["<Resource reference>"],
      }),
      remove: leaf({}, resourceRemove, {
        parameters: ["<Resource reference>"],
      }),
      materialize: leaf(
        { ...stringArgs("request-file", "operation-id") },
        resourceMaterialize,
      ),
    },
  });
}
