// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { workspacePaths } from "../workspace.js";
import {
  cp,
  mkdir,
  readFile,
  rename,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { dirname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { parseArgs } from "node:util";
import { bundleApplication } from "./bundle.js";
import { loadNotices } from "./notices.js";
import { writeManifest } from "./manifest.js";
import { resolveLocations } from "../../apps/nous-core/src/locations.js";
import { installRuntime } from "../../apps/nous-core/src/runtime-packs.js";
const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const { values } = parseArgs({
  options: { "runtime-root": { type: "string" } },
});
const runtimePacks = values["runtime-root"]
  ? resolve(values["runtime-root"])
  : workspacePaths.runtime;
if (process.platform !== "win32" || process.arch !== "x64")
  throw new Error("Windows x64 assembly requires Windows x64");
const parent = join(workspacePaths.releases, "windows-x64");
const current = join(parent, "current"),
  output = join(parent, "current.staging"),
  previous = join(parent, "current.previous");
async function discard(path: string) {
  if (
    !resolve(path).startsWith(resolve(parent) + sep) ||
    ![output, previous].includes(path)
  )
    throw new Error("Unexpected staging path");
  await rm(path, { recursive: true, force: true });
}
await mkdir(parent, { recursive: true });
await discard(output);
await mkdir(output);
const { program: appCache, packages } = await bundleApplication(repo);
const program = join(output, "program");
await cp(appCache, program, { recursive: true });
await mkdir(join(program, "kernel"));
for (const name of ["nous-kernel.exe", "libc++.dll", "libunwind.dll"])
  await cp(
    join(repo, "target/x86_64-pc-windows-gnullvm/release", name),
    join(program, "kernel", name),
  );
for (const [source, target] of [
  ["prompts", "prompts"],
  ["crates/persistence/migrations", "migrations"],
])
  await cp(join(repo, source!), join(program, target!), { recursive: true });
const components = JSON.parse(
  await readFile(join(runtimePacks, "manifest/runtimes.json"), "utf8"),
) as {
  packs: {
    component: string;
    version: string;
    platform: string;
    arch: string;
    sha256: string;
    manifest_sha256: string;
    archive: string;
  }[];
};
const selectedPacks = ["node", "postgresql", "ffmpeg"].map((name) => {
  const entries = components.packs.filter(
    (p) =>
      p.component === name &&
      p.platform === process.platform &&
      p.arch === process.arch,
  );
  if (entries.length !== 1) throw new Error(`No unique ${name} pack`);
  return entries[0]!;
});
await mkdir(join(program, "manifest"));
await writeFile(
  join(program, "manifest/runtimes.json"),
  JSON.stringify({ packs: selectedPacks }, null, 2) + "\n",
);
for (const pack of selectedPacks) {
  const name = pack.component;
  const cache = join(
    workspacePaths.cache,
    "release/runtime",
    `${name}-${pack.version}-${pack.platform}-${pack.arch}-${pack.sha256}`,
  );
  const locations = await resolveLocations({
    installationHome: cache,
    programRoot: program,
    runtimeRoot: cache,
  });
  await installRuntime(
    locations,
    name,
    join(runtimePacks, "packs", pack.archive),
  );
  await cp(join(cache, name), join(output, "runtime", name), {
    recursive: true,
  });
}
await mkdir(join(output, "bin"));
await writeFile(
  join(output, "bin/nous.cmd"),
  '@echo off\r\n"%~dp0..\\runtime\\node\\node.exe" "%~dp0..\\program\\core\\launcher.js" %*\r\n',
);
const notices = await loadNotices(repo, packages, output);
await writeManifest(repo, output, notices, { packs: selectedPacks });
const systemRoot = process.env.SystemRoot;
if (!systemRoot) throw new Error("SystemRoot missing");
await promisify(execFile)(
  join(systemRoot, "System32/WindowsPowerShell/v1.0/powershell.exe"),
  [
    "-NoProfile",
    "-File",
    join(repo, "scripts/release/zip.ps1"),
    "-SourceRoot",
    output,
    "-ArchivePath",
    output + ".zip",
  ],
  { windowsHide: true },
);
await discard(previous);
if (await stat(current).catch(() => undefined)) await rename(current, previous);
try {
  await rename(output, current);
} catch (error) {
  if (await stat(previous).catch(() => undefined))
    await rename(previous, current);
  throw error;
}
await rm(current + ".zip", { force: true });
await rename(output + ".zip", current + ".zip");
await discard(previous);
console.log("Portable current: " + current + ".zip");
