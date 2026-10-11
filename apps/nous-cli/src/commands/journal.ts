// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import { commonArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";
import { cognitionSubcommands } from "./cognition.js";

export function createJournalCommand(leaf: CliLeaf) {
  const cognition = cognitionSubcommands(leaf, {
    kind: "journal",
    label: "Journal",
    list: true,
    history: true,
  });
  return defineCommand({
    args: commonArgs,
    subCommands: {
      ...cognition.reads,
      ...cognition.lifecycle,
    },
  });
}
