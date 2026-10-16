#!/usr/bin/env node
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import {
  mkdir,
  readFile,
  rename,
  writeFile,
  copyFile,
  chmod,
  rm,
  readdir,
  realpath,
  lstat,
} from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  inspectPluginArtifact,
  verifyInstalledPlugin,
  assertNoActivePluginState,
} from "./controlled-plugin.mjs";

/** @typedef {{name?:string,version?:string,nousManagedRuntime?:number,dependencies?:Record<string,string>,overrides?:Record<string,string>,[field:string]:unknown}} RuntimePackage */
/** @typedef {import("./controlled-plugin.mjs").ControlledPluginRegistry} ControlledPluginRegistry */
const plugin = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const prefix = process.argv[2] ? resolve(process.argv[2]) : undefined;
const additions = [],
  removals = [];
for (let index = 3; index < process.argv.length; index += 2) {
  const option = process.argv[index],
    value = process.argv[index + 1];
  if (!value || !["--plugin", "--remove-plugin"].includes(option ?? ""))
    throw Error(
      "Usage: nous-openclaw-install <runtime-directory> [--plugin <local.tgz>] [--remove-plugin <plugin-id>]",
    );
  if (option === "--plugin") additions.push(resolve(value));
  else removals.push(value);
}
if (!prefix)
  throw Error("An explicit controlled runtime directory is required");
const manifest =
  /** @type {{version:string,dependencyOverrides:Record<string,string>}} */ (
    JSON.parse(await readFile(join(plugin, "patches/manifest.json"), "utf8"))
  );
const existing = await readFile(join(prefix, "package.json"), "utf8")
  .then((text) => /** @type {RuntimePackage} */ (JSON.parse(text)))
  .catch((error) => {
    if (error instanceof Error && "code" in error && error.code === "ENOENT")
      return undefined;
    throw error;
  });
if (existing && existing.nousManagedRuntime !== 1)
  throw Error(
    "Choose a fresh runtime directory; this directory is not owned by the Nous native installer",
  );
if (
  !existing &&
  (
    await readdir(prefix).catch((error) => {
      if (error instanceof Error && "code" in error && error.code === "ENOENT")
        return [];
      throw error;
    })
  ).length
)
  throw Error("A fresh runtime directory must be empty");
