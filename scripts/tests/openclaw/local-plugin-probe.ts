// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Test-only entry: invoke the actual pinned native loader with an isolated explicit profile.
import { resolve, join } from "node:path";
import { pathToFileURL } from "node:url";
const runtime = resolve(process.argv[2]!);
const packageRoot = resolve(process.argv[3]!);
const pluginId = process.argv[4]!;
const { t: loadRoot } = (await import(
  pathToFileURL(join(runtime, "node_modules/openclaw/dist/loader-UHuBXZwI.js"))
    .href
)) as {
  t: (options: unknown) => { plugins: Array<{ id: string; status: string }> };
};
const registry = loadRoot({
  config: {
    plugins: {
      enabled: true,
      allow: [pluginId],
      load: { paths: [packageRoot] },
      entries: { [pluginId]: { enabled: true } },
    },
  },
  onlyPluginIds: [pluginId],
  cache: false,
});
if (
  !registry.plugins.some(
    (record) => record.id === pluginId && record.status === "loaded",
  )
)
  throw Error("Native fixture plugin was not loaded");
