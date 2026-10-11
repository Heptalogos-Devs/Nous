// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { execFile } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { promisify } from "node:util";
import { expect, test } from "vitest";
import { repositoryRoot, workspacePaths } from "../../workspace.js";

const execute = promisify(execFile);

test("checks physical boundaries and the actual tracked/untracked worktree with strict configuration", async () => {
  const temporary = await mkdtemp(
    join(workspacePaths.temporary, "code-length-"),
  );
  const command = [
    join(repositoryRoot, "node_modules/tsx/dist/cli.mjs"),
    join(repositoryRoot, "scripts/dev/check-code-length.ts"),
  ];
  const options = { cwd: temporary, windowsHide: true };
  const run = async () => {
    try {
      const output = await execute(process.execPath, command, options);
      return { ...output, code: 0 };
    } catch (error) {
      return error as { stdout: string; stderr: string; code: number };
    }
  };
  try {
    await execute("git", ["init", "-q"], options);
    await mkdir(join(temporary, ".config/scripts"), { recursive: true });
    const configPath = join(temporary, ".config/scripts/code-length.toml");
    const config = `[typescript]
extensions = [".ts", ".tsx", ".mts", ".cts"]
warn_at = 3
reject_at = 5
[rust]
extensions = [".rs"]
warn_at = 2
reject_at = 4
[generated]
exclude = ["crates/protocol/src/generated/**", "packages/protocol-ts/src/generated/**"]
`;
    await writeFile(configPath, config);
    await writeFile(
      join(temporary, "边界 tracked.ts"),
      "\ufeff" + "a\r\n".repeat(2),
    );
    await writeFile(join(temporary, "deleted.rs"), "a\n".repeat(4));
    await writeFile(join(temporary, ".gitignore"), "ignored.ts\n");
    await execute("git", ["add", "."], options);
    await rm(join(temporary, "deleted.rs"));
    await mkdir(join(temporary, "tests/support"), { recursive: true });
    await writeFile(join(temporary, "tests/new.test.ts"), "a\r".repeat(3));
    await writeFile(join(temporary, "tests/support/anchors.rs"), "a\na");
    await writeFile(join(temporary, "empty.mts"), "");
    await writeFile(join(temporary, "ignored.ts"), "a\n".repeat(5));
    const generated = join(temporary, "crates/protocol/src/generated");
    await mkdir(generated, { recursive: true });
    await writeFile(join(generated, "wire.rs"), "a\n".repeat(4));
    const warnings = await run();
    expect(warnings.code).toBe(0);
    expect(warnings.stdout).toContain("WARN tests/new.test.ts 3 lines");
    expect(warnings.stdout).toContain("WARN tests/support/anchors.rs 2 lines");
    expect(warnings.stdout).toContain(
      "4 files, 2 warnings, 0 rejected, 0 read errors",
    );
    await writeFile(join(temporary, "边界 tracked.ts"), "a\n".repeat(5));
    await writeFile(join(temporary, "tests/support/new.rs"), "a\n".repeat(4));
    const rejected = await run();
    expect(rejected.code).toBe(1);
    expect(rejected.stdout).toContain("ERROR 边界 tracked.ts 5 lines");
    expect(rejected.stdout).toContain("ERROR tests/support/new.rs 4 lines");
    await writeFile(configPath, config + "\nunknown = true\n");
    const invalid = await run();
    expect(invalid.code).toBe(2);
    expect(invalid.stderr).toContain("unknown");
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});
