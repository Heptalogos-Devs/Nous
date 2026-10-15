#!/usr/bin/env node
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFile, realpath, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Ships inside the standalone plugin, without build tools or a repository.
const plugin = resolve(dirname(fileURLToPath(import.meta.url)), "..");
if (!process.argv[2])
  throw new Error("Usage: nous-openclaw-prepare <openclaw-package-root>");
const native = await realpath(resolve(process.argv[2]));
const manifest =
  /** @type {{version: string, files: Record<string, {original: string, patched: string}>}} */ (
    JSON.parse(await readFile(join(plugin, "patches/manifest.json"), "utf8"))
  );
const installed = /** @type {{name: string, version: string}} */ (
  JSON.parse(await readFile(join(native, "package.json"), "utf8"))
);
if (installed.name !== "openclaw" || installed.version !== manifest.version)
  throw new Error(`Managed cognition requires OpenClaw ${manifest.version}`);
/** @param {Uint8Array} bytes */
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
const states = await Promise.all(
  Object.entries(manifest.files).map(async ([path, hashes]) => ({
    path,
    hashes,
    actual: digest(await readFile(join(native, path))),
  })),
);
if (
  states.some(
    ({ actual, hashes }) =>
      actual !== hashes.original && actual !== hashes.patched,
  )
)
  throw new Error(
    "Installed OpenClaw code differs from the controlled distribution",
  );
if (!states.every(({ actual, hashes }) => actual === hashes.patched)) {
  if (!states.every(({ actual, hashes }) => actual === hashes.original))
    throw new Error(
      "OpenClaw installation contains only part of the required patch",
    );
  const patch = join(plugin, `patches/openclaw-${manifest.version}.patch`);
  const args = ["apply", "--no-index", "--unsafe-paths", "--whitespace=nowarn"];
  // A package can live below an unrelated Git checkout. Stop repository
  // discovery here so Git cannot silently skip package-relative patch paths.
  const environment = {
    ...process.env,
    GIT_CEILING_DIRECTORIES: dirname(native),
  };
  execFileSync("git", [...args, "--check", patch], {
    cwd: native,
    env: environment,
    windowsHide: true,
  });
  execFileSync("git", [...args, patch], {
    cwd: native,
    env: environment,
    windowsHide: true,
  });
}
for (const [path, hashes] of Object.entries(manifest.files)) {
  if (digest(await readFile(join(native, path))) !== hashes.patched)
    throw new Error(`Required native model input patch did not apply: ${path}`);
  execFileSync(process.execPath, ["--check", join(native, path)], {
    windowsHide: true,
  });
}
await writeFile(
  join(native, "nous-managed-cognition.json"),
  JSON.stringify(manifest, null, 2) + "\n",
);
process.stdout.write(
  `Prepared OpenClaw ${manifest.version}; plugin=${plugin}\n`,
);
