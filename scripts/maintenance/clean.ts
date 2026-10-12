// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { lstat, readdir, rm } from "node:fs/promises";
import { join, relative, sep } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { repositoryRoot, workspacePaths } from "../workspace.js";

const execute = promisify(execFile);
const roots = [workspacePaths.temporary, workspacePaths.cache];
const clusters: string[] = [];

async function inspect(path: string) {
  const stat = await lstat(path).catch((error: NodeJS.ErrnoException) => {
    if (error.code === "ENOENT") return undefined;
    throw error;
  });
  if (!stat) return;
  if (stat.isSymbolicLink())
    throw new Error(`Cleanup refuses linked path: ${path}`);
  if (!stat.isDirectory()) return;
  const entries = await readdir(path);
  if (entries.includes("PG_VERSION")) {
    const owned = relative(workspacePaths.temporary, path);
    if (!owned || owned === ".." || owned.startsWith(`..${sep}`))
      throw new Error(`PostgreSQL cluster is outside temporary roots: ${path}`);
    clusters.push(path);
  }
  for (const entry of entries) await inspect(join(path, entry));
}

// Preflight all paths before stopping any cluster or deleting anything.
const data = await lstat(join(repositoryRoot, "data")).catch(
  (error: NodeJS.ErrnoException) => {
    if (error.code === "ENOENT") return undefined;
    throw error;
  },
);
if (data?.isSymbolicLink()) throw new Error("Cleanup refuses linked data root");
for (const root of roots) await inspect(root);
// The data directory may contain live instances and research. Only the two
// explicitly selected disposable roots are removed below.
const pgCtl = join(
  process.env.NOUS_WAVE_POSTGRES_RUNTIME ??
    join(workspacePaths.installedRuntime, "postgresql"),
  "bin",
  process.platform === "win32" ? "pg_ctl.exe" : "pg_ctl",
);
for (const cluster of clusters) {
  const status = await execute(pgCtl, ["status", "-D", cluster], {
    windowsHide: true,
  }).catch((error: Error & { code?: number }) => {
    if (error.code === 3) return undefined;
    throw error;
  });
  if (status)
    await execute(
      pgCtl,
      ["stop", "-D", cluster, "-m", "fast", "-w", "-t", "30"],
      {
        windowsHide: true,
      },
    );
}
for (const root of roots) await rm(root, { recursive: true, force: true });
console.log("Cleared repository data/temp and data/cache");
