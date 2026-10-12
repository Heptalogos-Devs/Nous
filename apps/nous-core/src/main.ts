// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { toJson, type JsonObject } from "@bufbuild/protobuf";
import { ValueSchema } from "@bufbuild/protobuf/wkt";
import { ConfigExposure } from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";
import { resolvedEmbedding } from "./model/embedding-profile.js";
import { randomBytes } from "node:crypto";
import {
  loadConfig,
  parseEffectiveConfiguration,
  loadCredentials,
  resolveMediaExecutables,
} from "./configuration/schema.js";
import { claimInstance } from "./discovery.js";
import { startKernel } from "./process.js";
import { createCore } from "./server.js";
import { ModelRuntime } from "./model/runtime.js";
import { initializeConfiguration } from "./configuration/file.js";
import { resolveLocations, type RuntimeLocations } from "./locations.js";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { writeFile } from "node:fs/promises";
import { startMaintenanceLoop } from "./maintenance/grants.js";
import { ResourceRegistry } from "./resources/registry.js";

export async function runCore(
  values: {
    home?: string;
    locator?: string;
    development?: boolean;
    "stop-on-stdin-close"?: boolean;
  },
  resources?: ResourceRegistry,
  resolvedLocations?: RuntimeLocations,
) {
  const installationHome = resolve(
    dirname(fileURLToPath(import.meta.url)),
    values.development ? "../../.." : "../..",
  );
  const locations =
    resolvedLocations ??
    (await resolveLocations({
      home: values.home,
      locator: values.locator,
      installationHome,
      ...(values.development
        ? {
            programRoot: installationHome,
            runtimeRoot: join(installationHome, "data/runtime/installed"),
          }
        : {}),
    }));
  await initializeConfiguration(locations);
  const config = await loadConfig(locations, values.development);
  const instance = await claimInstance(locations);
  let kernel: Awaited<ReturnType<typeof startKernel>> | undefined;
  let app: Awaited<ReturnType<typeof createCore>> | undefined;
  let stopMaintenance: (() => Promise<void>) | undefined;
  const errors: unknown[] = [];
  try {
    const kernelConfig = join(locations.run, "kernel-bootstrap.json");
    await writeFile(kernelConfig, JSON.stringify(config.kernelBootstrap), {
      mode: 0o600,
    });
    kernel = await startKernel(config.kernelExecutable, kernelConfig, {
      timeoutMs: config.startupTimeoutMs,
      shutdownTimeoutMs: config.shutdownTimeoutMs,
    });
    const snapshot = await kernel.client.configuration.getConfiguration({
      exposureCeiling: ConfigExposure.DEVELOPER,
    });
    const effective = parseEffectiveConfiguration(
      Object.fromEntries(
        snapshot.entries.map((entry) => [
          entry.path,
          entry.value ? toJson(ValueSchema, entry.value) : null,
        ]),
      ),
    );
    kernel.configureExecution(effective.execution);
    await loadCredentials(locations, config.dotenvFile, [
      ...Object.values(effective.models.gateway_profiles).map(
        (g) => g.credential_env,
      ),
    ]);
    await resolveMediaExecutables(locations, effective.models);
    const embedding = resolvedEmbedding(effective.models);
    await kernel.client.hostRuntime.initializeHostRuntime({
      configurationDigest: snapshot.effectiveDigest,
      resolvedEmbedding: embedding
        ? (JSON.parse(JSON.stringify(embedding)) as JsonObject)
        : undefined,
    });
    const token = randomBytes(32).toString("hex");
    const models = await ModelRuntime.fromConfig(
      effective.models,
      join(locations.program, "prompts"),
      join(locations.config, "prompts"),
      locations.temp,
    );
    app = await createCore({
      kernel: kernel.client,
      token,
      consumers: effective.consumers,
      resources: resources ?? new ResourceRegistry(),
      models,
      execution: effective.execution,
    });
    const endpoint = await app.listen({ host: "127.0.0.1", port: config.port });
    await instance.publish(endpoint, token);
    stopMaintenance = await startMaintenanceLoop(kernel.client, models);
    console.log(JSON.stringify({ endpoint, discovery: instance.path }));
    await new Promise<void>((stopped, failed) => {
      const child = kernel!.child;
      let failure: Error | undefined;
      const exited = (code: number | null, signal: NodeJS.Signals | null) => {
        failure = new Error(`Kernel exited (${signal ?? code})`);
        stop();
      };
      const stop = () => {
        process.off("SIGINT", stop);
        process.off("SIGTERM", stop);
        if (values["stop-on-stdin-close"]) {
          process.stdin.off("end", stop);
          process.stdin.pause();
        }
        child.off("exit", exited);
        if (failure) failed(failure);
        else stopped();
      };
      child.once("exit", exited);
      if (child.exitCode !== null || child.signalCode !== null) {
        exited(child.exitCode, child.signalCode);
        return;
      }
      process.once("SIGINT", stop);
      process.once("SIGTERM", stop);
      if (values["stop-on-stdin-close"]) {
        process.stdin.once("end", stop);
        process.stdin.resume();
      }
    });
  } catch (error) {
    errors.push(error);
  } finally {
    for (const close of [
      stopMaintenance,
      () => app?.close(),
      () => kernel?.stop(),
      () => instance.release(),
    ]) {
      try {
        await close?.();
      } catch (error) {
        errors.push(error);
      }
    }
  }
  if (errors.length === 1) throw errors[0];
  if (errors.length) throw new AggregateError(errors, "Core lifecycle failed");
}
