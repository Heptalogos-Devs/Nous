#!/usr/bin/env node
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { execFileSync } from "node:child_process";
import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Root npm policy is required: dependency-package overrides do not affect consumers.
const plugin = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const prefix = process.argv[2] ? resolve(process.argv[2]) : undefined;
if (!prefix || process.argv.length !== 3)
  throw new Error("Usage: nous-openclaw-install <runtime-directory>");
const manifest =
  /** @type {{version:string, dependencyOverrides:Record<string,string>}} */ (
    JSON.parse(await readFile(join(plugin, "patches/manifest.json"), "utf8"))
  );
const existing = await readFile(join(prefix, "package.json"), "utf8")
  .then(
    (text) => /** @type {{nousManagedRuntime?:number}} */ (JSON.parse(text)),
  )
  .catch((error) => {
    if (error instanceof Error && "code" in error && error.code === "ENOENT")
      return undefined;
    throw error;
  });
if (existing && existing.nousManagedRuntime !== 1)
  throw new Error(
    "Choose a fresh runtime directory; this directory is not owned by the Nous native installer",
  );
await mkdir(join(prefix, "artifacts"), { recursive: true });
const npmCli =
  process.platform === "win32"
    ? process.env.npm_execpath?.endsWith("npm-cli.js")
      ? process.env.npm_execpath
      : join(dirname(process.execPath), "node_modules/npm/bin/npm-cli.js")
    : undefined;
const npm = npmCli ? process.execPath : "npm";
/** @param {string[]} args */
const npmArguments = (args) => (npmCli ? [npmCli, ...args] : args);
const packed = /** @type {{filename:string,shasum:string}[]} */ (
  JSON.parse(
    execFileSync(
      npm,
      [
        "pack",
        plugin,
        "--json",
        "--pack-destination",
        join(prefix, "artifacts"),
      ],
      { cwd: prefix, encoding: "utf8", windowsHide: true },
    ),
  )
);
const packedArtifact = packed[0];
if (!packedArtifact?.filename || !/^[a-f0-9]{40}$/.test(packedArtifact.shasum))
  throw new Error("npm pack returned no identifiable plugin artifact");
// npm otherwise reuses the old lock entry when a development build keeps its version.
const filename = `${basename(packedArtifact.filename, ".tgz")}-${packedArtifact.shasum}.tgz`;
await rename(
  join(prefix, "artifacts", packedArtifact.filename),
  join(prefix, "artifacts", filename),
);
await writeFile(
  join(prefix, "package.json"),
  JSON.stringify(
    {
      name: "nous-openclaw-runtime",
      version: "1.0.0",
      private: true,
      nousManagedRuntime: 1,
      dependencies: {
        openclaw: manifest.version,
        "@nous-wave/openclaw": `file:artifacts/${filename}`,
      },
      overrides: manifest.dependencyOverrides,
    },
    null,
    2,
  ) + "\n",
);
execFileSync(npm, npmArguments(["install", "--no-audit", "--no-fund"]), {
  cwd: prefix,
  stdio: "inherit",
  windowsHide: true,
});
execFileSync(
  process.execPath,
  [
    join(prefix, "node_modules/@nous-wave/openclaw/bin/prepare.mjs"),
    join(prefix, "node_modules/openclaw"),
  ],
  { cwd: prefix, stdio: "inherit", windowsHide: true },
);
process.stdout.write(`Installed controlled runtime: ${prefix}\n`);
