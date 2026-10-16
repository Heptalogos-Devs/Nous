// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import {
  ConfigureClockRequestSchema,
  AdvanceClockRequestSchema,
  ResumeClockRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/clock_pb.js";
import { content, operationId } from "../content.js";
import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
export async function clockCommands(
  env: CliEnvironment,
  action: string | undefined,
) {
  const identity = {
    subjectId: env.required(await env.subjectId, "Selected Subject"),
  };
  if (action === "read") return env.client.clocks.readClock(identity);
  const mutation = { ...identity, operationId: operationId(env) };
  if (action === "pause") return env.client.clocks.pauseClock(mutation);
  if (action === "configure")
    return env.client.clocks.configureClock({
      ...(await content(env, ConfigureClockRequestSchema)),
      ...mutation,
    });
  if (action === "advance")
    return env.client.clocks.advanceClock({
      ...(await content(env, AdvanceClockRequestSchema)),
      ...mutation,
    });
  if (action === "resume")
    return env.client.clocks.resumeClock({
      ...(await content(env, ResumeClockRequestSchema)),
      ...mutation,
    });
  throw new CliError("INVALID_ARGUMENT", "Unknown clock command");
}
