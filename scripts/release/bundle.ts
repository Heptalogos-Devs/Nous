// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { workspacePaths } from "../workspace.js";
import { build } from "esbuild";
import { createHash } from "node:crypto";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { readModelImplementation } from "../../apps/nous-core/src/model/implementation.js";
export type BundledPackage = {
  name: string;
  version: string;
  license: string;
  root: string;
};
export async function bundleApplication(repo: string) {
  const files = (
    await promisify(execFile)(
      "git",
      ["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
      { cwd: repo },
    )
  ).stdout
    .split("\0")
    .filter(
      (p) =>
        /^(apps|packages|prompts|proto)\//.test(p) || p === "pnpm-lock.yaml",
    )
    .sort();
  const digest = createHash("sha256");
  for (const file of [
    "scripts/release/bundle.ts",
    "tsconfig.json",
    "package.json",
  ])
    digest.update(await readFile(join(repo, file)));
  for (const file of files) {
    const bytes = await readFile(join(repo, file)).catch(() => undefined);
    if (bytes) digest.update(file).update("\0").update(bytes);
  }
  const cache = join(
    workspacePaths.cache,
    "release/application",
    digest.digest("hex"),
  );
  const program = join(cache, "program");
  const metadata = join(cache, "packages.json");
  if (await stat(metadata).catch(() => undefined))
    return {
      program,
      packages: new Map<string, BundledPackage>(
        JSON.parse(await readFile(metadata, "utf8")) as [
          string,
          BundledPackage,
        ][],
      ),
    };
  for (const folder of ["core", "cli", "client"])
    await mkdir(join(program, folder), { recursive: true });
  await writeFile(join(program, "package.json"), '{"type":"module"}\n');
  const entries = [
    ["apps/nous-core/src/main.ts", "core/main.js"],
    ["apps/nous-core/src/launcher.ts", "core/launcher.js"],
    ["apps/nous-cli/src/main.ts", "cli/main.js"],
    ["apps/nous-cli/src/mcp-main.ts", "cli/mcp-main.js"],
    ["packages/client/src/index.ts", "client/index.js"],
    ["packages/client/src/node.ts", "client/node.js"],
  ] as const;
  const packages = new Map<
    string,
    { name: string; version: string; license: string; root: string }
  >();
  const implementations = await readModelImplementation(repo);
  for (const [entry, target] of entries) {
    const result = await build({
      absWorkingDir: repo,
      entryPoints: [entry],
      outfile: join(program, target),
      bundle: true,
      platform: "node",
      target: "node24",
      format: "esm",
      metafile: true,
      minify: true,
      define: {
        nousModelImplementations: JSON.stringify(implementations),
      },
      banner: {
        js: 'import { createRequire as __createRequire } from "node:module"; import { fileURLToPath as __fileURLToPath } from "node:url"; import { dirname as __dirnameOf } from "node:path"; const require=__createRequire(import.meta.url); const __filename=__fileURLToPath(import.meta.url); const __dirname=__dirnameOf(__filename);',
      },
      logLevel: "warning",
    });
    for (const input of Object.keys(result.metafile!.inputs)) {
      const marker = input.lastIndexOf("node_modules/");
      if (marker < 0) continue;
      const tail = input.slice(marker + 13).split("/");
      const count = tail[0]?.startsWith("@") ? 2 : 1;
      const root = resolve(
        repo,
        input.slice(0, marker + 13) + tail.slice(0, count).join("/"),
      );
      const packageMetadata = JSON.parse(
        await readFile(join(root, "package.json"), "utf8"),
      ) as { name: string; version: string; license?: string };
      packages.set(packageMetadata.name + "@" + packageMetadata.version, {
        name: packageMetadata.name,
        version: packageMetadata.version,
        license: packageMetadata.license ?? "NOASSERTION",
        root,
      });
    }
  }

  await writeFile(metadata, JSON.stringify([...packages]));
  return { program, packages };
}
