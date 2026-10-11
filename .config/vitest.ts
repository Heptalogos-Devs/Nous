// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { defineConfig } from "vitest/config";
import { workspacePaths } from "../scripts/workspace.js";

const temporary = join(workspacePaths.temporary, "tests");
mkdirSync(temporary, { recursive: true });
const runTemporary = mkdtempSync(join(temporary, "vitest-"));
process.env.TMP = runTemporary;
process.env.TEMP = runTemporary;
process.env.TMPDIR = runTemporary;
process.once("exit", () =>
  rmSync(runTemporary, { recursive: true, force: true }),
);

export default defineConfig({
  test: {
    include: [
      "apps/**/*.test.ts",
      "packages/**/*.test.ts",
      "scripts/**/*.test.ts",
    ],
  },
  cacheDir: join(workspacePaths.cache, "vitest"),
});
