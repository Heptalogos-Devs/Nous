// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { build } from "esbuild";
import {
  chmod,
  cp,
  mkdir,
  readFile,
  readdir,
  writeFile,
} from "node:fs/promises";
import { join, resolve } from "node:path";
import { repositoryRoot as repo, workspacePaths } from "../workspace.js";

/** Development and hosts use the same independently installable artifact. */
export async function buildDistribution(destination: string) {
  const source = join(repo, "packages/openclaw");
  await mkdir(join(destination, "dist"), { recursive: true });
  const result = await build({
    absWorkingDir: repo,
    entryPoints: [join(source, "src/index.ts")],
    outfile: join(destination, "dist/index.js"),
    bundle: true,
    format: "esm",
    platform: "node",
    target: "node24",
    external: ["openclaw/*"],
    metafile: true,
  });
  const metadata = JSON.parse(
    await readFile(join(source, "package.json"), "utf8"),
  ) as Record<string, unknown>;
  await writeFile(
    join(destination, "package.json"),
    JSON.stringify(
      {
        name: metadata.name,
        version: metadata.version,
        license: metadata.license,
        type: "module",
        exports: { ".": "./dist/index.js" },
        bin: {
          "nous-openclaw-prepare": "./bin/prepare.mjs",
          "nous-openclaw-install": "./bin/install.mjs",
        },
        engines: { node: ">=24.21.0" },
        openclaw: metadata.openclaw,
        peerDependencies: metadata.peerDependencies,
        peerDependenciesMeta: metadata.peerDependenciesMeta,
      },
      null,
      2,
    ) + "\n",
  );
  for (const name of ["bin", "patches", "openclaw.plugin.json", "README.md"])
    await cp(join(source, name), join(destination, name), { recursive: true });
  await chmod(join(destination, "bin/prepare.mjs"), 0o755);
  await chmod(join(destination, "bin/install.mjs"), 0o755);
  for (const name of ["LICENSE", "NOTICE"])
    await cp(join(repo, name), join(destination, name));
  const packages = new Set<string>();
  const notices: string[] = [];
  const inlineNotices = new Set<string>();
  for (const input of Object.keys(result.metafile.inputs)) {
    const marker = input.lastIndexOf("node_modules/");
    if (marker < 0) continue;
    // Protobuf's BSD varint code carries its additional terms in the source
    // header, separately from the package's root Apache license.
    const header = (await readFile(resolve(repo, input), "utf8")).match(
      /^(?:\/\/[^\n]*\n)+/u,
    )?.[0];
    if (header?.includes("Redistribution and use")) inlineNotices.add(header);
    const tail = input.slice(marker + 13).split("/");
    const root = resolve(
      repo,
      input.slice(0, marker + 13),
      ...tail.slice(0, tail[0]?.startsWith("@") ? 2 : 1),
    );
    if (packages.has(root)) continue;
    packages.add(root);
    const pkg = JSON.parse(
      await readFile(join(root, "package.json"), "utf8"),
    ) as { name: string; version: string; repository?: { url?: string } };
    const files = (await readdir(root)).filter((name) =>
      /^(license|licence|notice|copying)([.\-_]|$)/iu.test(name),
    );
    notices.push(`## ${pkg.name}@${pkg.version}\n`);
    for (const file of files)
      notices.push(await readFile(join(root, file), "utf8"));
    if (!files.length) {
      const repository = pkg.repository?.url?.match(
        /github\.com[/:]([^/]+\/[^/]+?)(?:\.git)?$/u,
      )?.[1];
      if (!repository)
        throw new Error(
          `Bundled dependency has no license source: ${pkg.name}`,
        );
      const cache = join(
        workspacePaths.cache,
        "licenses",
        `${pkg.name.replaceAll("/", "_")}@${pkg.version}`,
      );
      await mkdir(cache, { recursive: true });
      for (const name of ["LICENSE", "NOTICE"]) {
        let text = await readFile(join(cache, name), "utf8").catch(
          () => undefined,
        );
        if (text === undefined) {
          const response = await fetch(
            `https://raw.githubusercontent.com/${repository}/v${pkg.version}/${name}`,
            { signal: AbortSignal.timeout(30000) },
          );
          if (response.status === 404 && name === "NOTICE") continue;
          if (!response.ok)
            throw new Error(`Publisher ${name} unavailable: ${pkg.name}`);
          text = await response.text();
          await writeFile(join(cache, name), text);
        }
        notices.push(text);
      }
    }
  }
  await writeFile(
    join(destination, "THIRD_PARTY_NOTICES.txt"),
    [...notices, ...inlineNotices].join("\n\n"),
  );
}
