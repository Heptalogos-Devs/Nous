#!/usr/bin/env node
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFile, realpath, writeFile, mkdir } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

// Ships inside the standalone plugin, without build tools or a repository.
const plugin = resolve(dirname(fileURLToPath(import.meta.url)), "..");
if (!process.argv[2])
  throw new Error("Usage: nous-openclaw-prepare <openclaw-package-root>");
const native = await realpath(resolve(process.argv[2]));
const manifest =
  /** @type {{version: string, files: Record<string, {original: string, patched: string}>, dependencies: Record<string,string>, dependencyPatches?:Record<string,{patch:string,files:Record<string,{original:string,patched:string}>}>, managedFiles?:Record<string,string>}} */ (
    JSON.parse(await readFile(join(plugin, "patches/manifest.json"), "utf8"))
  );
const installed = /** @type {{name: string, version: string}} */ (
  JSON.parse(await readFile(join(native, "package.json"), "utf8"))
);
if (installed.name !== "openclaw" || installed.version !== manifest.version)
  throw new Error(`Managed cognition requires OpenClaw ${manifest.version}`);
// Validate actual Node resolution, including hoisted and pnpm-installed dependencies.
const nativeRequire = createRequire(join(native, "package.json"));
const requires = [
  nativeRequire,
  createRequire(nativeRequire.resolve("@openclaw/proxyline")),
];
/** @type {Map<string, Set<string>>} */
const dependencyRoots = new Map();
for (const require of requires)
  for (const [name, version] of Object.entries(manifest.dependencies)) {
    let directory = dirname(
      require.resolve(
        name === "@modelcontextprotocol/sdk" ? `${name}/client/index.js` : name,
      ),
    );
    for (;;) {
      const metadata = await readFile(join(directory, "package.json"), "utf8")
        .then(
          (text) =>
            /** @type {{name?:string,version?:string}} */ (JSON.parse(text)),
        )
        .catch((error) => {
          if (
            error instanceof Error &&
            "code" in error &&
            error.code === "ENOENT"
          )
            return undefined;
          throw error;
        });
      if (metadata?.name === name) {
        if (metadata.version !== version)
          throw new Error(
            `Controlled runtime requires ${name}@${version}; use nous-openclaw-install`,
          );
        const roots = dependencyRoots.get(name) ?? new Set();
        roots.add(await realpath(directory));
        dependencyRoots.set(name, roots);
        break;
      }
      const parent = dirname(directory);
      if (parent === directory)
        throw new Error(`Cannot verify native dependency ${name}`);
      directory = parent;
    }
  }
/** @param {Uint8Array} bytes */
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
/** @param {string} root @param {Record<string,{original:string,patched:string}>} files @param {string} patchName */
async function prepareCode(root, files, patchName) {
  const states = await Promise.all(
    Object.entries(files).map(async ([path, hashes]) => ({
      path,
      hashes,
      actual: digest(await readFile(join(root, path))),
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
    const patch = join(plugin, patchName);
    // Native npm files and the manifest describe exact LF bytes. Git's host
    // checkout preferences must not rewrite the installed runtime to CRLF.
    const args = [
      "-c",
      "core.autocrlf=false",
      "-c",
      "core.eol=lf",
      "apply",
      "--no-index",
      "--unsafe-paths",
      "--whitespace=nowarn",
    ];
    // A package can live below an unrelated Git checkout. Stop repository
    // discovery here so Git cannot silently skip package-relative patch paths.
    const environment = {
      ...process.env,
      GIT_CEILING_DIRECTORIES: dirname(root),
    };
    execFileSync("git", [...args, "--check", patch], {
      cwd: root,
      env: environment,
      windowsHide: true,
    });
    execFileSync("git", [...args, patch], {
      cwd: root,
      env: environment,
      windowsHide: true,
    });
  }
  for (const [path, hashes] of Object.entries(files)) {
    if (digest(await readFile(join(root, path))) !== hashes.patched)
      throw new Error(
        `Required native model input patch did not apply: ${path}`,
      );
    execFileSync(process.execPath, ["--check", join(root, path)], {
      windowsHide: true,
    });
  }
}
await prepareCode(
  native,
  manifest.files,
  `patches/openclaw-${manifest.version}.patch`,
);
for (const [name, patch] of Object.entries(manifest.dependencyPatches ?? {})) {
  const roots = dependencyRoots.get(name);
  if (!roots?.size)
    throw Error(`Controlled provider dependency is unresolved: ${name}`);
  for (const root of roots) await prepareCode(root, patch.files, patch.patch);
}
// The controlled loader shares this exact installer-owned implementation, never a user plugin module.
for (const [path, expected] of Object.entries(manifest.managedFiles ?? {})) {
  const source = {
    "dist/nous-controlled-plugin.mjs": "bin/controlled-plugin.mjs",
    "dist/nous-provider-custody.mjs": "bin/provider-custody.mjs",
  }[path];
  if (!source) throw Error("Unknown installer-owned native module");
  const bytes = await readFile(join(plugin, source));
  if (digest(bytes) !== expected)
    throw Error("Controlled plugin installer implementation integrity failed");
  await mkdir(dirname(join(native, path)), { recursive: true });
  await writeFile(join(native, path), bytes);
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
