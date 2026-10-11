// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import type { CliLeaf } from "./command-types.js";

import type { CliEnvironment } from "../runtime.js";

async function statusCommands(env: CliEnvironment) {
  const { client } = env;
  return {
    status: await client.system.status({}),
    capabilities: await client.system.capabilities({}),
    ...(env.subjectId
      ? {
          subjectId: env.subjectId,
          serving: await client.system.projections({
            subjectId: env.subjectId,
          }),
        }
      : {}),
  };
}

export function createStatusCommand(leaf: CliLeaf) {
  return leaf({}, statusCommands, {
    description: "Instance status and capabilities.",
  });
}