/** @param {string} path */
const ownedDirectory = async (path) => {
  const stat = await lstat(path);
  if (
    !stat.isDirectory() ||
    stat.isSymbolicLink() ||
    (await realpath(path)) !== resolve(path)
  )
    throw Error("Controlled runtime directories must not be redirected");
};
await mkdir(prefix, { recursive: true });
await ownedDirectory(prefix);
const modules = join(prefix, "node_modules");
await mkdir(modules, { recursive: true });
await ownedDirectory(modules);
const lock = join(prefix, ".nous-install-lock");
try {
  await mkdir(lock, { mode: 0o700 });
} catch (error) {
  if (!(error instanceof Error && "code" in error && error.code === "EEXIST"))
    throw error;
  throw Error(
    `Controlled installer lock exists at ${lock}; verify that its installer has stopped, then remove the lock explicitly before retrying`,
    { cause: error },
  );
}
await writeFile(
  join(lock, "owner.json"),
  JSON.stringify({ pid: process.pid }) + "\n",
  { mode: 0o600 },
);
/** @param {string} path @param {unknown} value */
const writeJson = async (path, value) => {
  const temporary = `${path}.${randomUUID()}.tmp`;
  await writeFile(temporary, JSON.stringify(value, null, 2) + "\n", {
    mode: 0o600,
  });
  await rename(temporary, path);
};
try {
  assertNoActivePluginState(prefix);
  const registryPath = join(prefix, "nous-local-plugins.json");
  const registry = await readFile(registryPath, "utf8")
    .then((text) => /** @type {ControlledPluginRegistry} */ (JSON.parse(text)))
    .catch((error) => {
      if (error instanceof Error && "code" in error && error.code === "ENOENT")
        return { schemaVersion: 1, plugins: [] };
      throw error;
    });
  if (registry.schemaVersion !== 1 || !Array.isArray(registry.plugins))
    throw Error("Controlled plugin installation record is invalid");
  const nativeRoot = join(prefix, "node_modules/openclaw");
  let descriptors = existing
    ? additions.map((path) => ({
        path,
        record: inspectPluginArtifact(nativeRoot, path),
      }))
    : undefined;
  const earlyIds = new Set([
    ...removals,
    ...(descriptors ?? []).map(({ record }) => record.pluginId),
  ]);
  const earlyPackages = new Set(
    (descriptors ?? []).map(({ record }) => record.packageName),
  );
  await writeJson(registryPath, {
    ...registry,
    plugins: registry.plugins.filter(
      (record) =>
        !earlyIds.has(record.pluginId) &&
        !earlyPackages.has(record.packageName),
    ),
  });
  await mkdir(join(prefix, "artifacts"), { recursive: true });
  await ownedDirectory(join(prefix, "artifacts"));
  const npmCli =
    process.platform === "win32"
      ? process.env.npm_execpath?.endsWith("npm-cli.js")
        ? process.env.npm_execpath
        : join(dirname(process.execPath), "node_modules/npm/bin/npm-cli.js")
      : undefined;
  const npm = npmCli ? process.execPath : "npm";
  /** @param {string[]} args */
  const npmArguments = (args) => (npmCli ? [npmCli, ...args] : args);
  /** @param {string[]} args @param {boolean} [capture] */
  const npmRun = (args, capture = false) =>
    execFileSync(npm, npmArguments(args), {
      cwd: prefix,
      encoding: "utf8",
      stdio: capture ? "pipe" : "inherit",
      windowsHide: true,
    });
  const packed = /** @type {Array<{filename:string,shasum:string}>} */ (
    JSON.parse(
      npmRun(
        [
          "pack",
          plugin,
          "--json",
          "--pack-destination",
          join(prefix, "artifacts"),
        ],
        true,
      ),
    )
  )[0];
  if (!packed?.filename || !/^[a-f0-9]{40}$/u.test(packed.shasum))
    throw Error("npm pack returned no identifiable plugin artifact");
  const filename = `${basename(packed.filename, ".tgz")}-${packed.shasum}.tgz`;
  await rename(
    join(prefix, "artifacts", packed.filename),
    join(prefix, "artifacts", filename),
  );
  /** @type {RuntimePackage & {dependencies:Record<string,string>,overrides:Record<string,string>}} */
  const runtime = {
    ...existing,
    name: existing?.name ?? "nous-openclaw-runtime",
    version: existing?.version ?? "1.0.0",
    private: true,
    nousManagedRuntime: 1,
    dependencies: {
      ...existing?.dependencies,
      openclaw: manifest.version,
      "@nous-wave/openclaw": `file:artifacts/${filename}`,
    },
    overrides: { ...existing?.overrides, ...manifest.dependencyOverrides },
  };
  await writeJson(join(prefix, "package.json"), runtime);
  // A new controlled patch must start from the exact pinned npm package, never a partial old patch.
  const prepared = await readFile(
    join(nativeRoot, "nous-managed-cognition.json"),
    "utf8",
  )
    .then((text) => /** @type {unknown} */ (JSON.parse(text)))
    .catch(() => undefined);
  if (prepared && JSON.stringify(prepared) !== JSON.stringify(manifest))
    await rm(nativeRoot, { recursive: true, force: true });
  npmRun(["install", "--no-audit", "--no-fund"]);
  descriptors ??= additions.map((path) => ({
    path,
    record: inspectPluginArtifact(nativeRoot, path),
  }));
  const changedIds = new Set([
    ...removals,
    ...descriptors.map(({ record }) => record.pluginId),
  ]);
  const changedPackages = new Set(
    descriptors.map(({ record }) => record.packageName),
  );
  if (
    new Set(descriptors.map(({ record }) => record.pluginId)).size !==
      descriptors.length ||
    changedPackages.size !== descriptors.length
  )
    throw Error("Local plugin additions must have unique identities");
  const retained = registry.plugins.filter(
    (record) =>
      !changedIds.has(record.pluginId) &&
      !changedPackages.has(record.packageName),
  );
  // Revoke replacement/removal records before changing any installed local code. The install lock denies new grants throughout.
  await writeJson(registryPath, { ...registry, plugins: retained });
  for (const record of registry.plugins)
    if (
      changedIds.has(record.pluginId) ||
      changedPackages.has(record.packageName)
    )
      delete runtime.dependencies[record.packageName];
  const installed = [];
  for (const { path, record } of descriptors) {
    const digest = createHash("sha256")
      .update(await readFile(path))
      .digest("hex");
    const artifact = `artifacts/local-${digest}.tgz`;
    const stagedArtifact = join(prefix, "artifacts", `${randomUUID()}.tmp`);
    await copyFile(path, stagedArtifact);
    await chmod(stagedArtifact, 0o600);
    await rename(stagedArtifact, join(prefix, artifact));
    runtime.dependencies[record.packageName] = `file:${artifact}`;
    installed.push({ ...record, artifact });
  }
  if (descriptors.length || removals.length) {
    await writeJson(join(prefix, "package.json"), runtime);
    npmRun(["install", "--ignore-scripts", "--no-audit", "--no-fund"]);
  }
  execFileSync(
    process.execPath,
    [
      join(prefix, "node_modules/@nous-wave/openclaw/bin/prepare.mjs"),
      nativeRoot,
    ],
    { cwd: prefix, stdio: "inherit", windowsHide: true },
  );
  for (const record of installed) verifyInstalledPlugin(prefix, record);
  await writeJson(registryPath, {
    ...registry,
    plugins: [...retained, ...installed],
  });
  // This reviewable template grants nothing to existing operator profiles.
  await writeFile(
    join(prefix, "nous-source-policy.example.json"),
    JSON.stringify(
      {
        sourceRead: {
          currentConversation: true,
          sourceRefs: [],
          originConversationRefs: [],
          routes: [],
        },
      },
      null,
      2,
    ) + "\n",
    { mode: 0o600, flag: "wx" },
  ).catch((error) => {
    if (!(error instanceof Error && "code" in error && error.code === "EEXIST"))
      throw error;
  });
  process.stdout.write(`Installed controlled runtime: ${prefix}\n`);
} finally {
  await rm(lock, { recursive: true, force: true });
}
