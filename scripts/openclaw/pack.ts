// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { workspacePaths } from "../workspace.js";
import { buildDistribution } from "./distribution.js";

const output = join(workspacePaths.releases, "openclaw");
const directory = join(output, "package");
await buildDistribution(directory);
execFileSync(
  process.platform === "win32" ? "npm.cmd" : "npm",
  ["pack", "--pack-destination", output],
  { cwd: directory, stdio: "inherit", windowsHide: true },
);
