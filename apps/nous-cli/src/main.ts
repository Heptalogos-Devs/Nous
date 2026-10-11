// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { runCli } from "./commands/index.js";
import { formatError, formatResult } from "./format.js";
import { friendlyOutput } from "./friendly.js";
const args = process.argv.slice(2);
runCli(args, undefined, friendlyOutput, process.stdin)
  .then((result) => console.log(formatResult(result, args)))
  .catch((error: unknown) => {
    console.error(formatError(error, args));
    process.exitCode = 1;
  });
