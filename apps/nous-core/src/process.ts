// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  hostSchema,
  type CoreExecutionPolicy,
} from "./configuration/catalog.js";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { randomBytes } from "node:crypto";
import { createInterface } from "node:readline";
import { once } from "node:events";
import { KernelClient } from "./kernel-client.js";
import { HealthCheckResponse_ServingStatus } from "@nous-wave/protocol/grpc/health/v1/health_pb.js";

export interface KernelProcess {
  client: KernelClient;
  child: ChildProcessWithoutNullStreams;
  stop(): Promise<void>;
  configureExecution(policy: CoreExecutionPolicy): void;
}
export async function startKernel(
  executable: string,
  configPath: string,
  options: {
    timeoutMs?: number;
    shutdownTimeoutMs?: number;
  } = {},
): Promise<KernelProcess> {
  const reference = hostSchema.parse(undefined);
  const timeoutMs = options.timeoutMs ?? reference.kernel_startup_timeout_ms;
  const shutdownTimeoutMs =
    options.shutdownTimeoutMs ?? reference.kernel_shutdown_timeout_ms;
  const inheritedNames = new Set([
    "path",
    "home",
    "user",
    "logname",
    "temp",
    "tmp",
    "tmpdir",
    "systemroot",
    "systemdrive",
    "windir",
    "comspec",
    "pathext",
    "rust_log",
  ]);
  const environment: NodeJS.ProcessEnv = {};
  for (const [name, value] of Object.entries(process.env))
    if (inheritedNames.has(name.toLowerCase())) environment[name] = value;
  const token = randomBytes(32).toString("hex");
  const child = spawn(executable, ["--config", configPath], {
    stdio: "pipe",
    windowsHide: true,
    env: environment,
  });
  child.stderr.pipe(process.stderr);
  const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
  let startupTimer: ReturnType<typeof setTimeout> | undefined;
  const endpoint = new Promise<string>((resolve, reject) => {
    startupTimer = setTimeout(
      () => reject(new Error("Kernel startup timed out")),
      timeoutMs,
    );
    child.once("error", reject);
    child.once("exit", (code) =>
      reject(new Error(`Kernel exited during startup (${code})`)),
    );
    lines.once("line", (line) => {
      try {
        if (Buffer.byteLength(line) > 4096)
          throw new Error("Kernel bootstrap output exceeds bound");
        const value: unknown = JSON.parse(line);
        if (
          !value ||
          typeof value !== "object" ||
          !("endpoint" in value) ||
          typeof value.endpoint !== "string"
        )
          throw new Error("Invalid Kernel discovery");
        const url = new URL(value.endpoint);
        if (url.protocol !== "http:" || url.hostname !== "127.0.0.1")
          throw new Error("Kernel endpoint must be loopback");
        resolve(value.endpoint);
      } catch (error) {
        reject(error);
      }
    });
  });
  child.stdin.on("error", () => {
    /* Child exit is reported by its process state and RPC transport. */
  });
  child.stdin.write(`${token}\n`);
  const stop = async () => {
    if (child.exitCode !== null || child.signalCode !== null) return;
    const exited = once(child, "exit");
    child.stdin.end();
    const timer = setTimeout(() => child.kill(), shutdownTimeoutMs);
    try {
      await exited;
    } finally {
      clearTimeout(timer);
      lines.close();
    }
  };
  try {
    const endpointURL = await endpoint;
    const client = KernelClient.connect(endpointURL, token);
    const health = await client.health.check({
      service: "nous.wave.kernel.v1alpha1.KernelQueryService",
    });
    if (health.status !== HealthCheckResponse_ServingStatus.SERVING)
      throw new Error("Kernel is not serving");
    const processState: KernelProcess = {
      client,
      child,
      stop,
      configureExecution(policy) {
        processState.client = KernelClient.connect(endpointURL, token, policy);
      },
    };
    return processState;
  } catch (error) {
    await stop();
    throw error;
  } finally {
    clearTimeout(startupTimer);
  }
}
