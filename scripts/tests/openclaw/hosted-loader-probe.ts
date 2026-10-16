// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// No mock API: the actual installed native loader must register both declared tools.
import assert from "node:assert/strict";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
const native = resolve(process.argv[2]!);
const packageRoot = resolve(process.argv[3]!);
const pluginId = "nous-cognition";
const { t: loadRoot } = (await import(
  pathToFileURL(join(native, "dist/loader-UHuBXZwI.js")).href
)) as {
  t: (options: unknown) => {
    plugins: { id: string; status: string; toolNames: string[] }[];
    tools: { pluginId: string; names: string[] }[];
    diagnostics: unknown[];
  };
};
const registry = loadRoot({
  config: {
    plugins: {
      enabled: true,
      allow: [pluginId],
      load: { paths: [packageRoot] },
      slots: { contextEngine: pluginId, memory: "none" },
      entries: {
        [pluginId]: {
          enabled: true,
          config: {
            runRoot: join(packageRoot, "fixture-nous-run"),
            subjectId: "00000000-0000-4000-8000-000000000001",
            consumerId: "consumer:fixture:hosted",
            hostBridge: {
              origin: "http://127.0.0.1:1",
              credentialFile: join(packageRoot, "fixture-not-read.token"),
            },
          },
        },
      },
    },
  },
  onlyPluginIds: [pluginId],
  cache: false,
});
const plugin = registry.plugins.find((entry) => entry.id === pluginId);
assert.equal(plugin?.status, "loaded", JSON.stringify(registry.diagnostics));
assert.deepEqual(plugin.toolNames.sort(), ["nous_query", "nous_read"]);
assert.deepEqual(
  registry.tools
    .filter((tool) => tool.pluginId === pluginId)
    .flatMap((tool) => tool.names)
    .sort(),
  ["nous_query", "nous_read"],
);
console.log(
  "Actual installed SDK hosted tool declaration/registration PASS; model/Host/QQ calls=0",
);
