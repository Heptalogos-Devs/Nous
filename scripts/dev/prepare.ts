// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { promisify } from "node:util";
import { execFile } from "node:child_process";
import { basename, dirname, join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { repositoryRoot as repo, workspacePaths } from "../workspace.js";
import { resolveLocations } from "../../apps/nous-core/src/locations.js";
import { installRuntime } from "../../apps/nous-core/src/runtime-packs.js";
import {
  installPostgresXmlLibrary,
  preparePostgresXmlLibrary,
} from "./postgres-linux.js";

const { values } = parseArgs({
  options: { home: { type: "string" }, locator: { type: "string" } },
});
const override = process.env.NOUS_WAVE_POSTGRES_RUNTIME
  ? resolve(process.env.NOUS_WAVE_POSTGRES_RUNTIME)
  : undefined;
if (override && basename(override) !== "postgresql")
  throw new Error(
    "NOUS_WAVE_POSTGRES_RUNTIME must select RuntimeRoot/postgresql; use the same RuntimeRoot in the serve locator",
  );
const locations = await resolveLocations({
  home:
    values.home ?? (values.locator ? undefined : workspacePaths.development),
  locator: values.locator,
  installationHome: repo,
  programRoot: repo,
  runtimeRoot: override ? dirname(override) : workspacePaths.installedRuntime,
});
const installation = join(locations.runtime, "postgresql");
if (override && override !== installation)
  throw new Error(
    "NOUS_WAVE_POSTGRES_RUNTIME disagrees with the selected locator RuntimeRoot/postgresql",
  );
if (process.platform === "win32") {
  await installRuntime(
    { ...locations, program: workspacePaths.runtime },
    "postgresql",
  );
} else {
  const environment = {
    ...process.env,
    NOUS_WAVE_POSTGRES_RUNTIME: installation,
  };
  const prepare = (env: NodeJS.ProcessEnv = environment) =>
    promisify(execFile)(
      "cargo",
      [
        "run",
        "-p",
        "nous-kernel",
        "--features",
        "dev-runtime",
        "--bin",
        "dev-prepare",
      ],
      { cwd: repo, maxBuffer: 1024 * 1024, env },
    );
  try {
    await prepare();
  } catch (error) {
    const detail = error as Error & { stderr?: string };
    if (
      process.platform !== "linux" ||
      !detail.stderr?.includes("libxml2.so.2")
    )
      throw error;
    const libraryRoot = await preparePostgresXmlLibrary();
    await prepare({
      ...environment,
      LD_LIBRARY_PATH: [libraryRoot, process.env.LD_LIBRARY_PATH]
        .filter(Boolean)
        .join(":"),
    });
    await installPostgresXmlLibrary(installation, libraryRoot);
  }
}
console.log("Developer PostgreSQL runtime ready");
