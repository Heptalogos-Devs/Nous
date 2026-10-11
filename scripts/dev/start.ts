// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { existsSync } from "node:fs";

import { join } from "node:path";
import { repositoryRoot as root, developmentLocator } from "../workspace.js";
import { spawn } from "node:child_process";
import { initializeConfiguration } from "../../apps/nous-core/src/configuration/file.js";
import { resolveLocations } from "../../apps/nous-core/src/locations.js";

const binary = join(
  root,
  "target",
  "debug",
  process.platform === "win32" ? "nous-kernel.exe" : "nous-kernel",
);
if (!existsSync(binary))
  throw new Error("Build the Kernel first: cargo build -p nous-kernel");
const locator = await developmentLocator();
const locations = await resolveLocations({ locator, installationHome: root });
await initializeConfiguration(locations);
const child = spawn(
  process.execPath,
  [
    join(root, "node_modules/tsx/dist/cli.mjs"),
    join(root, "apps/nous-core/src/main.ts"),
    "--locator",
    locator,
    "--development",
  ],
  { stdio: "inherit", windowsHide: true },
);
child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  else process.exitCode = code ?? 1;
});
