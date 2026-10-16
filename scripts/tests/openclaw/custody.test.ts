// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  copyFile,
  cp,
  mkdir,
  mkdtemp,
  readdir,
  realpath,
  rm,
  symlink,
} from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "vitest";
import { buildDistribution } from "../../openclaw/distribution.js";
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
    await cp(
      join(source, "docs/reference/templates"),
      join(native, "docs/reference/templates"),
      { recursive: true },
    );
    // Link each dependency to its physical package root. A junction to pnpm's
    // node_modules leaves scoped packages behind another relative junction;
    // Windows ESM package lookup cannot reliably traverse that nested shape.
    // Independent controlled installs keep dependencies under the package;
    // pnpm's workspace package stores them beside it.
    const dependencies = await realpath(join(source, "node_modules")).catch(
      () => dirname(source),
    );
    const modules = join(native, "node_modules");
    await mkdir(modules);
    for (const entry of await readdir(dependencies)) {
      if (entry.startsWith(".") || entry === "openclaw") continue;
      if (entry.startsWith("@")) {
        await mkdir(join(modules, entry));
        for (const name of await readdir(join(dependencies, entry)))
          await symlink(
            await realpath(join(dependencies, entry, name)),
            join(modules, entry, name),
            "junction",
          );
      } else {
        await symlink(
          await realpath(join(dependencies, entry)),
          join(modules, entry),
          "junction",
        );
      }
    }
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
    assert(result.includes('"nativeAgentRpcFinalProducer":"PASS"'));
    assert(result.includes('"qqTraffic":0'));
    const tools = execFileSync(
      process.execPath,
      [join(repositoryRoot, "scripts/tests/openclaw/tool-scope.ts"), native],
      { cwd: repositoryRoot, env, encoding: "utf8", windowsHide: true },
    );
    assert(tools.includes('"nativeToolExecutionGuard":"PASS"'));
    const plugin = join(modules, "@nous-wave/openclaw");
    await buildDistribution(plugin);
    await symlink(native, join(modules, "openclaw"), "junction");
    const hosted = execFileSync(
      process.execPath,
      [
        join(repositoryRoot, "scripts/tests/openclaw/hosted-loader-probe.ts"),
        native,
        plugin,
      ],
      { cwd: repositoryRoot, env, encoding: "utf8", windowsHide: true },
    );
    assert(
      hosted.includes(
        "Actual installed SDK hosted tool declaration/registration PASS",
      ),
    );
    const bindings = execFileSync(
      process.execPath,
      [join(repositoryRoot, "scripts/tests/openclaw/tool-bindings.ts"), native],
      { cwd: repositoryRoot, env, encoding: "utf8", windowsHide: true },
    );
    assert(bindings.includes('"nativeAgentRpcToolBindings":"PASS"'));
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}, 180_000);
