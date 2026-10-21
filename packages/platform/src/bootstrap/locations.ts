// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

// Adapted from Nous 412b0620 apps/nous-core/src/locations.ts.
import { dirname, join, resolve } from "node:path";
import envPaths from "env-paths";
import { z } from "zod";

const rootNames = [
  "program",
  "runtime",
  "instance",
  "config",
  "data",
  "blob",
  "cache",
  "secret",
  "log",
  "run",
  "temp",
  "backup",
  "workspaces",
  "exports",
] as const;
export type PathName = (typeof rootNames)[number];
export type Paths = Readonly<Record<PathName, string>>;
const path = z.string().min(1).refine(value=>!value.includes('\0'),'Path contains NUL');
export const PathsSchema = z.strictObject({
    program: path, runtime:path, instance:path, config:path, data:path, blob:path,
    cache:path, secret:path, log:path, run:path, temp:path, backup:path,workspaces:path,exports:path,
});
export const PathOverridesSchema = PathsSchema.partial();
const folders: Record<(typeof rootNames)[number], string> = {
  program: "program",
  runtime: "runtime",
  instance: "instance",
  config: "config",
  data: "data",
  blob: "blobs",
  cache: "cache",
  secret: "secrets",
  log: "logs",
  run: "run",
  temp: "temp",
  backup: "backups",
  workspaces: "workspaces",
  exports: "exports",
};

export const productNames = { nous: "nous-wave", hept: "heptalogos" } as const;
export type Product = keyof typeof productNames;

/** Only bootstrap maps physical directories; consumers receive frozen absolute roots. */
export function resolveLocations(input: {
  product: Product;
  installationId: string;
  configPath: string;
  instanceHome?: string;
  paths?: Partial<Paths>;
  programRoot?: string;
}): Paths {
  const defaults = envPaths(productNames[input.product], { suffix: "" });
  if(!input.installationId)throw new TypeError('Installation identity is required');
  const installationDir=`instance-${encodeURIComponent(input.installationId).replaceAll('.','%2E')}`;
  const base = dirname(resolve(input.configPath));
  const overrides = PathOverridesSchema.parse(input.paths ?? {});
  const home = input.instanceHome ? resolve(input.instanceHome) : overrides.instance ? resolve(base,overrides.instance) : undefined;
  const data = home ? join(home, folders.data) : join(defaults.data, installationDir);
  const result = {} as Record<PathName, string>;
  for (const name of rootNames) {
    const override = overrides[name];
    if (override !== undefined) { result[name] = resolve(base, override); continue; }
    if (name === "program") result[name] = resolve(input.programRoot ?? join(home ?? data, folders.program));
    else if (home) result[name] = name === "instance" ? home : join(home, folders[name]);
    else if (name === "config") result[name] = join(defaults.config, installationDir);
    else if (name === "instance") result[name] = join(data, folders.instance);
    else if (name === "data") result[name] = data;
    else if (name === "cache" || name === "log" || name === "temp") result[name] = join(defaults[name], installationDir);
    else if (name === "run") result[name] = join(defaults.temp, installationDir, folders.run);
    else result[name] = join(data, folders[name]);
  }
  return Object.freeze(result);
}

export function resolveLocator(input: { product:Product; configPath?:string; instanceHome?:string }): string {
  const prefix = input.product === "nous" ? "NOUS" : "HEPT";
  const selected = input.configPath ?? process.env[`${prefix}_CONFIG`];
  if (selected) return resolve(selected);
  if (input.instanceHome) return resolve(input.instanceHome, "bootstrap.toml");
  return join(envPaths(productNames[input.product], { suffix:"" }).config, "bootstrap.toml");
}
