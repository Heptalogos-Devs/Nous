// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson } from "@bufbuild/protobuf";
import { ValueSchema } from "@bufbuild/protobuf/wkt";
import { ConfigurationSnapshotSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";
import { consumerStatePolicySchema } from "@nous-wave/client/consumer-policy";
import type { connectNousInstance } from "@nous-wave/client/node";
import { runCli as execute } from "../../src/commands/index.js";

export const identity = {
  instanceId: "11111111-1111-4111-8111-111111111111",
  consumer: "default",
};
export const policy = consumerStatePolicySchema.parse({ receipt_limit: 2 });
type Connection = Awaited<ReturnType<typeof connectNousInstance>>;
function consumerClient(value: Connection): Connection {
  return {
    ...value,
    instanceId: identity.instanceId,
    identity: {
      ...value.identity,
      resolveIdentity: async (request, options) =>
        ["subject", "session", "work_context"].includes(request.kind ?? "") &&
        request.locator?.case === "name" &&
        !/^[0-9a-f-]{36}$/i.test(request.locator.value)
          ? {
              $typeName: "nous.wave.v1alpha1.ResolveIdentityResponse",
              status: "BOUND",
              candidates: [
                {
                  $typeName: "nous.wave.v1alpha1.IdentityBinding",
                  status: "BOUND",
                  canonical: {
                    $typeName: "nous.wave.v1alpha1.CognitiveRef",
                    kind: request.kind!,
                    value: request.locator.value,
                  },
                  lexicalRef: "",
                  displayName: "",
                  aliases: [],
                },
              ],
            }
          : value.identity.resolveIdentity(request, options),
    },
    configuration: {
      ...value.configuration,
      getConfiguration: async (input, options) =>
        input.paths?.includes("consumer_state.receipt_limit")
          ? create(ConfigurationSnapshotSchema, {
              catalogDigest: "test",
              configurationRevision: 1n,
              effectiveDigest: "test",
              entries: Object.entries(policy).map(([name, fieldValue]) => ({
                path: `consumer_state.${name}`,
                value: fromJson(ValueSchema, fieldValue),
                source: "reference_default",
                owner: "official-consumer",
                applyMode: "restart_process",
              })),
            })
          : value.configuration.getConfiguration(input, options),
    },
  };
}
export function runCli(
  ...[args, connect, present, input]: Parameters<typeof execute>
) {
  return execute(
    args,
    connect && (async (location) => consumerClient(await connect(location))),
    present,
    input,
  );
}
