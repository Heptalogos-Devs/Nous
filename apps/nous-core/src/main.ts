// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { toJson, type JsonObject } from "@bufbuild/protobuf";
import { ValueSchema } from "@bufbuild/protobuf/wkt";
import { ConfigExposure } from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";
import { resolvedEmbedding } from "./model/embedding-profile.js";
import { parseArgs } from "node:util";
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
import { resolveLocations } from "./locations.js";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { writeFile, rename } from "node:fs/promises";
import { stringify } from "smol-toml";
import { verifyRuntime } from "./runtime-packs.js";
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
) {
  const locations = await resolveLocations({
    home: values.home,
    locator: values.locator,
    installationHome: resolve(dirname(fileURLToPath(import.meta.url)), "../.."),
  });
  const config = await loadConfig(locations, values.development);
  if (config.deployment === "portable") {
    await verifyRuntime(locations, "node");
    if (config.kernelBootstrap.bootstrap.database.mode === "managed_private")
      await verifyRuntime(locations, "postgresql");
  }
  const instance = await claimInstance(locations);
  let kernel: Awaited<ReturnType<typeof startKernel>> | undefined;
  let app: Awaited<ReturnType<typeof createCore>> | undefined;
  let stopMaintenance: (() => Promise<void>) | undefined;
  try {
    const kernelConfig = join(locations.run, "kernel-bootstrap.toml");
    await writeFile(kernelConfig, stringify(config.kernelBootstrap), {
      mode: 0o600,
    });
    const bundlePath = config.kernelBootstrap.configuration_bundle;
    await writeFile(`${bundlePath}.tmp`, JSON.stringify(config.bundle), {
      mode: 0o600,
    });
    await rename(`${bundlePath}.tmp`, bundlePath);
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
    const externalFfmpeg = await resolveMediaExecutables(
      locations,
      effective.models,
    );
    if (config.deployment === "portable" && !externalFfmpeg)
      await verifyRuntime(locations, "ffmpeg");
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
    await new Promise<void>((stopped) => {
      const stop = () => {
        process.off("SIGINT", stop);
        process.off("SIGTERM", stop);
        if (values["stop-on-stdin-close"]) {
          process.stdin.off("end", stop);
          process.stdin.pause();
        }
        stopped();
      };
      process.once("SIGINT", stop);
      process.once("SIGTERM", stop);
      if (values["stop-on-stdin-close"]) {
        process.stdin.once("end", stop);
        process.stdin.resume();
      }
    });
  } finally {
    await stopMaintenance?.();
    await app?.close();
    await kernel?.stop();
    await instance.release();
  }
}
if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const { values } = parseArgs({
    options: {
      home: { type: "string" },
      locator: { type: "string" },
      development: { type: "boolean", default: false },
      "stop-on-stdin-close": { type: "boolean", default: false },
    },
  });
  runCore(values).catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
