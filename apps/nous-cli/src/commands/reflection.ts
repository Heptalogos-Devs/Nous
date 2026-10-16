// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { ReflectionRepairRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/reflection_pb.js";
import { content } from "../content.js";
import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
export async function reflectionCommands(
  env: CliEnvironment,
  action?: string,
  ref?: string,
) {
  const subjectId = await env.subjectId;
  if (action === "status")
    return env.client.reflection.inspect({
      subjectId,
      limit: 16,
      maxPayloadBytes: 1048576,
      operationId: ref,
    });
  if (action === "repair")
    return env.client.reflection.repair({
      ...(await content(env, ReflectionRepairRequestSchema)),
      subjectId,
    });
  throw new CliError(
    "INVALID_ARGUMENT",
    "Reflection command requires status or explicit reviewed repair",
  );
}
