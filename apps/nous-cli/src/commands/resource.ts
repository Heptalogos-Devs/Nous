// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { ResourceDescriptorSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { StableExternalRefSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { CliError } from "../agent.js";
import { content, operationId, page, target } from "../content.js";
import type { CliEnvironment } from "../runtime.js";
export async function resourceCommands(
  env: CliEnvironment,
  action?: string,
  ref?: string,
) {
  const { client, subjectId } = env;
  if (action === "list")
    return client.resourceRegistry.listResources({
      subjectId,
      status: env.values.status,
      page: page(env),
    });
  if (action === "put")
    return client.resourceRegistry.putResource({
      subjectId,
      descriptor: await content(env, ResourceDescriptorSchema),
    });
  if (action === "materialize")
    return client.resources.materializeResource({
      subjectId,
      operationId: operationId(env),
      reference: await content(env, StableExternalRefSchema),
      observedAt: { seconds: BigInt(Math.floor(Date.now() / 1000)), nanos: 0 },
    });
  const id = await target(env, ref, "resource");
  if (action === "get")
    return client.resourceRegistry.getResource({ subjectId, id });
  if (action === "remove")
    return client.resourceRegistry.removeResource({ subjectId, id });
  throw new CliError("INVALID_ARGUMENT", "Unknown Resource command");
}
