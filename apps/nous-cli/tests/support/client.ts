// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { consumerStatePolicySchema } from "@nous-wave/client/consumer-policy";
import type { connectNousInstance } from "@nous-wave/client/node";
import { runCli as execute } from "../../src/commands/index.js";
import { createEnvironment as environment } from "../../src/runtime.js";

export const identity = {
  instanceId: "11111111-1111-4111-8111-111111111111",
  consumer: "consumer:nous-cli:default",
};
export const policy = consumerStatePolicySchema.parse({ receipt_limit: 2 });
type Connection = Awaited<ReturnType<typeof connectNousInstance>>;
function consumerClient(value: Connection): Connection {
  return {
    ...value,
    instanceId: identity.instanceId,
    configuration: {
      ...value.configuration,
      get: async (input, options) =>
        input.paths?.includes("consumer_state.receipt_limit")
          ? {
              catalogDigest: "test",
              configurationRevision: 1n,
              effectiveDigest: "test",
              entries: Object.entries(policy).map(([name, fieldValue]) => ({
                path: `consumer_state.${name}`,
                value: fieldValue,
                source: "reference_default",
                owner: "official-consumer",
                applyMode: "restart_process",
              })),
            }
          : value.configuration.get(input, options),
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
export function createEnvironment(
  ...[values, connect, input]: Parameters<typeof environment>
) {
  return environment(
    values,
    connect && (async (location) => consumerClient(await connect(location))),
    input,
  );
}
