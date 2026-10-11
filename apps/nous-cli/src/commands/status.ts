// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { CliEnvironment } from "../runtime.js";

export async function statusCommands(env: CliEnvironment) {
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
