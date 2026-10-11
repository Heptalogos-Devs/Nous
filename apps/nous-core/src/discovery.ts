// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdir, open, readFile, unlink, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { randomUUID } from "node:crypto";

const execute = promisify(execFile);
function errorCode(error: unknown): string | undefined {
  if (
    error instanceof Error &&
    "code" in error &&
    typeof error.code === "string"
  )
    return error.code;
  return undefined;
}

function parseLock(value: unknown): { pid: number } {
  if (
    value !== null &&
    typeof value === "object" &&
    "pid" in value &&
    typeof value.pid === "number" &&
    Number.isInteger(value.pid) &&
    value.pid > 0
  )
    return { pid: value.pid };
  throw new Error("Invalid instance lock");
}

async function protectPrivateDirectory(runtime: string) {
  await mkdir(runtime, { recursive: true, mode: 0o700 });
  if (process.platform === "win32") {
    const systemRoot = process.env.SystemRoot;
    if (!systemRoot) throw new Error("Windows SystemRoot is unavailable");
    const { stdout } = await execute(
      join(systemRoot, "System32", "whoami.exe"),
      [],
      { windowsHide: true },
    );
    const user = stdout.trim();
    if (!user || /[\r\n]/.test(user))
      throw new Error("Cannot identify local credential owner");
    await execute(
      join(systemRoot, "System32", "icacls.exe"),
      [runtime, "/inheritance:r", "/grant:r", `${user}:(OI)(CI)F`],
      { windowsHide: true },
    );
  }
}
export async function claimInstance(locations: {
  run: string;
  instance: string;
  secret: string;
}) {
  const runtime = locations.run;
  for (const root of [runtime, locations.instance, locations.secret])
    await protectPrivateDirectory(root);
  const lock = join(locations.instance, "instance.lock");
  try {
    const previous = parseLock(JSON.parse(await readFile(lock, "utf8")));
    let alive = true;
    try {
      process.kill(previous.pid, 0);
    } catch (error) {
      if (errorCode(error) === "ESRCH") alive = false;
      else throw error;
    }
    if (alive) throw new Error("A Core process already owns this instance");
    await unlink(lock);
  } catch (error) {
    if (errorCode(error) !== "ENOENT") throw error;
  }
  const handle = await open(lock, "wx", 0o600);
  await handle.writeFile(JSON.stringify({ pid: process.pid }));
  await handle.close();
  const identityPath = join(locations.instance, "instance.json");
  try {
    await writeFile(
      identityPath,
      JSON.stringify({ instanceId: randomUUID() }),
      { flag: "wx", mode: 0o600 },
    );
  } catch (error) {
    if (errorCode(error) !== "EEXIST") throw error;
  }
  const identity: unknown = JSON.parse(await readFile(identityPath, "utf8"));
  if (
    !identity ||
    typeof identity !== "object" ||
    !("instanceId" in identity) ||
    typeof identity.instanceId !== "string" ||
    !/^[0-9a-f-]{36}$/i.test(identity.instanceId)
  ) {
    await unlink(lock);
    throw new Error("Invalid instance identity");
  }
  const path = join(runtime, "core.json");
  return {
    path,
    publish: (endpoint: string, token: string) =>
      writeFile(
        path,
        JSON.stringify({
          instanceId: identity.instanceId,
          endpoint,
          token,
          pid: process.pid,
        }),
        {
          mode: 0o600,
        },
      ),
    release: async () => {
      await unlink(path).catch((error: unknown) => {
        if (errorCode(error) !== "ENOENT") throw error;
      });
      await unlink(lock);
      await unlink(join(runtime, "kernel-bootstrap.toml")).catch(
        (error: unknown) => {
          if (errorCode(error) !== "ENOENT") throw error;
        },
      );
    },
  };
}
