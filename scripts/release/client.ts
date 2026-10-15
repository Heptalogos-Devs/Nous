// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { execFileSync, execSync } from "node:child_process";
import { cp, mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { repositoryRoot, workspacePaths, workspaceTemp } from "../workspace.js";

const require = createRequire(import.meta.url);
type Package = {
  name: string;
  version: string;
  license: string;
  exports: Record<string, string>;
  dependencies: Record<string, string>;
};

/** Compile the actual public packages; npm carries the generated protocol with the Client. */
export async function packClient() {
  const staging = await workspaceTemp("client", "pack-");
  const client = join(staging, "client");
  const protocol = join(client, "node_modules/@nous-wave/protocol");
  const metadata = async (path: string) =>
    JSON.parse(
      await readFile(join(repositoryRoot, path, "package.json"), "utf8"),
    ) as Package;
  const clientMetadata = await metadata("packages/client");
  const protocolMetadata = await metadata("packages/protocol-ts");
  const exportsFor = (entries: Record<string, string>) =>
    Object.fromEntries(
      Object.entries(entries).map(([name, source]) => {
        const path = source
          .replace(/^\.\/src\//u, "./dist/")
          .replace(/\.ts$/u, ".js");
        return [name, { types: path.replace(/\.js$/u, ".d.ts"), import: path }];
      }),
    );
  const emit = async (
    source: string,
    output: string,
    paths?: Record<string, string[]>,
  ) => {
    const config = join(staging, source.replaceAll("/", "-") + ".json");
    await writeFile(
      config,
      JSON.stringify({
        extends: join(repositoryRoot, "tsconfig.json"),
        compilerOptions: {
          noEmit: false,
          noEmitOnError: true,
          declaration: true,
          declarationMap: false,
          sourceMap: false,
          rootDir: join(repositoryRoot, source, "src"),
          outDir: join(output, "dist"),
          paths,
        },
        include: [join(repositoryRoot, source, "src/**/*.ts")],
      }),
    );
    execFileSync(
      process.execPath,
      [
        join(dirname(require.resolve("typescript/package.json")), "bin/tsc"),
        "--project",
        config,
      ],
      {
        cwd: repositoryRoot,
        stdio: "inherit",
        windowsHide: true,
      },
    );
  };
  await mkdir(protocol, { recursive: true });
  await emit("packages/protocol-ts", protocol);
  await writeFile(
    join(protocol, "package.json"),
    JSON.stringify(
      {
        name: protocolMetadata.name,
        version: protocolMetadata.version,
        license: protocolMetadata.license,
        type: "module",
        exports: {
          "./messages": {
            types: "./dist/messages.d.ts",
            import: "./dist/messages.js",
          },
          "./*.js": {
            types: "./dist/generated/*.d.ts",
            import: "./dist/generated/*.js",
          },
        },
        dependencies: protocolMetadata.dependencies,
      },
      null,
      2,
    ) + "\n",
  );
  await emit("packages/client", client, {
    "@nous-wave/protocol/messages": [join(protocol, "dist/messages.d.ts")],
    "@nous-wave/protocol/*": [join(protocol, "dist/generated/*")],
  });
  await writeFile(
    join(client, "package.json"),
    JSON.stringify(
      {
        name: clientMetadata.name,
        version: clientMetadata.version,
        license: clientMetadata.license,
        type: "module",
        engines: { node: ">=24.21.0" },
        types: "./dist/index.d.ts",
        exports: exportsFor(clientMetadata.exports),
        files: ["dist", "LICENSE", "NOTICE"],
        dependencies: {
          ...clientMetadata.dependencies,
          "@nous-wave/protocol": protocolMetadata.version,
        },
        bundledDependencies: ["@nous-wave/protocol"],
      },
      null,
      2,
    ) + "\n",
  );
  for (const directory of [client, protocol])
    for (const name of ["LICENSE", "NOTICE"])
      await cp(join(repositoryRoot, name), join(directory, name));
  // The shell command is fixed; paths are passed only through cwd, including on Windows.
  const packed = JSON.parse(
    execSync("npm pack --json", {
      cwd: client,
      encoding: "utf8",
      windowsHide: true,
    }),
  ) as {
    filename: string;
  }[];
  if (!packed[0]?.filename)
    throw new Error("npm pack returned no Client artifact");
  const destination = join(workspacePaths.releases, "client");
  await mkdir(destination, { recursive: true });
  const artifact = join(destination, packed[0].filename);
  await rm(artifact, { force: true });
  await rename(join(client, packed[0].filename), artifact);
  await rm(staging, { recursive: true, force: true });
  return artifact;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
)
  console.log(await packClient());
