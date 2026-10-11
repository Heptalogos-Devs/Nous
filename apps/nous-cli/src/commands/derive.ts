// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { stringArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

import type { CliEnvironment } from "../runtime.js";

async function deriveCommands(env: CliEnvironment, argument?: string) {
  const { client, values, subjectId, required } = env;
  return client.model.deriveMaterial({
    subjectId,
    sourceRegionId: (
      await env.resolveReference(
        required(argument, "Source region reference"),
        "source_region",
      )
    ).value,
    strategy: values.strategy,
    target: values.target,
    supersedes: values.supersedes,
  });
}

export function createDeriveCommand(leaf: CliLeaf) {
  return leaf(
    { ...stringArgs("strategy", "target", "supersedes") },
    deriveCommands,
    {
      description: "Derive material with an explicit strategy.",
      parameters: ["<SourceRegion reference>", "--strategy <strategy>"],
    },
  );
}
