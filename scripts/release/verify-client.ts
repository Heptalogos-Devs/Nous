// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { execFileSync, execSync } from "node:child_process";
import { cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { repositoryRoot } from "../workspace.js";
import { packClient } from "./client.js";

/** Exercise the shipped artifact outside repository resolution and without a TS loader. */
export async function verifyClient(runRoot?: string) {
  const artifact = await packClient();
  const consumer = await mkdtemp(join(tmpdir(), "nous-client-consumer-"));
  const require = createRequire(import.meta.url);
  const env = { ...process.env, NODE_OPTIONS: "", NODE_PATH: "" };
  const workspace = JSON.parse(
    await readFile(join(repositoryRoot, "package.json"), "utf8"),
  ) as {
    devDependencies: Record<string, string>;
  };
  const source = JSON.parse(
    await readFile(
      join(repositoryRoot, "packages/client/package.json"),
      "utf8",
    ),
  ) as {
    exports: Record<string, string>;
  };
  const expected: Record<string, string[]> = {};
  for (const [entry, path] of Object.entries(source.exports))
    expected[entry] = Object.keys(
      (await import(
        pathToFileURL(join(repositoryRoot, "packages/client", path)).href
      )) as Record<string, unknown>,
    ).sort();
  await writeFile(
    join(consumer, "package.json"),
    JSON.stringify({
      private: true,
      type: "module",
      dependencies: { "@nous-wave/client": pathToFileURL(artifact).href },
      devDependencies: {
        "@types/node": workspace.devDependencies["@types/node"],
      },
    }),
  );
  try {
    execSync("npm install --ignore-scripts --no-audit --no-fund", {
      cwd: consumer,
      env,
      stdio: "inherit",
      windowsHide: true,
    });
    await cp(
      join(repositoryRoot, "scripts/tests/release/client-consumer.ts"),
      join(consumer, "consumer.ts"),
    );
    await writeFile(
      join(consumer, "tsconfig.json"),
      JSON.stringify({
        compilerOptions: {
          target: "ES2023",
          module: "NodeNext",
          moduleResolution: "NodeNext",
          strict: true,
          noUncheckedIndexedAccess: true,
          skipLibCheck: false,
          types: ["node"],
          declaration: true,
          outDir: "dist",
        },
        files: ["consumer.ts"],
      }),
    );
    execFileSync(
      process.execPath,
      [
        join(dirname(require.resolve("typescript/package.json")), "bin/tsc"),
        "--project",
        join(consumer, "tsconfig.json"),
      ],
      {
        cwd: consumer,
        env,
        stdio: "inherit",
        windowsHide: true,
      },
    );
    const result = execFileSync(
      process.execPath,
      [join(consumer, "dist/consumer.js")],
      {
        cwd: consumer,
        env: {
          ...env,
          NOUS_CLIENT_RUN_ROOT: runRoot ?? "",
          NOUS_CLIENT_EXPORTS: JSON.stringify(expected),
          NOUS_CLIENT_ARTIFACT: artifact,
        },
        encoding: "utf8",
        windowsHide: true,
      },
    );
    assert(result.includes('"types":"PASS"'));
    process.stdout.write(result);
    await rm(consumer, { recursive: true, force: true });
    return artifact;
  } catch (error) {
    console.error(`Independent Client consumer retained at ${consumer}`);
    throw error;
  }
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
)
  await verifyClient();
