// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { parseArgs } from "node:util";
import { resolve } from "node:path";
import { serveStdio } from "@modelcontextprotocol/server/stdio";
import { cliExecutor, createMcpServer } from "./mcp.js";

try {
  const { values } = parseArgs({
    options: {
      "run-root": { type: "string" },
      "instance-root": { type: "string" },
      "state-root": { type: "string" },
      consumer: { type: "string" },
    },
  });
  if (!values["run-root"] || !values["state-root"] || !values.consumer)
    throw new Error(
      "Use nous mcp --state-root <unique Agent directory> --consumer <stable consumer ref> with a launcher location or --run-root.",
    );
  const execute = cliExecutor({
    runRoot: resolve(values["run-root"]),
    stateRoot: resolve(values["state-root"]),
    consumer: values.consumer,
  });
  serveStdio(() => createMcpServer(execute));
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
