// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { z } from "zod";
import { readFile, stat } from "node:fs/promises";
import { join, resolve } from "node:path";
import { parse } from "smol-toml";
import { parseEnv } from "node:util";
import {
  modelConfigurationSchema,
  modelExecutionSchema,
} from "../model/configuration.js";
import type { RuntimeLocations } from "../locations.js";
import {
  CONFIG_REVISION,
  bootstrapSchema,
  consumersSchema,
  configurationBundle,
  coreExecutionSchema,
  coreConfigurationGroup,
} from "./catalog.js";
export { CONFIG_REVISION } from "./catalog.js";

export class ConfigurationError extends Error {
  constructor(
    readonly issues: { path: string; code: string; message: string }[],
  ) {
    super(issues.map((issue) => `${issue.path}: ${issue.message}`).join("; "));
  }
}

/** File loading parses only bootstrap; Kernel validates the full managed document. */
export function parseConfiguration(text: string, development = false) {
  let document;
  try {
    document = parse(text);
  } catch {
    throw new ConfigurationError([
      {
        path: "nous.toml",
        code: "invalid_toml",
        message: "Invalid TOML syntax",
      },
    ]);
  }
  if (document.config_revision !== CONFIG_REVISION)
    throw new ConfigurationError([
      {
        path: "config_revision",
        code: "revision_mismatch",
        message: `Expected configuration revision ${CONFIG_REVISION}`,
      },
    ]);
  const parsed = bootstrapSchema.safeParse(document);
  if (!parsed.success)
    throw new ConfigurationError(
      parsed.error.issues.map((issue) => ({
        path: issue.path.join("."),
        code: issue.code,
        message: issue.message,
      })),
    );
  if (parsed.data.host.kernel_executable && !development)
    throw new ConfigurationError([
      {
        path: "host.kernel_executable",
        code: "invalid_profile",
        message: "Kernel executable override is source-development-only",
      },
    ]);
  return { config: parsed.data, document };
}
export async function readConfiguration(
  locations: RuntimeLocations,
  development = false,
) {
  return parseConfiguration(
    await readFile(join(locations.config, "nous.toml"), "utf8"),
    development,
  );
}

/** Zod owners consume values resolved by Kernel; no override precedence exists in Core. */
export function parseEffectiveConfiguration(values: Record<string, unknown>) {
  const parseOwner = <T>(
    path: string,
    schema: z.ZodType<T>,
    input: unknown,
  ): T => {
    const parsed = schema.safeParse(input);
    if (!parsed.success)
      throw new ConfigurationError(
        parsed.error.issues.map((issue) => ({
          path: [path, ...issue.path].filter(Boolean).join("."),
          code: issue.code,
          message: issue.message,
        })),
      );
    return parsed.data;
  };
  const models = parseOwner("", modelConfigurationSchema, {
    ...parseOwner("models", modelExecutionSchema, values.models),
    audio: coreConfigurationGroup(values, "audio"),
    video: coreConfigurationGroup(values, "video"),
    material_strategy: values["material.strategy"],
    material_inputs: coreConfigurationGroup(values, "material.inputs"),
  });
  const consumers = parseOwner("consumers", consumersSchema, values.consumers);
  return {
    models,
    execution: parseOwner(
      "core_execution",
      coreExecutionSchema,
      coreConfigurationGroup(values, "core_execution"),
    ),
    consumers: consumers.map((c) => ({
      consumerId: c.consumer_id,
      revision: c.revision,
      memory: c.memory,
      runtime: c.runtime,
      resource: c.resource,
      maxItems: c.max_items,
      maxTextBytes: c.max_text_bytes,
      materialize: c.materialize,
    })),
  };
}
export function kernelExecutable(
  locations: RuntimeLocations,
  development: boolean,
  override?: string | null,
) {
  return override
    ? resolve(locations.program, override)
    : development
      ? join(
          locations.program,
          "target/debug",
          process.platform === "win32" ? "nous-kernel.exe" : "nous-kernel",
        )
      : join(
          locations.program,
          "kernel",
          process.platform === "win32" ? "nous-kernel.exe" : "nous-kernel",
        );
}
export async function loadCredentials(
  locations: RuntimeLocations,
  dotenvFile: string,
  names: readonly string[],
) {
  try {
    const credentials = parseEnv(
      await readFile(resolve(locations.secret, dotenvFile), "utf8"),
    );
    for (const name of names)
      if (process.env[name] === undefined && credentials[name] !== undefined)
        process.env[name] = credentials[name];
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT")
      // oxlint-disable-next-line preserve-caught-error -- Raw dotenv errors can include credential content.
      throw new Error("Credential dotenv file could not be loaded");
  }
}
export async function loadConfig(
  locations: RuntimeLocations,
  development = false,
) {
  const { config, document } = await readConfiguration(locations, development);
  await loadCredentials(
    locations,
    config.host.dotenv_file,
    config.database.url_env ? [config.database.url_env] : [],
  );
  const databaseUrl = config.database.url_env
    ? process.env[config.database.url_env]
    : undefined;
  if (config.database.mode === "external" && !databaseUrl)
    throw new Error("External database credential environment is unavailable");
  return {
    deployment: development ? "development" : "portable",
    locations,
    port: config.host.port,
    dotenvFile: config.host.dotenv_file,
    startupTimeoutMs: config.host.kernel_startup_timeout_ms,
    shutdownTimeoutMs: config.host.kernel_shutdown_timeout_ms,
    kernelExecutable: kernelExecutable(
      locations,
      development,
      config.host.kernel_executable,
    ),
    bundle: configurationBundle(document),
    kernelBootstrap: {
      configuration_bundle: join(locations.run, "configuration-bundle.json"),
      bootstrap: {
        database: {
          ...config.database,
          url: databaseUrl ?? "",
          url_env: undefined,
          install_dir: join(locations.runtime, "postgresql"),
          data_dir: join(locations.data, "postgres"),
          instance_dir: locations.instance,
          secret_dir: locations.secret,
        },
        object_store: { backend: "fs", root: locations.blob },
        serving: { root: join(locations.cache, "serving") },
      },
    },
  };
}
export async function resolveMediaExecutables(
  locations: RuntimeLocations,
  models: ReturnType<typeof parseEffectiveConfiguration>["models"],
) {
  const explicit = models.video.ffmpeg_executable;
  if (explicit)
    models.video.ffmpeg_executable = resolve(locations.config, explicit);
  else {
    const path = join(
      locations.runtime,
      "ffmpeg/bin",
      process.platform === "win32" ? "ffmpeg.exe" : "ffmpeg",
    );
    if ((await stat(path).catch(() => undefined))?.isFile())
      models.video.ffmpeg_executable = path;
  }
  return Boolean(explicit);
}
