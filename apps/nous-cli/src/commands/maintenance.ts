// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { boundedInteger } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";

export async function maintenanceCommands(env: CliEnvironment) {
  const { client, values, subjectId } = env;
  const result = await client.cognition.grantMaintenance({
    subjectId,
    maxOperations: boundedInteger(
      values["max-operations"],
      1,
      32,
      "--max-operations",
    ),
    maxModelCalls: boundedInteger(
      values["max-model-calls"],
      0,
      32,
      "--max-model-calls",
    ),
    maxElapsedMs: boundedInteger(
      values["max-elapsed-ms"],
      1,
      900000,
      "--max-elapsed-ms",
    ),
  });
  return {
    ...result,
    ...(result.disposition === "disabled_by_policy"
      ? {
          next: `config get maintenance.enabled --desired --subject ${subjectId}; then config set maintenance.enabled true --subject ${subjectId} --expected-revision <configurationRevision>`,
        }
      : {}),
  };
}
