// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { CONFIG_REVISION } from "../../apps/nous-core/src/configuration/schema.js";
import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { connectNousInstance } from "@nous-wave/client/node";
import { resolveLocations } from "../../apps/nous-core/src/locations.js";
import { stringify } from "smol-toml";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
export type Boot = {
  process: ChildProcess;
  client: Awaited<ReturnType<typeof connectNousInstance>>;
};
export async function prepare(home: string) {
  const runtime = process.env.NOUS_WAVE_POSTGRES_RUNTIME
    ? dirname(process.env.NOUS_WAVE_POSTGRES_RUNTIME)
    : join(repo, "data/runtime/installed");
  const locator = join(home, "bootstrap.toml");
  await writeFile(locator, stringify({ paths: { program: repo, runtime } }));
  const locations = await resolveLocations({ locator, installationHome: repo });
  await mkdir(locations.config, { recursive: true });
  await writeFile(
    join(locations.config, "nous.toml"),
    stringify({
      config_revision: CONFIG_REVISION,
      host: {
        port: 0,
        kernel_executable:
          process.env.NOUS_WAVE_KERNEL_EXECUTABLE ??
          join(
            repo,
            "target/debug",
            process.platform === "win32" ? "nous-kernel.exe" : "nous-kernel",
          ),
      },
      consumers: [
        "consumer:smoke:memory-reference",
        "consumer:smoke:cognitive-runtime-episode",
      ].map((consumer_id) => ({
        consumer_id,
        memory: "REQUIRED",
        runtime: "REQUIRED",
        resource: "FORBIDDEN",
        max_items: 32,
        max_text_bytes: 4096,
        materialize: true,
      })),
    }),
  );
  return locator;
}
export async function boot(locator: string): Promise<Boot> {
  const child = spawn(
    process.execPath,
    [
      join(repo, "node_modules/tsx/dist/cli.mjs"),
      join(repo, "apps/nous-core/src/main.ts"),
      "--development",
      "--locator",
      locator,
      "--stop-on-stdin-close",
    ],
    { cwd: repo, stdio: ["pipe", "pipe", "pipe"], windowsHide: true },
  );
  let errors = "";
  child.stderr!.on("data", (chunk: Buffer) => {
    errors = (errors + chunk.toString()).slice(-8192);
  });
  try {
    await new Promise<void>((done, failed) => {
      const timer = setTimeout(() => {
        cleanup();
        failed(new Error(`Core startup timeout: ${errors}`));
      }, 120000);
      let pending = "";
      const cleanup = () => {
        clearTimeout(timer);
        child.off("exit", exited);
        child.off("error", errored);
        child.stdout!.off("data", output);
      };
      const exited = (code: number | null) => {
        cleanup();
        failed(new Error(`Core startup exited (${code}): ${errors}`));
      };
      const errored = (error: Error) => {
        cleanup();
        failed(error);
      };
      const output = (chunk: Buffer) => {
        pending += chunk.toString();
        const lines = pending.split("\n");
        pending = lines.pop() ?? "";
        for (const line of lines) {
          try {
            const record: unknown = JSON.parse(line);
            if (record && typeof record === "object" && "discovery" in record) {
              cleanup();
              done();
              return;
            }
          } catch {
            /* Only the READY discovery record completes startup. */
          }
        }
      };
      child.once("exit", exited).once("error", errored);
      child.stdout!.on("data", output);
    });
    const locations = await resolveLocations({
      locator,
      installationHome: repo,
    });
    return {
      process: child,
      client: await connectNousInstance({ runRoot: locations.run }),
    };
  } catch (error) {
    child.stdin!.end();
    throw error;
  }
}
export async function stop(value: Boot | undefined) {
  if (!value || value.process.exitCode !== null) return;
  const exited = once(value.process, "exit");
  value.process.stdin!.end();
  await exited;
}
