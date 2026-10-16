// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { parseArgs } from "node:util";
import { resolve } from "node:path";
import { readFile } from "node:fs/promises";
import { fromJsonString } from "@bufbuild/protobuf";
import { CognitionUsageSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { serveStdio } from "@modelcontextprotocol/server/stdio";
import { cliExecutor, createMcpServer } from "./mcp.js";

try {
  const { values } = parseArgs({
    options: {
      "run-root": { type: "string" },
      "state-root": { type: "string" },
      consumer: { type: "string" },
      management: { type: "boolean", default: false },
      "source-scope-file": { type: "string" },
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
    management: values.management,
    trustedUsage: values["source-scope-file"]
      ? fromJsonString(
          CognitionUsageSchema,
          await readFile(resolve(values["source-scope-file"]), "utf8"),
        )
      : undefined,
  });
  serveStdio(() => createMcpServer(execute));
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
