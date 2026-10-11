// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
import { typedCognition } from "./cognition.js";

export async function journalCommands(
  env: CliEnvironment,
  action?: string,
  argument?: string,
) {
  const common = await typedCognition.commonCommand(
    env,
    "journal",
    action,
    argument,
    { list: true, history: true },
  );
  if (common.handled) return common.value;
  throw new CliError(
    "INVALID_ARGUMENT",
    "Use journal list|get|revision|history|suppress|restore|withdraw|reaccept|purge",
  );
}
