// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { ArgsDef, CommandDef } from "citty";
import type { CliEnvironment } from "../runtime.js";

export type CliAction = (
  env: CliEnvironment,
  argument?: string,
  positionals?: string[],
) => Promise<unknown>;

export type CliHelp = {
  description?: string;
  parameters?: readonly string[];
  example?: string;
};

export type CliLeaf = (
  args: ArgsDef,
  run: CliAction,
  help?: CliHelp,
) => CommandDef;
