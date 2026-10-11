// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { CliEnvironment } from "../runtime.js";

export async function embeddingsCommands(env: CliEnvironment) {
  const { client, values, subjectId } = env;
  const budget = Number(values["max-batches"]);
  if (!Number.isInteger(budget) || budget < 1 || budget > 10_000)
    throw new Error("--max-batches must be 1..10000");
  let committed = 0;
  for (let requests = 0; requests < budget; requests++) {
    const batch = await client.model.prepareEmbeddings({
      subjectId,
      limit: 64,
    });
    committed += batch.committed;
    if (batch.committed === 0 || batch.degradation.length)
      return {
        committed,
        requests: requests + 1,
        degradation: batch.degradation,
      };
  }
  return {
    committed,
    degradation: [
      {
        code: "caller_budget_exhausted",
        detail: "Embedding batch budget reached",
      },
    ],
  };
}
