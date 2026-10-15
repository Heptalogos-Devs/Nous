// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { copyFile, cp, mkdtemp, rm, symlink } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "vitest";
import { repositoryRoot } from "../../workspace.js";

test("controlled native custody survives SQLite and gates real delivery producers", async () => {
  // Use the pinned workspace dependency, never an operator installation or a
  // network install. Copy runtime code before prepare; dependencies stay read-only.
  const nativeRequire = createRequire(
    join(repositoryRoot, "packages/openclaw/package.json"),
  );
  const source = dirname(
    dirname(
      dirname(nativeRequire.resolve("openclaw/plugin-sdk/channel-message")),
    ),
  );
  const temporary = await mkdtemp(join(tmpdir(), "nous-custody-offline-"));
  const native = join(temporary, "native");
  try {
    await cp(join(source, "dist"), join(native, "dist"), { recursive: true });
    await copyFile(join(source, "package.json"), join(native, "package.json"));
    await symlink(dirname(source), join(native, "node_modules"), "junction");
    const env = {
      ...process.env,
      // Reproduce Windows/user Git newline preferences on every platform;
      // prepare must still produce the exact manifest bytes.
      GIT_CONFIG_COUNT: "2",
      GIT_CONFIG_KEY_0: "core.autocrlf",
      GIT_CONFIG_VALUE_0: "true",
      GIT_CONFIG_KEY_1: "core.eol",
      GIT_CONFIG_VALUE_1: "crlf",
      OPENCLAW_STATE_DIR: join(temporary, "state"),
      OPENCLAW_CONFIG_PATH: join(temporary, "openclaw.json"),
      TMPDIR: temporary,
      TMP: temporary,
      TEMP: temporary,
    };
    execFileSync(
      process.execPath,
      [join(repositoryRoot, "packages/openclaw/bin/prepare.mjs"), native],
      { cwd: repositoryRoot, env, encoding: "utf8", windowsHide: true },
    );
    const result = execFileSync(
      process.execPath,
      [join(repositoryRoot, "scripts/tests/openclaw/custody.ts"), native],
      { cwd: repositoryRoot, env, encoding: "utf8", windowsHide: true },
    );
    assert(result.includes('"nativeSqliteStableIntent":"PASS"'));
    assert(result.includes('"nativeMessageSkipQueueProducer":"PASS"'));
    assert(result.includes('"qqTraffic":0'));
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}, 120_000);
