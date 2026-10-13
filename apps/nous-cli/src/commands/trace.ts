// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { boundedInteger } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";

export async function traceCommands(
  env: CliEnvironment,
  action: string | undefined,
) {
  return env.client.memory.trace({
    subjectId: env.subjectId,
    reference: await env.resolveReference(env.required(action, "Reference")),
    maxNodes: boundedInteger(env.values["max-nodes"], 1, 512, "--max-nodes"),
    maxBytes: boundedInteger(
      env.values["max-bytes"] ?? "65536",
      1,
      1048576,
      "--max-bytes",
    ),
  });
}
