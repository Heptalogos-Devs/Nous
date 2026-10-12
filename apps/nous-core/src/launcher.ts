// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { parseArgs } from "node:util";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import {
  ConfigurationError,
  readConfiguration,
} from "./configuration/schema.js";
import { checkConfiguration } from "./configuration/check.js";
import { resolveLocations } from "./locations.js";
import { initializeConfiguration } from "./configuration/file.js";
import { runCore } from "./main.js";
import { launcherCommandOffset } from "./launcher-arguments.js";
import {
  installRuntime,
  listRuntimes,
  verifyRuntime,
} from "./runtime-packs.js";

async function main() {
  const args = process.argv.slice(2);
  const profileArgs: string[] = [];
  const forwarded: string[] = [];
  for (let index = 0; index < args.length; index++) {
    if (args[index] === "--development") profileArgs.push(args[index]!);
    else if (/^--(?:home|locator)=/.test(args[index]!))
      profileArgs.push(args[index]!);
    else if (args[index] === "--home" || args[index] === "--locator") {
      if (!args[index + 1])
        throw new Error("Missing instance location argument");
      profileArgs.push(args[index]!, args[++index]!);
    } else forwarded.push(args[index]!);
  }
  const { values } = parseArgs({
    args: profileArgs,
    options: {
      home: { type: "string" },
      locator: { type: "string" },
      development: { type: "boolean", default: false },
    },
  });
  const installationHome = resolve(
    dirname(fileURLToPath(import.meta.url)),
    values.development ? "../../.." : "../..",
  );
  const locations = await resolveLocations({
    home: values.home,
    locator: values.locator,
    installationHome,
    ...(values.development
      ? {
          programRoot: installationHome,
          runtimeRoot: join(installationHome, "data/runtime/installed"),
        }
      : {}),
  });
  const commandOffset = launcherCommandOffset(forwarded);
  const [command, action, name] = forwarded.slice(commandOffset);
  if (command === "config" && action === "check") {
    const checked = await checkConfiguration(locations, values.development);
    console.log(JSON.stringify(checked));
    if (!checked.valid) process.exitCode = 1;
    return;
  }
  if (command === "runtime") {
    if (action === "list")
      console.log(JSON.stringify(await listRuntimes(locations)));
    else if (action === "verify" && name) {
      const pack = await verifyRuntime(locations, name);
      console.log(
        JSON.stringify({
          component: pack.component,
          version: pack.version,
          status: "PASS",
        }),
      );
    } else if (action === "install" && name) {
      const { values: install } = parseArgs({
        args: forwarded.slice(commandOffset + 3),
        options: { pack: { type: "string" } },
      });
      await initializeConfiguration(locations);
      const { config } = await readConfiguration(locations, values.development);
      const controller = new AbortController();
      process.once("SIGINT", () => controller.abort());
      console.log(
        JSON.stringify(
          await installRuntime(
            locations,
            name,
            install.pack,
            controller.signal,
            config.host.runtime_download_timeout_ms,
          ),
        ),
      );
    } else
      throw new Error(
        "Use runtime list | verify <component> | install <component> [--pack <archive>]",
      );
    return;
  }
  if (command === "init") {
    console.log(JSON.stringify(await initializeConfiguration(locations)));
    return;
  }
  if (command === "serve") {
    const { values: serve } = parseArgs({
      args: forwarded.slice(commandOffset + 1),
      options: { "stop-on-stdin-close": { type: "boolean", default: false } },
    });
    await runCore({ ...values, ...serve }, undefined, locations);
    return;
  }
  const development = values.development;
  const entry = development
    ? join(
        locations.program,
        "apps",
        "nous-cli",
        "src",
        command === "mcp" ? "mcp-main.ts" : "main.ts",
      )
    : join(
        locations.program,
        "cli",
        command === "mcp" ? "mcp-main.js" : "main.js",
      );
  const nodeArgs = development
    ? [join(locations.program, "node_modules", "tsx", "dist", "cli.mjs"), entry]
    : [entry];
  nodeArgs.push(
    "--run-root",
    locations.run,
    "--instance-root",
    locations.instance,
    ...(command === "mcp"
      ? [
          ...forwarded.slice(0, commandOffset),
          ...forwarded.slice(commandOffset + 1),
        ]
      : forwarded),
  );
  const child = spawn(process.execPath, nodeArgs, {
    stdio: "inherit",
    windowsHide: true,
  });
  for (const signal of ["SIGINT", "SIGTERM"] as const)
    process.once(signal, () => child.kill(signal));
  child.once("error", (error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
  child.once("exit", (code) => {
    process.exitCode = code ?? 1;
  });
}
main().catch((error) => {
  if (error instanceof ConfigurationError) {
    console.error(
      JSON.stringify({
        valid: false,
        issues: error.issues,
      }),
    );
    process.exitCode = 1;
    return;
  }
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
