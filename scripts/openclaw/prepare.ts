// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { execFileSync } from "node:child_process";
import { realpath } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { repositoryRoot } from "../workspace.js";
import { buildDistribution } from "./distribution.js";

const native = await realpath(
  resolve(
    process.argv[2] ??
      join(repositoryRoot, "data/runtime/openclaw/node_modules/openclaw"),
  ),
);
const plugin = join(dirname(native), "@nous-wave/openclaw");
await buildDistribution(plugin);
execFileSync(process.execPath, [join(plugin, "bin/prepare.mjs"), native], {
  stdio: "inherit",
  windowsHide: true,
});
